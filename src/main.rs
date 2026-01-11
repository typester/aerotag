use argh::FromArgs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

mod aerospace;
mod state;

use aerospace::{AerospaceMonitor, AerospaceWindow, AerospaceWorkspace};
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
    Set(SetCommand),
    Copy(CopyCommand),
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
/// Set focused window tags to a specific bitmask
#[argh(subcommand, name = "set")]
struct SetCommand {
    #[argh(positional)]
    mask: u32,
}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Copy focused window to a specific tag
#[argh(subcommand, name = "copy")]
struct CopyCommand {
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
    Set(u32),
    Copy(u8),
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

// Internal commands for actor model
enum InternalCommand {
    HandleSwitch(Option<AerospaceMonitor>, u8),
    HandleToggle(Option<AerospaceMonitor>, u8),
    HandleLast(Option<AerospaceMonitor>),
    HandleMove(Option<AerospaceWindow>, Option<AerospaceMonitor>, u8),
    HandleSet(Option<AerospaceWindow>, Option<AerospaceMonitor>, u32),
    HandleCopy(Option<AerospaceWindow>, Option<AerospaceMonitor>, u8),
    HandleMoveMonitor(Option<AerospaceWindow>, Option<AerospaceMonitor>, String),
    HandleSync(
        anyhow::Result<Vec<AerospaceWindow>>,
        anyhow::Result<Vec<AerospaceMonitor>>,
        std::collections::HashMap<u32, String>,
        anyhow::Result<Option<AerospaceWorkspace>>,
        anyhow::Result<Option<AerospaceWindow>>,
        anyhow::Result<Option<AerospaceMonitor>>,
    ),
}

enum ManagerMessage {
    Ipc(IpcCommand),
    SubscribeClient(tokio::net::unix::OwnedWriteHalf),
    Internal(InternalCommand),
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
        SubCommand::Switch(cmd) => {
            if cmd.tag == 0 {
                anyhow::bail!("Tag index must be 1-based");
            }
            send_client_command(IpcCommand::Switch(cmd.tag - 1)).await
        }
        SubCommand::Toggle(cmd) => {
            if cmd.tag == 0 {
                anyhow::bail!("Tag index must be 1-based");
            }
            send_client_command(IpcCommand::Toggle(cmd.tag - 1)).await
        }
        SubCommand::Move(cmd) => {
            if cmd.tag == 0 {
                anyhow::bail!("Tag index must be 1-based");
            }
            send_client_command(IpcCommand::Move(cmd.tag - 1)).await
        }
        SubCommand::Set(cmd) => send_client_command(IpcCommand::Set(cmd.mask)).await,
        SubCommand::Copy(cmd) => {
            if cmd.tag == 0 {
                anyhow::bail!("Tag index must be 1-based");
            }
            send_client_command(IpcCommand::Copy(cmd.tag - 1)).await
        }
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
    let actor_tx = tx.clone(); // Clone for internal messages
    tokio::spawn(async move {
        if let Ok(monitors) = aerospace::list_monitors().await {
            initialize_all_monitors(&monitors).await;
        }

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

        // Initial window population. Send Sync command to self.
        let _ = actor_tx.send(ManagerMessage::Ipc(IpcCommand::Sync)).await;

        while let Some(msg) = rx.recv().await {
            match msg {
                ManagerMessage::Ipc(cmd) => {
                    handle_ipc_command_async(cmd, actor_tx.clone());
                }
                ManagerMessage::Internal(cmd) => {
                    handle_internal_command(&mut state, cmd, &event_tx_clone);
                }
                ManagerMessage::SubscribeClient(mut stream_tx) => {
                    let mut event_rx = event_tx_clone.subscribe();

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

// Spawns tasks to fetch external state, then sends InternalCommand back to Actor
fn handle_ipc_command_async(cmd: IpcCommand, tx: mpsc::Sender<ManagerMessage>) {
    tokio::spawn(async move {
        match cmd {
            IpcCommand::Switch(tag) => {
                let m = aerospace::get_focused_monitor().await.ok().flatten();
                let _ = tx
                    .send(ManagerMessage::Internal(InternalCommand::HandleSwitch(
                        m, tag,
                    )))
                    .await;
            }
            IpcCommand::Toggle(tag) => {
                let m = aerospace::get_focused_monitor().await.ok().flatten();
                let _ = tx
                    .send(ManagerMessage::Internal(InternalCommand::HandleToggle(
                        m, tag,
                    )))
                    .await;
            }
            IpcCommand::Last => {
                let m = aerospace::get_focused_monitor().await.ok().flatten();
                let _ = tx
                    .send(ManagerMessage::Internal(InternalCommand::HandleLast(m)))
                    .await;
            }
            IpcCommand::Move(tag) => {
                let w = aerospace::get_focused_window().await.ok().flatten();
                let m = aerospace::get_focused_monitor().await.ok().flatten();
                let _ = tx
                    .send(ManagerMessage::Internal(InternalCommand::HandleMove(
                        w, m, tag,
                    )))
                    .await;
            }
            IpcCommand::Set(mask) => {
                let w = aerospace::get_focused_window().await.ok().flatten();
                let m = aerospace::get_focused_monitor().await.ok().flatten();
                let _ = tx
                    .send(ManagerMessage::Internal(InternalCommand::HandleSet(
                        w, m, mask,
                    )))
                    .await;
            }
            IpcCommand::Copy(tag) => {
                let w = aerospace::get_focused_window().await.ok().flatten();
                let m = aerospace::get_focused_monitor().await.ok().flatten();
                let _ = tx
                    .send(ManagerMessage::Internal(InternalCommand::HandleCopy(
                        w, m, tag,
                    )))
                    .await;
            }
            IpcCommand::MoveMonitor(target) => {
                let w = aerospace::get_focused_window().await.ok().flatten();
                let m = aerospace::get_focused_monitor().await.ok().flatten();
                let _ = tx
                    .send(ManagerMessage::Internal(
                        InternalCommand::HandleMoveMonitor(w, m, target),
                    ))
                    .await;
            }
            IpcCommand::Sync => {
                // Fetch all needed info in parallel
                let windows = aerospace::list_windows().await;
                let monitors_result = aerospace::list_monitors().await;

                let mut visible_workspaces = std::collections::HashMap::new();
                if let Ok(ref monitors) = monitors_result {
                    for m in monitors {
                        if let Ok(ws) = aerospace::get_visible_workspace(m.monitor_id).await {
                            visible_workspaces.insert(m.monitor_id, ws);
                        }
                    }
                }

                let fw_workspace = aerospace::get_focused_workspace().await;
                let fw_window = aerospace::get_focused_window().await;
                let fw_monitor = aerospace::get_focused_monitor().await;

                let _ = tx
                    .send(ManagerMessage::Internal(InternalCommand::HandleSync(
                        windows,
                        monitors_result,
                        visible_workspaces,
                        fw_workspace,
                        fw_window,
                        fw_monitor,
                    )))
                    .await;
            }
            IpcCommand::Subscribe => {} // Already handled
        }
    });
}
fn handle_internal_command(
    state: &mut State,
    cmd: InternalCommand,
    event_tx: &tokio::sync::broadcast::Sender<StateEvent>,
) {
    match cmd {
        InternalCommand::HandleSwitch(Some(m), tag) => {
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
                tokio::spawn(async move {
                    sync_monitor_state(&tags, selected_tags, &visible_workspace, &hidden_workspace)
                        .await;
                });
            } else {
                tracing::warn!("Monitor {} not found in state", m.monitor_id);
            }
        }
        InternalCommand::HandleSwitch(None, _) => tracing::warn!("No focused monitor found"),

        InternalCommand::HandleToggle(Some(m), tag) => {
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
                tokio::spawn(async move {
                    sync_monitor_state(&tags, selected_tags, &visible_workspace, &hidden_workspace)
                        .await;
                });
            }
        }
        InternalCommand::HandleToggle(None, _) => tracing::warn!("No focused monitor found"),

        InternalCommand::HandleLast(Some(m)) => {
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
                tokio::spawn(async move {
                    sync_monitor_state(&tags, selected_tags, &visible_workspace, &hidden_workspace)
                        .await;
                });
            }
        }
        InternalCommand::HandleLast(None) => tracing::warn!("No focused monitor found"),

        InternalCommand::HandleMove(Some(w), focused_monitor, tag) => {
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

                    state
                        .windows
                        .entry(w.window_id)
                        .or_insert(state::WindowInfo {
                            id: w.window_id,
                            app_name: w.app_name,
                            title: w.window_title,
                        });

                    tokio::spawn(async move {
                        sync_monitor_state(
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
        InternalCommand::HandleMove(None, _, _) => tracing::warn!("No focused window found"),

        InternalCommand::HandleSet(Some(w), focused_monitor, mask) => {
            tracing::info!("Setting window tags to mask {:b}", mask);
            let mut target_monitor_id = None;

            if let Some(mid) = state.find_monitor_by_window(w.window_id) {
                target_monitor_id = Some(mid);
            } else if let Some(m) = focused_monitor {
                target_monitor_id = Some(m.monitor_id);
            }

            if let Some(mid) = target_monitor_id {
                let mut sync_data = None;
                if let Some(monitor) = state.get_monitor_mut(mid) {
                    // Remove from all tags
                    for t in &mut monitor.tags {
                        t.window_ids.retain(|&id| id != w.window_id);
                    }
                    // Add to tags specified by mask
                    for i in 0..32 {
                        if (mask & (1 << i)) != 0 {
                            if (i as usize) < monitor.tags.len() {
                                monitor.tags[i as usize].window_ids.push(w.window_id);
                            }
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

                    state
                        .windows
                        .entry(w.window_id)
                        .or_insert(state::WindowInfo {
                            id: w.window_id,
                            app_name: w.app_name,
                            title: w.window_title,
                        });

                    tokio::spawn(async move {
                        sync_monitor_state(
                            &tags,
                            selected_tags,
                            &visible_workspace,
                            &hidden_workspace,
                        )
                        .await;
                    });
                }
            } else {
                tracing::warn!("No monitor found for window set");
            }
        }
        InternalCommand::HandleSet(None, _, _) => tracing::warn!("No focused window found"),

        InternalCommand::HandleCopy(Some(w), focused_monitor, tag) => {
            tracing::info!("Adding window to tag {}", tag);
            let mut target_monitor_id = None;

            if let Some(mid) = state.find_monitor_by_window(w.window_id) {
                target_monitor_id = Some(mid);
            } else if let Some(m) = focused_monitor {
                target_monitor_id = Some(m.monitor_id);
            }

            if let Some(mid) = target_monitor_id {
                let mut sync_data = None;
                if let Some(monitor) = state.get_monitor_mut(mid) {
                    if (tag as usize) < monitor.tags.len() {
                        if !monitor.tags[tag as usize].window_ids.contains(&w.window_id) {
                            monitor.tags[tag as usize].window_ids.push(w.window_id);
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

                    state
                        .windows
                        .entry(w.window_id)
                        .or_insert(state::WindowInfo {
                            id: w.window_id,
                            app_name: w.app_name,
                            title: w.window_title,
                        });

                    tokio::spawn(async move {
                        sync_monitor_state(
                            &tags,
                            selected_tags,
                            &visible_workspace,
                            &hidden_workspace,
                        )
                        .await;
                    });
                }
            } else {
                tracing::warn!("No monitor found for window copy");
            }
        }
        InternalCommand::HandleCopy(None, _, _) => tracing::warn!("No focused window found"),

        InternalCommand::HandleMoveMonitor(Some(w), Some(current_monitor), target) => {
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
                        tokio::spawn(async move {
                            if let Err(e) =
                                aerospace::move_node_to_workspace(window_id, &target_workspace)
                                    .await
                            {
                                tracing::error!(
                                    "Failed to move window to monitor {}: {}",
                                    next_monitor_id,
                                    e
                                );
                            }
                            let _ = aerospace::focus_window(window_id).await;
                        });
                    }
                }
            }
        }
        InternalCommand::HandleMoveMonitor(None, _, _) => tracing::warn!("No focused window found"),
        InternalCommand::HandleMoveMonitor(_, None, _) => {
            tracing::warn!("No focused monitor found")
        }

        InternalCommand::HandleSync(
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
                tokio::spawn(async move {
                    initialize_all_monitors(&monitors_clone).await;
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

                    if !rescued_windows.is_empty() {
                        if let Some(rescue_monitor) = state.get_monitor_mut(rescue_id) {
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

                            tokio::spawn(async move {
                                sync_monitor_state(
                                    &tags,
                                    selected_tags,
                                    &visible_workspace,
                                    &hidden_workspace,
                                )
                                .await;
                            });
                        }
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
                        state::WindowInfo {
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
            if let Ok(Some(ws)) = fw_workspace {
                if ws.workspace.starts_with("h-") {
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

                                tokio::spawn(async move {
                                    sync_monitor_state(
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
                            if let Ok(Some(current_monitor)) = &fw_monitor {
                                if current_monitor.monitor_id != mid {
                                    if let Some(wrong_monitor) =
                                        state.monitors.get(&current_monitor.monitor_id)
                                    {
                                        let restore_ws = wrong_monitor.visible_workspace.clone();
                                        tracing::info!(
                                            "Restoring monitor {} from hidden workspace to {}",
                                            current_monitor.monitor_id,
                                            restore_ws
                                        );
                                        tokio::spawn(async move {
                                            // Wait a bit to let the primary sync happen first?
                                            // Or just fire it. AeroSpace queues commands usually.
                                            let _ = aerospace::focus_workspace(&restore_ws).await;
                                        });
                                    }
                                }
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
        InternalCommand::HandleSync(Err(e), _, _, _, _, _) => {
            tracing::error!("Failed to list windows from AeroSpace: {}", e);
        }
        InternalCommand::HandleSync(_, Err(e), _, _, _, _) => {
            tracing::error!("Failed to list monitors from AeroSpace: {}", e);
        }
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

async fn initialize_all_monitors(monitors: &[AerospaceMonitor]) {
    if monitors.is_empty() {
        return;
    }

    let mut sorted: Vec<_> = monitors.to_vec();
    sorted.sort_by_key(|m| m.monitor_id);

    let mut current_state: std::collections::HashMap<u32, String> =
        std::collections::HashMap::new();
    for m in &sorted {
        if let Ok(ws) = aerospace::get_visible_workspace(m.monitor_id).await {
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
        "Realigning workspaces: current={:?}, expected={:?}",
        current_state,
        sorted.iter().map(|m| (m.monitor_id, m.monitor_id.to_string())).collect::<Vec<_>>()
    );

    // Ensure all required workspaces exist by focusing them once
    for m in &sorted {
        let ws_name = m.monitor_id.to_string();
        let _ = aerospace::focus_workspace(&ws_name).await;
    }

    // Explicitly move each workspace to its correct monitor
    for m in &sorted {
        let ws_name = m.monitor_id.to_string();
        tracing::debug!("Moving workspace {} to monitor {}", ws_name, m.monitor_id);
        let _ = aerospace::move_workspace_to_monitor(&ws_name, m.monitor_id).await;
    }

    // Focus each monitor on its correct workspace
    for m in &sorted {
        let ws_name = m.monitor_id.to_string();
        let _ = aerospace::focus_monitor(m.monitor_id).await;
        let _ = aerospace::focus_workspace(&ws_name).await;
    }

    // Focus the primary monitor (lowest ID) so user sees the change
    if let Some(primary) = sorted.first() {
        tracing::info!("Focusing primary monitor {}", primary.monitor_id);
        let _ = aerospace::focus_monitor(primary.monitor_id).await;
        let _ = aerospace::focus_workspace(&primary.monitor_id.to_string()).await;
    }

    tracing::info!("Workspace realignment complete");
}

async fn send_client_command(cmd: IpcCommand) -> anyhow::Result<()> {
    let socket_path = get_socket_path();
    let mut stream = UnixStream::connect(socket_path).await?;
    let data = serde_json::to_vec(&cmd)?;
    stream.write_all(&data).await?;
    stream.shutdown().await?;
    Ok(())
}
