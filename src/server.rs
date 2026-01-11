use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

use crate::aerospace::{AerospaceClient, AerospaceMonitor, AerospaceWindow, RealClient};
use crate::ipc::{InternalCommand, IpcCommand, ManagerMessage, QueryTarget, StateEvent};
use crate::state::{Monitor, State, Tag, WindowInfo};
use crate::util::get_socket_path;

pub async fn run_server() -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel::<ManagerMessage>(100);
    let (event_tx, _) = tokio::sync::broadcast::channel::<StateEvent>(16);

    let client: Arc<dyn AerospaceClient> = Arc::new(RealClient);

    // --- State Manager Actor ---
    let event_tx_clone = event_tx.clone();
    let actor_tx = tx.clone(); // Clone for internal messages
    let client_clone = client.clone();
    tokio::spawn(async move {
        let client = client_clone;
        if let Ok(monitors) = client.list_monitors().await {
            initialize_all_monitors(&client, &monitors).await;
        }

        let mut state = State::new();
        tracing::info!("State manager started");

        // Initial sync
        match client.list_monitors().await {
            Ok(monitors) => {
                for m in monitors {
                    let visible_ws = m.monitor_id.to_string();
                    let monitor = Monitor::new(m.monitor_id, m.monitor_name.clone(), visible_ws);
                    state.monitors.insert(m.monitor_id, monitor);
                }
            }
            Err(e) => tracing::error!("Failed to list monitors: {}", e),
        }

        // Initial window population. Send Sync command to self.
        let _ = actor_tx.send(ManagerMessage::Ipc(IpcCommand::Sync)).await;

        while let Some(msg) = rx.recv().await {
            match msg {
                ManagerMessage::Ipc(cmd) => {
                    tokio::spawn(handle_ipc_command(cmd, actor_tx.clone(), client.clone()));
                }
                ManagerMessage::Internal(cmd) => {
                    handle_internal_command(&mut state, *cmd, &event_tx_clone, client.clone());
                }
                ManagerMessage::QueryClient(target, stream_tx) => {
                    tokio::spawn(handle_query_client(
                        target,
                        stream_tx,
                        actor_tx.clone(),
                        client.clone(),
                    ));
                }
                ManagerMessage::SubscribeClient(stream_tx) => {
                    handle_subscribe_client(stream_tx, &event_tx_clone, &state);
                }
            }
        }
    });

    // --- IPC Listener ---
    let socket_path = get_socket_path();
    if socket_path.exists() {
        tokio::fs::remove_file(&socket_path).await?;
    }
    let listener = UnixListener::bind(&socket_path)?;
    tracing::info!("Server listening on {:?}", socket_path);

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                handle_connection(stream, tx.clone());
            }
            Err(e) => tracing::error!("Accept error: {}", e),
        }
    }
}

fn handle_subscribe_client(
    mut stream_tx: tokio::net::unix::OwnedWriteHalf,
    event_tx: &tokio::sync::broadcast::Sender<StateEvent>,
    state: &State,
) {
    let mut event_rx = event_tx.subscribe();

    // Collect initial state for all monitors synchronously
    let mut initial_events = Vec::new();
    for m in state.monitors.values() {
        let occupied = calculate_occupied_tags(m);
        let event = StateEvent {
            event: "state_change".to_string(),
            monitor_id: m.id,
            selected_tags: m.selected_tags,
            occupied_tags: occupied,
        };
        if let Ok(json) = serde_json::to_string(&event) {
            initial_events.push(json);
        }
    }

    // Perform I/O in a separate task
    tokio::spawn(async move {
        // Send initial events
        for json in initial_events {
            if stream_tx
                .write_all(format!("{}\n", json).as_bytes())
                .await
                .is_err()
            {
                return; // Client disconnected
            }
        }

        // Stream future events
        while let Ok(event) = event_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&event)
                && stream_tx
                    .write_all(format!("{}\n", json).as_bytes())
                    .await
                    .is_err()
            {
                break; // Client disconnected
            }
        }
    });
}

fn handle_connection(stream: tokio::net::UnixStream, tx: mpsc::Sender<ManagerMessage>) {
    tokio::spawn(async move {
        let (mut stream_rx, stream_tx) = stream.into_split();
        let mut buf = Vec::new();
        // Read command
        if let Ok(_) = stream_rx.read_to_end(&mut buf).await
            && let Ok(cmd) = serde_json::from_slice::<IpcCommand>(&buf)
        {
            match cmd {
                IpcCommand::Subscribe => {
                    let _ = tx.send(ManagerMessage::SubscribeClient(stream_tx)).await;
                }
                IpcCommand::Query(target) => {
                    let _ = tx
                        .send(ManagerMessage::QueryClient(target, stream_tx))
                        .await;
                }
                _ => {
                    let _ = tx.send(ManagerMessage::Ipc(cmd)).await;
                }
            }
        }
    });
}

fn calculate_occupied_tags(monitor: &Monitor) -> u32 {
    let mut occupied = 0;
    for (i, tag) in monitor.tags.iter().enumerate() {
        if !tag.window_ids.is_empty() {
            occupied |= 1 << i;
        }
    }
    occupied
}

fn broadcast_state_change(
    state: &State,
    monitor_id: u32,
    event_tx: &tokio::sync::broadcast::Sender<StateEvent>,
) {
    if let Some(monitor) = state.monitors.get(&monitor_id) {
        let occupied = calculate_occupied_tags(monitor);
        let event = StateEvent {
            event: "state_change".to_string(),
            monitor_id,
            selected_tags: monitor.selected_tags,
            occupied_tags: occupied,
        };
        let _ = event_tx.send(event);
    }
}

async fn handle_query_client(
    target: QueryTarget,
    stream_tx: tokio::net::unix::OwnedWriteHalf,
    tx: mpsc::Sender<ManagerMessage>,
    client: Arc<dyn AerospaceClient>,
) {
    let mut fw: Option<AerospaceWindow> = None;
    let mut fm: Option<AerospaceMonitor> = None;

    let needs_focus_info = matches!(
        &target,
        QueryTarget::Window(None) | QueryTarget::Monitor(None) | QueryTarget::State
    );

    if needs_focus_info {
        fw = client.get_focused_window().await.ok().flatten();
        fm = client.get_focused_monitor().await.ok().flatten();
    }

    let _ = tx
        .send(ManagerMessage::Internal(Box::new(InternalCommand::Query(
            fw, fm, target, stream_tx,
        ))))
        .await;
}

