use argh::FromArgs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

mod aerospace;
mod state;

use state::{Monitor, State};

#[derive(Debug, FromArgs)]
/// Tag based workspace manager for AeroSpace
struct Args {
    #[argh(subcommand)]
    cmd: SubCommand,
}

#[derive(Debug, FromArgs)]
#[argh(subcommand)]
enum SubCommand {
    Server(ServerCommand),
    Switch(SwitchCommand),
    Toggle(ToggleCommand),
    Move(MoveCommand),
    Hook(HookCommand),
    Last(LastCommand),
    MoveMonitor(MoveMonitorCommand),
    Subscribe(SubscribeCommand),
}

#[derive(Debug, FromArgs)]
/// Launch workspace manager daemon
#[argh(subcommand, name = "server")]
struct ServerCommand {}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Switch to a specific tag on the focused monitor
#[argh(subcommand, name = "switch")]
struct SwitchCommand {
    #[argh(positional)]
    tag: u8,
}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Toggle a specific tag on the focused monitor
#[argh(subcommand, name = "toggle")]
struct ToggleCommand {
    #[argh(positional)]
    tag: u8,
}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Move focused window to a specific tag
#[argh(subcommand, name = "move")]
struct MoveCommand {
    #[argh(positional)]
    tag: u8,
}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Trigger synchronization (e.g., from AeroSpace exec-on-workspace-change)
#[argh(subcommand, name = "hook")]
struct HookCommand {}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Restore the last selected tags on the focused monitor
#[argh(subcommand, name = "last")]
struct LastCommand {}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Move focused window to next/prev monitor
#[argh(subcommand, name = "move-monitor")]
struct MoveMonitorCommand {
    #[argh(positional)]
    target: String,
}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Subscribe to state change events
#[argh(subcommand, name = "subscribe")]
struct SubscribeCommand {}

#[derive(Debug, Serialize, Deserialize)]
enum IpcCommand {
    Switch(u8),
    Toggle(u8),
    Move(u8),
    Sync,
    Last,
    MoveMonitor(String),
    Subscribe,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StateEvent {
    event: String,
    monitor_id: u32,
    selected_tags: u32,
    occupied_tags: u32,
}

enum ManagerMessage {
    Ipc(IpcCommand),
    SubscribeClient(tokio::net::unix::OwnedWriteHalf),
}

fn get_socket_path() -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push("aerotag.sock");
    path
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args: Args = argh::from_env();

    match args.cmd {
        SubCommand::Server(_) => run_server().await,
        SubCommand::Switch(cmd) => send_client_command(IpcCommand::Switch(cmd.tag)).await,
        SubCommand::Toggle(cmd) => send_client_command(IpcCommand::Toggle(cmd.tag)).await,
        SubCommand::Move(cmd) => send_client_command(IpcCommand::Move(cmd.tag)).await,
        SubCommand::Hook(_) => send_client_command(IpcCommand::Sync).await,
        SubCommand::Last(_) => send_client_command(IpcCommand::Last).await,
        SubCommand::MoveMonitor(cmd) => {
            send_client_command(IpcCommand::MoveMonitor(cmd.target)).await
        }
        SubCommand::Subscribe(_) => run_subscriber().await,
    }
}

async fn run_subscriber() -> anyhow::Result<()> {
    let socket_path = get_socket_path();
    let stream = UnixStream::connect(socket_path).await?;
    let (rx, mut tx) = stream.into_split();

    // Send Subscribe command
    let cmd = IpcCommand::Subscribe;
    let data = serde_json::to_vec(&cmd)?;
    tx.write_all(&data).await?;
    tx.shutdown().await?;

    let mut reader = tokio::io::BufReader::new(rx);
    let mut line = String::new();

    while reader.read_line(&mut line).await? > 0 {
        print!("{}", line);
        line.clear();
    }
    Ok(())
}

