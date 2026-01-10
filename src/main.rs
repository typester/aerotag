use argh::FromArgs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

mod aerospace;
mod state;

use state::{State, Monitor};

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
    Move(MoveCommand),
    Toggle(ToggleCommand),
    Hook(HookCommand),
    Last(LastCommand),
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

#[derive(Debug, Serialize, Deserialize)]
enum IpcCommand {
    Switch(u8),
    Toggle(u8),
    Move(u8),
    Sync,
    Last,
}

enum ManagerMessage {
    Ipc(IpcCommand),
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
    }
}

async fn run_server() -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel::<ManagerMessage>(100);

    // --- State Manager Actor ---
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
        handle_ipc_command(&mut state, IpcCommand::Sync).await;

        while let Some(msg) = rx.recv().await {
            match msg {
                ManagerMessage::Ipc(cmd) => handle_ipc_command(&mut state, cmd).await,
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
            Ok((mut stream, _)) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    if let Ok(_) = stream.read_to_end(&mut buf).await {
                        if let Ok(cmd) = serde_json::from_slice::<IpcCommand>(&buf) {
                            let _ = tx.send(ManagerMessage::Ipc(cmd)).await;
                        }
                    }
                });
            }
            Err(e) => tracing::error!("Accept error: {}", e),
        }
    }
}

async fn handle_ipc_command(state: &mut State, cmd: IpcCommand) {
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
                    }
                     else {
                        monitor_sync_data = None;
                    }

                    state.windows.entry(w.window_id).or_insert(state::WindowInfo {
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
                        for t in &mut m.tags {
                            t.window_ids.retain(|&wid| wid != id);
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
                if let Err(e) = aerospace::move_node_to_workspace(window_id, visible_workspace).await
                {
                    tracing::error!("Failed to show window {}: {}", window_id, e);
                }
            }
        }
    }

    // Restore focus to the visible workspace to prevent getting stuck in hidden workspace
    tracing::debug!("Restoring focus to visible workspace: {}", visible_workspace);
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