// Spawns tasks to fetch external state, then sends InternalCommand back to Actor
async fn handle_ipc_command(
    cmd: IpcCommand,
    tx: mpsc::Sender<ManagerMessage>,
    client: Arc<dyn AerospaceClient>,
) {
    match cmd {
        IpcCommand::TagView(tag) => {
            let m = client.get_focused_monitor().await.ok().flatten();
            let _ = tx
                .send(ManagerMessage::Internal(Box::new(
                    InternalCommand::TagView(m, tag),
                )))
                .await;
        }
        IpcCommand::TagToggle(tag) => {
            let m = client.get_focused_monitor().await.ok().flatten();
            let _ = tx
                .send(ManagerMessage::Internal(Box::new(
                    InternalCommand::TagToggle(m, tag),
                )))
                .await;
        }
        IpcCommand::TagLast => {
            let m = client.get_focused_monitor().await.ok().flatten();
            let _ = tx
                .send(ManagerMessage::Internal(Box::new(
                    InternalCommand::TagLast(m),
                )))
                .await;
        }
        IpcCommand::TagSet(mask, monitor_id) => {
            let m = if monitor_id.is_none() {
                client.get_focused_monitor().await.ok().flatten()
            } else {
                None
            };
            let _ = tx
                .send(ManagerMessage::Internal(Box::new(InternalCommand::TagSet(
                    m, mask, monitor_id,
                ))))
                .await;
        }
        IpcCommand::WindowMove(tag) => {
            let w = client.get_focused_window().await.ok().flatten();
            let m = client.get_focused_monitor().await.ok().flatten();
            let _ = tx
                .send(ManagerMessage::Internal(Box::new(
                    InternalCommand::WindowMove(w, m, tag),
                )))
                .await;
        }
        IpcCommand::WindowSet(mask, window_id) => {
            let (w, m) = if window_id.is_none() {
                (
                    client.get_focused_window().await.ok().flatten(),
                    client.get_focused_monitor().await.ok().flatten(),
                )
            } else {
                (None, None)
            };
            let _ = tx
                .send(ManagerMessage::Internal(Box::new(
                    InternalCommand::WindowSet(w, m, mask, window_id),
                )))
                .await;
        }
        IpcCommand::WindowToggle(tag) => {
            let w = client.get_focused_window().await.ok().flatten();
            let m = client.get_focused_monitor().await.ok().flatten();
            let _ = tx
                .send(ManagerMessage::Internal(Box::new(
                    InternalCommand::WindowToggle(w, m, tag),
                )))
                .await;
        }
        IpcCommand::WindowMoveMonitor(target) => {
            let w = client.get_focused_window().await.ok().flatten();
            let m = client.get_focused_monitor().await.ok().flatten();
            let _ = tx
                .send(ManagerMessage::Internal(Box::new(
                    InternalCommand::WindowMoveMonitor(w, m, target),
                )))
                .await;
        }
        IpcCommand::Sync => {
            // Fetch all needed info in parallel
            let windows = client.list_windows().await;
            let monitors_result = client.list_monitors().await;

            let mut visible_workspaces = std::collections::HashMap::new();
            if let Ok(ref monitors) = monitors_result {
                for m in monitors {
                    if let Ok(ws) = client.get_visible_workspace(m.monitor_id).await {
                        visible_workspaces.insert(m.monitor_id, ws);
                    }
                }
            }

            let fw_workspace = client.get_focused_workspace().await;
            let fw_window = client.get_focused_window().await;
            let fw_monitor = client.get_focused_monitor().await;

            let _ = tx
                .send(ManagerMessage::Internal(Box::new(InternalCommand::Sync(
                    windows,
                    monitors_result,
                    visible_workspaces,
                    fw_workspace,
                    fw_window,
                    fw_monitor,
                ))))
                .await;
        }
        IpcCommand::Subscribe => {} // Already handled
        IpcCommand::Query(_) => {}  // Handled via QueryClient message
    }
}