async fn run_server() -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel::<ManagerMessage>(100);
    let (event_tx, _) = tokio::sync::broadcast::channel::<StateEvent>(16);

    // --- State Manager Actor ---
    let event_tx_clone = event_tx.clone();
    tokio::spawn(async move {
        let mut state = State::new();
        tracing::info!("State manager started");

        // Initial sync
        match aerospace::list_monitors().await {
            Ok(monitors) => {
                for m in monitors {
                    let visible_ws = m.monitor_id.to_string();
                    let monitor = Monitor::new(m.monitor_id, m.monitor_name.clone(), visible_ws);
                    state.monitors.insert(m.monitor_id, monitor);
                }
            }
            Err(e) => tracing::error!("Failed to list monitors: {}", e),
        }

        // Initial window population
        handle_ipc_command(&mut state, IpcCommand::Sync, &event_tx_clone).await;

        while let Some(msg) = rx.recv().await {
            match msg {
                ManagerMessage::Ipc(cmd) => {
                    handle_ipc_command(&mut state, cmd, &event_tx_clone).await;
                }
                ManagerMessage::SubscribeClient(mut stream_tx) => {
                    let mut event_rx = event_tx_clone.subscribe();

                    // Send initial state for all monitors
                    for m in state.monitors.values() {
                        let occupied = calculate_occupied_tags(m);
                        let event = StateEvent {
                            event: "state_change".to_string(),
                            monitor_id: m.id,
                            selected_tags: m.selected_tags,
                            occupied_tags: occupied,
                        };
                        if let Ok(json) = serde_json::to_string(&event) {
                            let _ = stream_tx.write_all(format!("{}\n", json).as_bytes()).await;
                        }
                    }

                    tokio::spawn(async move {
                        while let Ok(event) = event_rx.recv().await {
                            if let Ok(json) = serde_json::to_string(&event) {
                                if stream_tx
                                    .write_all(format!("{}\n", json).as_bytes())
                                    .await
                                    .is_err()
                                {
                                    break; // Client disconnected
                                }
                            }
                        }
                    });
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
                let tx = tx.clone();
                tokio::spawn(async move {
                    let (mut stream_rx, stream_tx) = stream.into_split();
                    let mut buf = Vec::new();
                    // Read command
                    if let Ok(_) = stream_rx.read_to_end(&mut buf).await {
                        if let Ok(cmd) = serde_json::from_slice::<IpcCommand>(&buf) {
                            match cmd {
                                IpcCommand::Subscribe => {
                                    let _ =
                                        tx.send(ManagerMessage::SubscribeClient(stream_tx)).await;
                                }
                                _ => {
                                    let _ = tx.send(ManagerMessage::Ipc(cmd)).await;
                                }
                            }
                        }
                    }
                });
            }
            Err(e) => tracing::error!("Accept error: {}", e),
        }
    }
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

async fn handle_ipc_command(
    state: &mut State,
    cmd: IpcCommand,
    event_tx: &tokio::sync::broadcast::Sender<StateEvent>,
) {
    tracing::debug!("Handling IPC command: {:?}", cmd);
    match cmd {
        IpcCommand::Switch(tag) => {
            tracing::info!("Switching to tag {}", tag);
            if let Ok(Some(m)) = aerospace::get_focused_monitor().await {
                let monitor_sync_data;
                if let Some(monitor) = state.get_monitor_mut(m.monitor_id) {
                    monitor.select_tag(tag);
                    monitor_sync_data = Some((
                        monitor.tags.clone(),
                        monitor.selected_tags,
                        monitor.visible_workspace.clone(),
                    ));
                    broadcast_state_change(state, m.monitor_id, event_tx);
                } else {
                    monitor_sync_data = None;
                    tracing::warn!("Monitor {} not found in state", m.monitor_id);
                }

                if let Some((tags, selected_tags, visible_workspace)) = monitor_sync_data {
                    sync_monitor_state(
                        &tags,
                        selected_tags,
                        &visible_workspace,
                        &state.hidden_workspace,
                    )
                    .await;
                }
            }
        }
        IpcCommand::Toggle(tag) => {
            tracing::info!("Toggling tag {}", tag);
            if let Ok(Some(m)) = aerospace::get_focused_monitor().await {
                let monitor_sync_data;
                if let Some(monitor) = state.get_monitor_mut(m.monitor_id) {
                    monitor.toggle_tag(tag);
                    monitor_sync_data = Some((
                        monitor.tags.clone(),
                        monitor.selected_tags,
                        monitor.visible_workspace.clone(),
                    ));
                    broadcast_state_change(state, m.monitor_id, event_tx);
                } else {
                    monitor_sync_data = None;
                }

                if let Some((tags, selected_tags, visible_workspace)) = monitor_sync_data {
                    sync_monitor_state(
                        &tags,
                        selected_tags,
                        &visible_workspace,
                        &state.hidden_workspace,
                    )
                    .await;
                }
            }
        }
        IpcCommand::Last => {
            tracing::info!("Restoring last tags");
            if let Ok(Some(m)) = aerospace::get_focused_monitor().await {
                let monitor_sync_data;
                if let Some(monitor) = state.get_monitor_mut(m.monitor_id) {
                    monitor.restore_last_tags();
                    monitor_sync_data = Some((
                        monitor.tags.clone(),
                        monitor.selected_tags,
                        monitor.visible_workspace.clone(),
                    ));
                    broadcast_state_change(state, m.monitor_id, event_tx);
                } else {
                    monitor_sync_data = None;
                }

                if let Some((tags, selected_tags, visible_workspace)) = monitor_sync_data {
                    sync_monitor_state(
                        &tags,
                        selected_tags,
                        &visible_workspace,
                        &state.hidden_workspace,
                    )
                    .await;
                }
            }
        }
        IpcCommand::MoveMonitor(target) => {
            tracing::info!("Moving window to monitor {}", target);
            if let Ok(Some(w)) = aerospace::get_focused_window().await {
                if let Ok(Some(current_monitor)) = aerospace::get_focused_monitor().await {
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
                            if let Some(monitor) = state.get_monitor_mut(current_monitor.monitor_id)
                            {
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

                            // 3. Move via AeroSpace
                            if !target_workspace.is_empty() {
                                if let Err(e) = aerospace::move_node_to_workspace(
                                    w.window_id,
                                    &target_workspace,
                                )
                                .await
                                {
                                    tracing::error!(
                                        "Failed to move window to monitor {}: {}",
                                        next_monitor_id,
                                        e
                                    );
                                }
                                let _ = aerospace::focus_window(w.window_id).await;
                            }
                        }
                    }
                }
            }
        }
        IpcCommand::Move(tag) => {
            tracing::info!("Moving window to tag {}", tag);
            if let Ok(Some(w)) = aerospace::get_focused_window().await {
                if let Ok(Some(m)) = aerospace::get_focused_monitor().await {
                    let monitor_sync_data;

                    if let Some(monitor) = state.get_monitor_mut(m.monitor_id) {
                        for t in &mut monitor.tags {
                            t.window_ids.retain(|&id| id != w.window_id);
                        }
                        if (tag as usize) < monitor.tags.len() {
                            monitor.tags[tag as usize].window_ids.push(w.window_id);
                        }
                        monitor_sync_data = Some((
                            monitor.tags.clone(),
                            monitor.selected_tags,
                            monitor.visible_workspace.clone(),
                        ));
                        broadcast_state_change(state, m.monitor_id, event_tx);
                    } else {
                        monitor_sync_data = None;
                    }

                    state
                        .windows
                        .entry(w.window_id)
                        .or_insert(state::WindowInfo {
                            id: w.window_id,
                            app_name: w.app_name,
                            title: w.window_title,
                        });

                    if let Some((tags, selected_tags, visible_workspace)) = monitor_sync_data {
                        sync_monitor_state(
                            &tags,
                            selected_tags,
                            &visible_workspace,
                            &state.hidden_workspace,
                        )
                        .await;
                    }
                }
            }
        }
        IpcCommand::Sync => {
            tracing::info!("Syncing windows");
            let mut changed_monitors = std::collections::HashSet::new();

            if let Ok(windows) = aerospace::list_windows().await {
                tracing::debug!("AeroSpace reported {} windows", windows.len());

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

                        if assigned_monitor_id.is_none() {
                            if let Ok(Some(fm)) = aerospace::get_focused_monitor().await {
                                assigned_monitor_id = Some(fm.monitor_id);
                            }
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
                            tracing::warn!(
                                "Could not determine monitor for window {} (workspace: {:?})",
                                w.window_id,
                                w.workspace
                            );
                        }

                        state.windows.insert(
                            w.window_id,
                            state::WindowInfo {
                                id: w.window_id,
                                app_name: w.app_name,
                                title: w.window_title,
                            },
                        );
                    }
                }
            } else {
                tracing::error!("Failed to list windows from AeroSpace");
            }

            for mid in changed_monitors {
                broadcast_state_change(state, mid, event_tx);
            }

            // 3. Rescue focus from hidden workspace
            if let Ok(Some(ws)) = aerospace::get_focused_workspace().await {
                if ws.workspace.starts_with("h-") {
                    tracing::info!("Detected focus on hidden workspace '{}'.", ws.workspace);

                    if let Ok(Some(fw)) = aerospace::get_focused_window().await {
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

                            let monitor_sync_data;
                            if let Some(monitor) = state.get_monitor_mut(mid) {
                                monitor.select_tag(tag);
                                monitor_sync_data = Some((
                                    monitor.tags.clone(),
                                    monitor.selected_tags,
                                    monitor.visible_workspace.clone(),
                                ));
                                broadcast_state_change(state, mid, event_tx);
                            } else {
                                monitor_sync_data = None;
                            }

                            if let Some((tags, selected_tags, visible_workspace)) =
                                monitor_sync_data
                            {
                                sync_monitor_state(
                                    &tags,
                                    selected_tags,
                                    &visible_workspace,
                                    &state.hidden_workspace,
                                )
                                .await;
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
        }
        IpcCommand::Subscribe => {} // Handled in run_server
    }
}

async fn sync_monitor_state(
    tags: &[state::Tag],
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
                if let Err(e) = aerospace::move_node_to_workspace(window_id, &hidden_ws).await {
                    tracing::error!("Failed to hide window {}: {}", window_id, e);
                }
            }
        }
    }

    // 2. Move windows to VISIBLE next
    for (i, tag) in tags.iter().enumerate() {
        if (selected_tags & (1 << i)) != 0 {
            for &window_id in &tag.window_ids {
                if let Err(e) =
                    aerospace::move_node_to_workspace(window_id, visible_workspace).await
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
    if let Err(e) = aerospace::focus_workspace(visible_workspace).await {
        tracing::error!("Failed to restore focus to {}: {}", visible_workspace, e);
    }
}

async fn send_client_command(cmd: IpcCommand) -> anyhow::Result<()> {
    let socket_path = get_socket_path();
    let mut stream = UnixStream::connect(socket_path).await?;
    let data = serde_json::to_vec(&cmd)?;
    stream.write_all(&data).await?;
    stream.shutdown().await?;
    Ok(())
}