fn handle_internal_command(
    state: &mut State,
    cmd: InternalCommand,
    event_tx: &tokio::sync::broadcast::Sender<StateEvent>,
    client: Arc<dyn AerospaceClient>,
) {
    match cmd {
        InternalCommand::TagView(Some(m), tag) => {
            tracing::info!("Switching to tag {}", tag);
            let mut sync_data = None;
            if let Some(monitor) = state.get_monitor_mut(m.monitor_id) {
                monitor.select_tag(tag);
                sync_data = Some((
                    monitor.tags.clone(),
                    monitor.selected_tags,
                    monitor.visible_workspace.clone(),
                ));
            }

            if let Some((tags, selected_tags, visible_workspace)) = sync_data {
                broadcast_state_change(state, m.monitor_id, event_tx);
                let hidden_workspace = state.hidden_workspace.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    sync_monitor_state(
                        &client,
                        &tags,
                        selected_tags,
                        &visible_workspace,
                        &hidden_workspace,
                    )
                    .await;
                });
            } else {
                tracing::warn!("Monitor {} not found in state", m.monitor_id);
            }
        }
        InternalCommand::TagView(None, _) => tracing::warn!("No focused monitor found"),

        InternalCommand::TagToggle(Some(m), tag) => {
            tracing::info!("Toggling tag {}", tag);
            let mut sync_data = None;
            if let Some(monitor) = state.get_monitor_mut(m.monitor_id) {
                monitor.toggle_tag(tag);
                sync_data = Some((
                    monitor.tags.clone(),
                    monitor.selected_tags,
                    monitor.visible_workspace.clone(),
                ));
            }

            if let Some((tags, selected_tags, visible_workspace)) = sync_data {
                broadcast_state_change(state, m.monitor_id, event_tx);
                let hidden_workspace = state.hidden_workspace.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    sync_monitor_state(
                        &client,
                        &tags,
                        selected_tags,
                        &visible_workspace,
                        &hidden_workspace,
                    )
                    .await;
                });
            }
        }
        InternalCommand::TagToggle(None, _) => tracing::warn!("No focused monitor found"),

        InternalCommand::TagLast(Some(m)) => {
            tracing::info!("Restoring last tags");
            let mut sync_data = None;
            if let Some(monitor) = state.get_monitor_mut(m.monitor_id) {
                monitor.restore_last_tags();
                sync_data = Some((
                    monitor.tags.clone(),
                    monitor.selected_tags,
                    monitor.visible_workspace.clone(),
                ));
            }

            if let Some((tags, selected_tags, visible_workspace)) = sync_data {
                broadcast_state_change(state, m.monitor_id, event_tx);
                let hidden_workspace = state.hidden_workspace.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    sync_monitor_state(
                        &client,
                        &tags,
                        selected_tags,
                        &visible_workspace,
                        &hidden_workspace,
                    )
                    .await;
                });
            }
        }
        InternalCommand::TagLast(None) => tracing::warn!("No focused monitor found"),

        InternalCommand::TagSet(focused_monitor, mask, monitor_id_opt) => {
            tracing::info!("Setting tag mask to {:b}", mask);
            let mut target_monitor_id = None;
            if let Some(id) = monitor_id_opt {
                target_monitor_id = Some(id);
            } else if let Some(m) = focused_monitor {
                target_monitor_id = Some(m.monitor_id);
            }

            if let Some(mid) = target_monitor_id {
                let mut sync_data = None;
                if let Some(monitor) = state.get_monitor_mut(mid) {
                    monitor.previous_tags = monitor.selected_tags;
                    monitor.selected_tags = mask;
                    sync_data = Some((
                        monitor.tags.clone(),
                        monitor.selected_tags,
                        monitor.visible_workspace.clone(),
                    ));
                }

                if let Some((tags, selected_tags, visible_workspace)) = sync_data {
                    broadcast_state_change(state, mid, event_tx);
                    let hidden_workspace = state.hidden_workspace.clone();
                    let client = client.clone();
                    tokio::spawn(async move {
                        sync_monitor_state(
                            &client,
                            &tags,
                            selected_tags,
                            &visible_workspace,
                            &hidden_workspace,
                        )
                        .await;
                    });
                }
            } else {
                tracing::warn!("No monitor found for tag set");
            }
        }

        InternalCommand::WindowMove(Some(w), focused_monitor, tag) => {
            tracing::info!("Moving window to tag {}", tag);
            let mut target_monitor_id = None;

            // 1. Check which monitor currently owns this window
            if let Some(mid) = state.find_monitor_by_window(w.window_id) {
                target_monitor_id = Some(mid);
            } else if let Some(m) = focused_monitor {
                // 2. If not found, fall back to focused monitor
                target_monitor_id = Some(m.monitor_id);
            }

            if let Some(mid) = target_monitor_id {
                let mut sync_data = None;
                if let Some(monitor) = state.get_monitor_mut(mid) {
                    for t in &mut monitor.tags {
                        t.window_ids.retain(|&id| id != w.window_id);
                    }
                    if (tag as usize) < monitor.tags.len() {
                        monitor.tags[tag as usize].window_ids.push(w.window_id);
                    }
                    sync_data = Some((
                        monitor.tags.clone(),
                        monitor.selected_tags,
                        monitor.visible_workspace.clone(),
                    ));
                }

                if let Some((tags, selected_tags, visible_workspace)) = sync_data {
                    broadcast_state_change(state, mid, event_tx);
                    let hidden_workspace = state.hidden_workspace.clone();

                    state.windows.entry(w.window_id).or_insert(WindowInfo {
                        id: w.window_id,
                        app_name: w.app_name,
                        title: w.window_title,
                    });

                    let client = client.clone();
                    tokio::spawn(async move {
                        sync_monitor_state(
                            &client,
                            &tags,
                            selected_tags,
                            &visible_workspace,
                            &hidden_workspace,
                        )
                        .await;
                    });
                }
            } else {
                tracing::warn!("No monitor found for window move");
            }
        }
        InternalCommand::WindowMove(None, _, _) => tracing::warn!("No focused window found"),

        InternalCommand::WindowSet(focused_window, focused_monitor, mask, window_id_opt) => {
            tracing::info!("Setting window tags to mask {:b}", mask);

            let mut target_window_id = None;
            let mut target_monitor_id = None;

            if let Some(wid) = window_id_opt {
                target_window_id = Some(wid);
                // Find monitor for this window
                if let Some(mid) = state.find_monitor_by_window(wid) {
                    target_monitor_id = Some(mid);
                } else {
                    // If not found in tags, maybe use focused monitor or search?
                    // For now, if we can't find the monitor, we can't update tags on a monitor.
                    // But if it's a new window, maybe we assign it to focused monitor?
                    // Let's stick to existing logic: find by window, else focused monitor.
                    if let Some(m) = focused_monitor {
                        target_monitor_id = Some(m.monitor_id);
                    }
                }
            } else if let Some(ref w) = focused_window {
                target_window_id = Some(w.window_id);
                if let Some(mid) = state.find_monitor_by_window(w.window_id) {
                    target_monitor_id = Some(mid);
                } else if let Some(m) = focused_monitor {
                    target_monitor_id = Some(m.monitor_id);
                }
            }

            if let (Some(wid), Some(mid)) = (target_window_id, target_monitor_id) {
                let mut sync_data = None;
                if let Some(monitor) = state.get_monitor_mut(mid) {
                    // Remove from all tags
                    for t in &mut monitor.tags {
                        t.window_ids.retain(|&id| id != wid);
                    }
                    // Add to tags specified by mask
                    for i in 0..32 {
                        if (mask & (1 << i)) != 0 && (i as usize) < monitor.tags.len() {
                            monitor.tags[i as usize].window_ids.push(wid);
                        }
                    }
                    sync_data = Some((
                        monitor.tags.clone(),
                        monitor.selected_tags,
                        monitor.visible_workspace.clone(),
                    ));
                }

                if let Some((tags, selected_tags, visible_workspace)) = sync_data {
                    broadcast_state_change(state, mid, event_tx);
                    let hidden_workspace = state.hidden_workspace.clone();

                    // Update window info if available
                    if let Some(ref w) = focused_window
                        && w.window_id == wid
                    {
                        state.windows.entry(w.window_id).or_insert(WindowInfo {
                            id: w.window_id,
                            app_name: w.app_name.clone(),
                            title: w.window_title.clone(),
                        });
                    }

                    let client = client.clone();
                    tokio::spawn(async move {
                        sync_monitor_state(
                            &client,
                            &tags,
                            selected_tags,
                            &visible_workspace,
                            &hidden_workspace,
                        )
                        .await;
                    });
                }
            } else {
                tracing::warn!("No monitor/window found for window set");
            }
        }

        InternalCommand::WindowToggle(Some(w), focused_monitor, tag) => {
            tracing::info!("Toggling window tag {}", tag);
            let mut target_monitor_id = None;

            if let Some(mid) = state.find_monitor_by_window(w.window_id) {
                target_monitor_id = Some(mid);
            } else if let Some(m) = focused_monitor {
                target_monitor_id = Some(m.monitor_id);
            }

            if let Some(mid) = target_monitor_id {
                let mut sync_data = None;
                if let Some(monitor) = state.get_monitor_mut(mid) {
                    let tag_idx = tag as usize;
                    if tag_idx < monitor.tags.len() {
                        let already_in_tag =
                            monitor.tags[tag_idx].window_ids.contains(&w.window_id);

                        let mut change_needed = false;
                        if !already_in_tag {
                            // Add
                            monitor.tags[tag_idx].window_ids.push(w.window_id);
                            change_needed = true;
                        } else {
                            // Try to remove, but check safety first
                            let mut tag_count = 0;
                            for t in &monitor.tags {
                                if t.window_ids.contains(&w.window_id) {
                                    tag_count += 1;
                                }
                            }

                            if tag_count > 1 {
                                // Safe to remove
                                monitor.tags[tag_idx]
                                    .window_ids
                                    .retain(|&id| id != w.window_id);
                                change_needed = true;
                            } else {
                                tracing::warn!(
                                    "Cannot remove last tag from window {}",
                                    w.window_id
                                );
                            }
                        }

                        if change_needed {
                            sync_data = Some((
                                monitor.tags.clone(),
                                monitor.selected_tags,
                                monitor.visible_workspace.clone(),
                            ));
                        }
                    }
                }

                if let Some((tags, selected_tags, visible_workspace)) = sync_data {
                    broadcast_state_change(state, mid, event_tx);
                    let hidden_workspace = state.hidden_workspace.clone();

                    state.windows.entry(w.window_id).or_insert(WindowInfo {
                        id: w.window_id,
                        app_name: w.app_name,
                        title: w.window_title,
                    });

                    let client = client.clone();
                    tokio::spawn(async move {
                        sync_monitor_state(
                            &client,
                            &tags,
                            selected_tags,
                            &visible_workspace,
                            &hidden_workspace,
                        )
                        .await;
                    });
                }
            } else {
                tracing::warn!("No monitor found for window toggle");
            }
        }
        InternalCommand::WindowToggle(None, _, _) => {
            tracing::warn!("No focused window found")
        }

        InternalCommand::WindowMoveMonitor(Some(w), Some(current_monitor), target) => {
            tracing::info!("Moving window to monitor {}", target);
            let mut monitor_ids: Vec<u32> = state.monitors.keys().cloned().collect();
            monitor_ids.sort();

            if let Ok(idx) = monitor_ids.binary_search(&current_monitor.monitor_id) {
                let next_idx = match target.as_str() {
                    "next" => (idx + 1) % monitor_ids.len(),
                    "prev" => (idx + monitor_ids.len() - 1) % monitor_ids.len(),
                    _ => idx,
                };
                let next_monitor_id = monitor_ids[next_idx];

                if next_monitor_id != current_monitor.monitor_id {
                    // 1. Remove from current monitor
                    if let Some(monitor) = state.get_monitor_mut(current_monitor.monitor_id) {
                        for t in &mut monitor.tags {
                            t.window_ids.retain(|&id| id != w.window_id);
                        }
                        broadcast_state_change(state, current_monitor.monitor_id, event_tx);
                    }

                    // 2. Add to next monitor's active tag
                    let mut target_workspace = String::new();
                    let mut target_tag = 0;

                    if let Some(monitor) = state.get_monitor_mut(next_monitor_id) {
                        target_tag = monitor.selected_tags.trailing_zeros() as u8;
                        target_workspace = monitor.visible_workspace.clone();
                    }

                    state.assign_window(w.window_id, target_tag, next_monitor_id);
                    broadcast_state_change(state, next_monitor_id, event_tx);

                    // 3. Move via AeroSpace (Async)
                    if !target_workspace.is_empty() {
                        let window_id = w.window_id;
                        let client = client.clone();
                        tokio::spawn(async move {
                            if let Err(e) = client
                                .move_node_to_workspace(window_id, &target_workspace)
                                .await
                            {
                                tracing::error!(
                                    "Failed to move window to monitor {}: {}",
                                    next_monitor_id,
                                    e
                                );
                            }
                            let _ = client.focus_window(window_id).await;
                        });
                    }
                }
            }
        }
        InternalCommand::WindowMoveMonitor(None, _, _) => {
            tracing::warn!("No focused window found")
        }
        InternalCommand::WindowMoveMonitor(_, None, _) => {
            tracing::warn!("No focused monitor found")
        }

        InternalCommand::Query(focused_window, focused_monitor, target, mut stream_tx) => {
            let response = match target {
                QueryTarget::Window(opt_id) => {
                    let mut target_wid = opt_id;
                    if target_wid.is_none()
                        && let Some(w) = &focused_window
                    {
                        target_wid = Some(w.window_id);
                    }

                    if let Some(wid) = target_wid {
                        if let Some(w_info) = state.windows.get(&wid) {
                            let monitor_id = state.find_monitor_by_window(wid);
                            let mut tags = 0;
                            if let Some(mid) = monitor_id
                                && let Some(m) = state.monitors.get(&mid)
                            {
                                for (i, tag) in m.tags.iter().enumerate() {
                                    if tag.window_ids.contains(&wid) {
                                        tags |= 1 << i;
                                    }
                                }
                            }
                            serde_json::json!({
                                "id": w_info.id,
                                "monitor_id": monitor_id,
                                "tags": tags,
                                "app_name": w_info.app_name,
                                "title": w_info.title
                            })
                        } else {
                            // If not in state, maybe it's the focused window that is not yet tracked?
                            if let Some(w) = focused_window {
                                if w.window_id == wid {
                                    // Not tracked yet, assume tag 0? or just return basic info
                                    serde_json::json!({
                                        "id": w.window_id,
                                        "monitor_id": focused_monitor.map(|m| m.monitor_id),
                                        "tags": 0,
                                        "app_name": w.app_name,
                                        "title": w.window_title
                                    })
                                } else {
                                    serde_json::Value::Null
                                }
                            } else {
                                serde_json::Value::Null
                            }
                        }
                    } else {
                        serde_json::Value::Null
                    }
                }
                QueryTarget::Monitor(opt_id) => {
                    let mut target_mid = opt_id;
                    if target_mid.is_none()
                        && let Some(m) = &focused_monitor
                    {
                        target_mid = Some(m.monitor_id);
                    }

                    if let Some(mid) = target_mid {
                        if let Some(m) = state.monitors.get(&mid) {
                            serde_json::json!({
                                "id": m.id,
                                "name": m.name,
                                "selected_tags": m.selected_tags,
                                "occupied_tags": calculate_occupied_tags(m),
                                "visible_workspace": m.visible_workspace
                            })
                        } else {
                            serde_json::Value::Null
                        }
                    } else {
                        serde_json::Value::Null
                    }
                }
                QueryTarget::State => {
                    // Need to make State serializable or construct it
                    // Let's implement Serialize for State in state.rs or just dump monitors and windows
                    // For now simple dump
                    let monitors: std::collections::HashMap<_, _> = state
                        .monitors
                        .iter()
                        .map(|(k, m)| {
                            (
                                k,
                                serde_json::json!({
                                    "id": m.id,
                                    "name": m.name,
                                    "selected_tags": m.selected_tags,
                                    "occupied_tags": calculate_occupied_tags(m),
                                    "visible_workspace": m.visible_workspace
                                }),
                            )
                        })
                        .collect();
                    serde_json::json!({
                        "focused_monitor_id": focused_monitor.map(|m| m.monitor_id),
                        "monitors": monitors,
                        "windows": state.windows
                    })
                }
            };

            tokio::spawn(async move {
                if let Ok(json) = serde_json::to_string(&response) {
                    let _ = stream_tx.write_all(json.as_bytes()).await;
                }
            });
        }

        InternalCommand::Sync(
            Ok(windows),
            Ok(monitors),
            visible_workspaces,
            fw_workspace,
            fw_window,
            fw_monitor,
        ) => {
            tracing::info!("Syncing windows");
            let mut changed_monitors = std::collections::HashSet::new();
            tracing::debug!("AeroSpace reported {} windows", windows.len());

            // 0. Update monitors and handle removed ones
            let current_monitor_ids: std::collections::HashSet<u32> =
                monitors.iter().map(|m| m.monitor_id).collect();

            // Add or update monitors
            let mut new_monitor_detected = false;
            for m in &monitors {
                let visible_ws = visible_workspaces
                    .get(&m.monitor_id)
                    .cloned()
                    .unwrap_or_else(|| m.monitor_id.to_string());

                if let Some(monitor) = state.monitors.get_mut(&m.monitor_id) {
                    if monitor.visible_workspace != visible_ws && !visible_ws.starts_with("h-") {
                        tracing::info!(
                            "Monitor {} visible workspace changed: {} -> {}",
                            m.monitor_id,
                            monitor.visible_workspace,
                            visible_ws
                        );
                        monitor.visible_workspace = visible_ws.clone();
                    }
                } else {
                    new_monitor_detected = true;
                    let initial_ws = if visible_ws.starts_with("h-") {
                        m.monitor_id.to_string()
                    } else {
                        visible_ws
                    };
                    tracing::info!(
                        "New monitor detected: {} (ws: {})",
                        m.monitor_id,
                        initial_ws
                    );
                    state.monitors.insert(
                        m.monitor_id,
                        Monitor::new(m.monitor_id, m.monitor_name.clone(), initial_ws.clone()),
                    );
                }
            }

            if new_monitor_detected {
                let monitors_clone = monitors.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    initialize_all_monitors(&client, &monitors_clone).await;
                });
            }
            let removed_monitor_ids: Vec<u32> = state
                .monitors
                .keys()
                .filter(|id| !current_monitor_ids.contains(id))
                .cloned()
                .collect();

            if !removed_monitor_ids.is_empty() {
                // Find a rescue monitor (e.g., the one with the smallest ID)
                let rescue_monitor_id = current_monitor_ids.iter().min().cloned();

                if let Some(rescue_id) = rescue_monitor_id {
                    tracing::info!(
                        "Monitors {:?} removed. Rescuing windows to monitor {}",
                        removed_monitor_ids,
                        rescue_id
                    );

                    let mut rescued_windows = Vec::new();

                    for &removed_id in &removed_monitor_ids {
                        if let Some(monitor) = state.monitors.remove(&removed_id) {
                            for tag in monitor.tags {
                                for wid in tag.window_ids {
                                    rescued_windows.push(wid);
                                }
                            }
                        }
                    }

                    if !rescued_windows.is_empty()
                        && let Some(rescue_monitor) = state.get_monitor_mut(rescue_id)
                    {
                        let target_tag = rescue_monitor.selected_tags.trailing_zeros() as u8;
                        for wid in rescued_windows {
                            if (target_tag as usize) < rescue_monitor.tags.len() {
                                rescue_monitor.tags[target_tag as usize]
                                    .window_ids
                                    .push(wid);
                            }
                        }
                        changed_monitors.insert(rescue_id);

                        // Schedule sync for rescue monitor
                        let tags = rescue_monitor.tags.clone();
                        let selected_tags = rescue_monitor.selected_tags;
                        let visible_workspace = rescue_monitor.visible_workspace.clone();
                        let hidden_workspace = state.hidden_workspace.clone();

                        let client = client.clone();
                        tokio::spawn(async move {
                            sync_monitor_state(
                                &client,
                                &tags,
                                selected_tags,
                                &visible_workspace,
                                &hidden_workspace,
                            )
                            .await;
                        });
                    }
                } else {
                    tracing::error!("All monitors removed? Cannot rescue windows.");
                    state.monitors.clear();
                }
            }

            // 1. Identify and remove closed windows
            let current_ids: std::collections::HashSet<u32> =
                windows.iter().map(|w| w.window_id).collect();
            let stale_ids: Vec<u32> = state
                .windows
                .keys()
                .filter(|id| !current_ids.contains(id))
                .cloned()
                .collect();

            for id in stale_ids {
                tracing::info!("Removing stale window: {}", id);
                state.windows.remove(&id);
                for m in state.monitors.values_mut() {
                    let mut removed = false;
                    for t in &mut m.tags {
                        if t.window_ids.contains(&id) {
                            t.window_ids.retain(|&wid| wid != id);
                            removed = true;
                        }
                    }
                    if removed {
                        changed_monitors.insert(m.id);
                    }
                }
            }

            // 2. Add new windows
            let focused_monitor_id = fw_monitor
                .as_ref()
                .ok()
                .and_then(|m| m.as_ref())
                .map(|m| m.monitor_id);

            for w in windows {
                if !state.windows.contains_key(&w.window_id) {
                    tracing::info!(
                        "Found new window: {} ({}) Workspace: {:?}",
                        w.window_id,
                        w.app_name,
                        w.workspace
                    );

                    let mut assigned_monitor_id = None;
                    if let Some(ws) = &w.workspace {
                        for m in state.monitors.values() {
                            if &m.visible_workspace == ws {
                                assigned_monitor_id = Some(m.id);
                                break;
                            }
                        }
                    }

                    // If not found by workspace name, try to assign to focused monitor
                    // BUT only if the window has no workspace (fresh window)
                    if assigned_monitor_id.is_none() && w.workspace.is_none() {
                        assigned_monitor_id = focused_monitor_id;
                    }

                    if let Some(mid) = assigned_monitor_id {
                        if let Some(monitor) = state.get_monitor_mut(mid) {
                            let target_tag = monitor.selected_tags.trailing_zeros() as u8;
                            tracing::debug!(
                                "Assigning window {} to monitor {} tag {}",
                                w.window_id,
                                mid,
                                target_tag
                            );
                            state.assign_window(w.window_id, target_tag, mid);
                            changed_monitors.insert(mid);
                        }
                    } else {
                        // If we can't determine the monitor, we don't track it yet.
                        // It might be on a non-managed workspace or hidden.
                        tracing::warn!(
                            "Could not determine monitor for window {} (workspace: {:?}). Skipping allocation.",
                            w.window_id,
                            w.workspace
                        );
                        continue;
                    }

                    state.windows.insert(
                        w.window_id,
                        WindowInfo {
                            id: w.window_id,
                            app_name: w.app_name,
                            title: w.window_title,
                        },
                    );
                }
            }

            for mid in changed_monitors {
                broadcast_state_change(state, mid, event_tx);
            }

            // 3. Rescue focus from hidden workspace
            if let Ok(Some(ws)) = fw_workspace
                && ws.workspace.starts_with("h-")
            {
                tracing::info!("Detected focus on hidden workspace '{}'.", ws.workspace);

                if let Ok(Some(fw)) = fw_window {
                    let mut target_monitor_id = None;
                    let mut target_tag_idx = None;

                    'search: for monitor in state.monitors.values() {
                        for (i, tag) in monitor.tags.iter().enumerate() {
                            if tag.window_ids.contains(&fw.window_id) {
                                target_monitor_id = Some(monitor.id);
                                target_tag_idx = Some(i as u8);
                                break 'search;
                            }
                        }
                    }

                    if let (Some(mid), Some(tag)) = (target_monitor_id, target_tag_idx) {
                        tracing::info!(
                            "Window {} belongs to Monitor {} Tag {}. Switching...",
                            fw.window_id,
                            mid,
                            tag
                        );

                        let mut sync_data = None;
                        if let Some(monitor) = state.get_monitor_mut(mid) {
                            monitor.select_tag(tag);
                            sync_data = Some((
                                monitor.tags.clone(),
                                monitor.selected_tags,
                                monitor.visible_workspace.clone(),
                            ));
                        }

                        if let Some((tags, selected_tags, visible_workspace)) = sync_data {
                            broadcast_state_change(state, mid, event_tx);
                            let hidden_workspace = state.hidden_workspace.clone();

                            let client = client.clone();
                            tokio::spawn(async move {
                                sync_monitor_state(
                                    &client,
                                    &tags,
                                    selected_tags,
                                    &visible_workspace,
                                    &hidden_workspace,
                                )
                                .await;
                            });
                        }

                        // If the focus was on a WRONG monitor (e.g. clicked Dock on Monitor B for window on Monitor A),
                        // Monitor B is now stuck on "h-xxx". We must restore it to its visible workspace.
                        if let Ok(Some(current_monitor)) = &fw_monitor
                            && current_monitor.monitor_id != mid
                            && let Some(wrong_monitor) =
                                state.monitors.get(&current_monitor.monitor_id)
                        {
                            let restore_ws = wrong_monitor.visible_workspace.clone();
                            tracing::info!(
                                "Restoring monitor {} from hidden workspace to {}",
                                current_monitor.monitor_id,
                                restore_ws
                            );
                            let client = client.clone();
                            tokio::spawn(async move {
                                // Wait a bit to let the primary sync happen first?
                                // Or just fire it. AeroSpace queues commands usually.
                                let _ = client.focus_workspace(&restore_ws).await;
                            });
                        }
                    } else {
                        tracing::warn!(
                            "Focused window {} on hidden workspace not found in state tags.",
                            fw.window_id
                        );
                    }
                }
            }
        }
        InternalCommand::Sync(Err(e), _, _, _, _, _) => {
            tracing::error!("Failed to list windows from AeroSpace: {}", e);
        }
        InternalCommand::Sync(_, Err(e), _, _, _, _) => {
            tracing::error!("Failed to list monitors from AeroSpace: {}", e);
        }
    }
}

async fn sync_monitor_state(
    client: &Arc<dyn AerospaceClient>,
    tags: &[Tag],
    selected_tags: u32,
    visible_workspace: &str,
    _hidden_workspace: &str,
) {
    tracing::debug!(
        "Syncing monitor state. Visible: {}, SelectedTags: {:b}",
        visible_workspace,
        selected_tags
    );

    // 1. Move windows to HIDDEN first
    for (i, tag) in tags.iter().enumerate() {
        if (selected_tags & (1 << i)) == 0 {
            for &window_id in &tag.window_ids {
                let hidden_ws = format!("h-{}", window_id);
                if let Err(e) = client.move_node_to_workspace(window_id, &hidden_ws).await {
                    tracing::error!("Failed to hide window {}: {}", window_id, e);
                }
            }
        }
    }

    // 2. Move windows to VISIBLE next
    for (i, tag) in tags.iter().enumerate() {
        if (selected_tags & (1 << i)) != 0 {
            for &window_id in &tag.window_ids {
                if let Err(e) = client
                    .move_node_to_workspace(window_id, visible_workspace)
                    .await
                {
                    tracing::error!("Failed to show window {}: {}", window_id, e);
                }
            }
        }
    }

    // Restore focus to the visible workspace to prevent getting stuck in hidden workspace
    tracing::debug!(
        "Restoring focus to visible workspace: {}",
        visible_workspace
    );
    if let Err(e) = client.focus_workspace(visible_workspace).await {
        tracing::error!("Failed to restore focus to {}: {}", visible_workspace, e);
    }
}

async fn initialize_all_monitors(client: &Arc<dyn AerospaceClient>, monitors: &[AerospaceMonitor]) {
    if monitors.is_empty() {
        return;
    }

    let mut sorted: Vec<_> = monitors.to_vec();
    sorted.sort_by_key(|m| m.monitor_id);

    let mut current_state: std::collections::HashMap<u32, String> =
        std::collections::HashMap::new();
    for m in &sorted {
        if let Ok(ws) = client.get_visible_workspace(m.monitor_id).await {
            current_state.insert(m.monitor_id, ws);
        }
    }

    let already_aligned = sorted.iter().all(|m| {
        current_state
            .get(&m.monitor_id)
            .map(|ws| ws == &m.monitor_id.to_string())
            .unwrap_or(false)
    });

    if already_aligned {
        tracing::debug!("All monitors already aligned");
        return;
    }

    tracing::info!(
        "Realigning workspaces: current={:?},
        expected={:?}",
        current_state,
        sorted
            .iter()
            .map(|m| (m.monitor_id, m.monitor_id.to_string()))
            .collect::<Vec<_>>()
    );

    // Ensure all required workspaces exist by focusing them once
    for m in &sorted {
        let ws_name = m.monitor_id.to_string();
        let _ = client.focus_workspace(&ws_name).await;
    }

    // Explicitly move each workspace to its correct monitor
    for m in &sorted {
        let ws_name = m.monitor_id.to_string();
        tracing::debug!("Moving workspace {} to monitor {}", ws_name, m.monitor_id);
        let _ = client
            .move_workspace_to_monitor(&ws_name, m.monitor_id)
            .await;
    }

    // Focus each monitor on its correct workspace
    for m in &sorted {
        let ws_name = m.monitor_id.to_string();
        let _ = client.focus_monitor(m.monitor_id).await;
        let _ = client.focus_workspace(&ws_name).await;
    }

    // Focus the primary monitor (lowest ID) so user sees the change
    if let Some(primary) = sorted.first() {
        tracing::info!("Focusing primary monitor {}", primary.monitor_id);
        let _ = client.focus_monitor(primary.monitor_id).await;
        let _ = client
            .focus_workspace(&primary.monitor_id.to_string())
            .await;
    }

    tracing::info!("Workspace realignment complete");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aerospace::MockAerospaceClient;
    use tokio::net::UnixStream;

    #[tokio::test]
    async fn test_window_toggle_safety_guard() {
        let mut state = State::new();
        let mut monitor = Monitor::new(1, "Main".to_string(), "1".to_string());
        // Window 100 is in Tag 0
        monitor.tags[0].window_ids.push(100);
        state.monitors.insert(1, monitor);

        let mock = MockAerospaceClient::new();
        let client: Arc<dyn AerospaceClient> = Arc::new(mock);
        let (event_tx, _rx) = tokio::sync::broadcast::channel(16);

        // Try to toggle Tag 0 for Window 100 (which is the only tag it has)
        let cmd = InternalCommand::WindowToggle(
            Some(AerospaceWindow {
                window_id: 100,
                app_name: "TestApp".into(),
                window_title: "TestWindow".into(),
                workspace: None,
            }),
            None,
            0,
        );

        handle_internal_command(&mut state, cmd, &event_tx, client);

        // Verify: Window 100 should STILL be in Tag 0
        let m = state.monitors.get(&1).unwrap();
        assert!(
            m.tags[0].window_ids.contains(&100),
            "Safety guard failed: Last tag was removed!"
        );
    }

    #[tokio::test]
    async fn test_window_toggle_normal() {
        let mut state = State::new();
        let mut monitor = Monitor::new(1, "Main".to_string(), "1".to_string());
        // Window 100 is in Tag 0 AND Tag 1
        monitor.tags[0].window_ids.push(100);
        monitor.tags[1].window_ids.push(100);
        state.monitors.insert(1, monitor);

        let mock = MockAerospaceClient::new();
        let client: Arc<dyn AerospaceClient> = Arc::new(mock);
        let (event_tx, _rx) = tokio::sync::broadcast::channel(16);

        // Toggle Tag 0 -> Should be removed because it has Tag 1 also
        let cmd = InternalCommand::WindowToggle(
            Some(AerospaceWindow {
                window_id: 100,
                app_name: "TestApp".into(),
                window_title: "TestWindow".into(),
                workspace: None,
            }),
            None,
            0,
        );

        handle_internal_command(&mut state, cmd, &event_tx, client);

        let m = state.monitors.get(&1).unwrap();
        assert!(
            !m.tags[0].window_ids.contains(&100),
            "Window should be removed from Tag 0"
        );
        assert!(
            m.tags[1].window_ids.contains(&100),
            "Window should remain in Tag 1"
        );
    }

    #[tokio::test]
    async fn test_window_toggle_add() {
        let mut state = State::new();
        let mut monitor = Monitor::new(1, "Main".to_string(), "1".to_string());
        monitor.tags[0].window_ids.push(100);
        state.monitors.insert(1, monitor);

        let mock = MockAerospaceClient::new();
        let client: Arc<dyn AerospaceClient> = Arc::new(mock);
        let (event_tx, _rx) = tokio::sync::broadcast::channel(16);

        // Toggle Tag 1 (Add)
        let cmd = InternalCommand::WindowToggle(
            Some(AerospaceWindow {
                window_id: 100,
                app_name: "TestApp".into(),
                window_title: "TestWindow".into(),
                workspace: None,
            }),
            None,
            1,
        );

        handle_internal_command(&mut state, cmd, &event_tx, client);

        let m = state.monitors.get(&1).unwrap();
        assert!(m.tags[0].window_ids.contains(&100));
        assert!(
            m.tags[1].window_ids.contains(&100),
            "Window should be added to Tag 1"
        );
    }

    #[tokio::test]
    async fn test_window_move() {
        let mut state = State::new();
        let mut monitor = Monitor::new(1, "Main".to_string(), "1".to_string());
        monitor.tags[0].window_ids.push(100);
        state.monitors.insert(1, monitor);

        let mock = MockAerospaceClient::new();
        let client: Arc<dyn AerospaceClient> = Arc::new(mock);
        let (event_tx, _rx) = tokio::sync::broadcast::channel(16);

        // Move to Tag 2
        let cmd = InternalCommand::WindowMove(
            Some(AerospaceWindow {
                window_id: 100,
                app_name: "TestApp".into(),
                window_title: "TestWindow".into(),
                workspace: None,
            }),
            None,
            2,
        );

        handle_internal_command(&mut state, cmd, &event_tx, client);

        let m = state.monitors.get(&1).unwrap();
        assert!(
            !m.tags[0].window_ids.contains(&100),
            "Should be removed from Tag 0"
        );
        assert!(
            m.tags[2].window_ids.contains(&100),
            "Should be added to Tag 2"
        );
    }

    #[tokio::test]
    async fn test_tag_view() {
        let mut state = State::new();
        let monitor = Monitor::new(1, "Main".to_string(), "1".to_string());
        state.monitors.insert(1, monitor); // Default selected: Tag 0 (mask 1)

        let mock = MockAerospaceClient::new();
        let client: Arc<dyn AerospaceClient> = Arc::new(mock);
        let (event_tx, _rx) = tokio::sync::broadcast::channel(16);

        // View Tag 2 (index 2, mask 4)
        let cmd = InternalCommand::TagView(
            Some(AerospaceMonitor {
                monitor_id: 1,
                monitor_name: "Main".into(),
            }),
            2,
        );

        handle_internal_command(&mut state, cmd, &event_tx, client);

        let m = state.monitors.get(&1).unwrap();
        assert_eq!(m.selected_tags, 1 << 2);
    }

    #[tokio::test]
    async fn test_tag_toggle() {
        let mut state = State::new();
        let monitor = Monitor::new(1, "Main".to_string(), "1".to_string());
        state.monitors.insert(1, monitor); // Default selected: Tag 0 (mask 1)

        let mock = MockAerospaceClient::new();
        let client: Arc<dyn AerospaceClient> = Arc::new(mock);
        let (event_tx, _rx) = tokio::sync::broadcast::channel(16);

        // Toggle Tag 1 (mask 2). Expected: 1 | 2 = 3
        let cmd = InternalCommand::TagToggle(
            Some(AerospaceMonitor {
                monitor_id: 1,
                monitor_name: "Main".into(),
            }),
            1,
        );

        handle_internal_command(&mut state, cmd, &event_tx, client);

        let m = state.monitors.get(&1).unwrap();
        assert_eq!(m.selected_tags, 1 | 2);
    }

    #[tokio::test]
    async fn test_query_state() {
        let mut state = State::new();
        let monitor = Monitor::new(1, "Main".to_string(), "1".to_string());
        state.monitors.insert(1, monitor);

        // Mock setup for focus info
        let mock = MockAerospaceClient::new();

        let client: Arc<dyn AerospaceClient> = Arc::new(mock);
        let (event_tx, _rx) = tokio::sync::broadcast::channel(16);

        let (stream_tx, stream_rx) = UnixStream::pair().unwrap();
        let (_rx_read, tx_write) = stream_tx.into_split(); // We need OwnedWriteHalf to pass

        let focused_monitor = Some(AerospaceMonitor {
            monitor_id: 1,
            monitor_name: "Main".into(),
        });

        let cmd = InternalCommand::Query(None, focused_monitor, QueryTarget::State, tx_write);

        handle_internal_command(&mut state, cmd, &event_tx, client);

        // Read response
        let mut reader = tokio::io::BufReader::new(stream_rx);
        let mut buf = String::new();
        reader.read_to_string(&mut buf).await.unwrap();

        let json: serde_json::Value = serde_json::from_str(&buf).unwrap();
        assert_eq!(json["focused_monitor_id"], 1);
        assert!(json["monitors"]["1"].is_object());
    }

    #[tokio::test]
    async fn test_window_set() {
        let mut state = State::new();
        let mut monitor = Monitor::new(1, "Main".to_string(), "1".to_string());
        monitor.tags[0].window_ids.push(100);
        state.monitors.insert(1, monitor);

        let mock = MockAerospaceClient::new();
        let client: Arc<dyn AerospaceClient> = Arc::new(mock);
        let (event_tx, _rx) = tokio::sync::broadcast::channel(16);

        // Set Window 100 tags to mask 6 (Tag 1 and Tag 2)
        let cmd = InternalCommand::WindowSet(
            None,
            Some(AerospaceMonitor {
                monitor_id: 1,
                monitor_name: "Main".into(),
            }),
            6,
            Some(100),
        );

        handle_internal_command(&mut state, cmd, &event_tx, client);

        let m = state.monitors.get(&1).unwrap();
        assert!(!m.tags[0].window_ids.contains(&100));
        assert!(m.tags[1].window_ids.contains(&100));
        assert!(m.tags[2].window_ids.contains(&100));
    }

    #[tokio::test]
    async fn test_tag_set() {
        let mut state = State::new();
        let monitor = Monitor::new(1, "Main".to_string(), "1".to_string());
        state.monitors.insert(1, monitor);

        let mock = MockAerospaceClient::new();
        let client: Arc<dyn AerospaceClient> = Arc::new(mock);
        let (event_tx, _rx) = tokio::sync::broadcast::channel(16);

        // Set Monitor 1 tags to mask 5 (Tag 0 and Tag 2)
        let cmd = InternalCommand::TagSet(None, 5, Some(1));

        handle_internal_command(&mut state, cmd, &event_tx, client);

        let m = state.monitors.get(&1).unwrap();
        assert_eq!(m.selected_tags, 5);
    }

    #[tokio::test]
    async fn test_window_move_monitor() {
        let mut state = State::new();
        // Monitor 1 (focused) and Monitor 2
        let mut m1 = Monitor::new(1, "M1".into(), "1".into());
        let m2 = Monitor::new(2, "M2".into(), "2".into());

        // Window 100 is in Monitor 1, Tag 0
        m1.tags[0].window_ids.push(100);
        state.monitors.insert(1, m1);
        state.monitors.insert(2, m2);

        let mut mock = MockAerospaceClient::new();
        // move_monitor calls move_node_to_workspace and focus_window
        mock.expect_move_node_to_workspace()
            .with(mockall::predicate::eq(100), mockall::predicate::eq("2"))
            .returning(|_, _| Ok(()));
        mock.expect_focus_window()
            .with(mockall::predicate::eq(100))
            .returning(|_| Ok(()));

        let client: Arc<dyn AerospaceClient> = Arc::new(mock);
        let (event_tx, _rx) = tokio::sync::broadcast::channel(16);

        let cmd = InternalCommand::WindowMoveMonitor(
            Some(AerospaceWindow {
                window_id: 100,
                app_name: "App".into(),
                window_title: "Title".into(),
                workspace: None,
            }),
            Some(AerospaceMonitor {
                monitor_id: 1,
                monitor_name: "M1".into(),
            }),
            "next".into(),
        );

        handle_internal_command(&mut state, cmd, &event_tx, client);

        // Window should be removed from M1 and added to M2
        assert!(
            !state.monitors.get(&1).unwrap().tags[0]
                .window_ids
                .contains(&100)
        );
        assert!(
            state.monitors.get(&2).unwrap().tags[0]
                .window_ids
                .contains(&100)
        );
    }

    #[tokio::test]
    async fn test_ipc_subscribe() {
        let (tx, mut rx) = mpsc::channel(10);
        let (client_conn, server_conn) = UnixStream::pair().unwrap();

        // Start connection handler
        handle_connection(server_conn, tx);

        // Send Subscribe command from client side
        let mut client_conn = client_conn;
        let cmd = IpcCommand::Subscribe;
        let data = serde_json::to_vec(&cmd).unwrap();
        client_conn.write_all(&data).await.unwrap();
        client_conn.shutdown().await.unwrap();

        // Check if actor receives SubscribeClient message
        let msg = rx.recv().await.unwrap();
        match msg {
            ManagerMessage::SubscribeClient(_) => {} // Expected
            _ => panic!("Expected SubscribeClient message"),
        }
    }

    #[tokio::test]
    async fn test_subscribe_events() {
        use tokio::io::AsyncBufReadExt;

        let mut state = State::new();
        let monitor = Monitor::new(1, "Main".to_string(), "1".to_string());
        state.monitors.insert(1, monitor);

        let (event_tx, _rx) = tokio::sync::broadcast::channel(16);
        let (stream_tx, stream_rx) = UnixStream::pair().unwrap();
        let (_rx_read, tx_write) = stream_tx.into_split();

        // Start subscription handler
        handle_subscribe_client(tx_write, &event_tx, &state);

        let mut reader = tokio::io::BufReader::new(stream_rx);
        let mut line = String::new();

        // 1. Check initial event
        reader.read_line(&mut line).await.unwrap();
        let json: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(json["event"], "state_change");
        assert_eq!(json["monitor_id"], 1);
        line.clear();

        // 2. Check broadcast event
        let event = StateEvent {
            event: "state_change".into(),
            monitor_id: 1,
            selected_tags: 5,
            occupied_tags: 0,
        };
        event_tx.send(event).unwrap();

        reader.read_line(&mut line).await.unwrap();
        let json: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(json["selected_tags"], 5);
    }
}
