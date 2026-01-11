use argh::FromArgs;
use serde::{Deserialize, Serialize};

#[derive(Debug, FromArgs)]
/// Tag based workspace manager for AeroSpace
pub struct Args {
    #[argh(subcommand)]
    pub cmd: SubCommand,
}

#[derive(Debug, FromArgs)]
#[argh(subcommand)]
pub enum SubCommand {
    Server(ServerCommand),
    TagView(TagViewCommand),
    TagToggle(TagToggleCommand),
    TagLast(TagLastCommand),
    TagSet(TagSetCommand),
    WindowMove(WindowMoveCommand),
    WindowToggle(WindowToggleCommand),
    WindowMoveMonitor(WindowMoveMonitorCommand),
    WindowSet(WindowSetCommand),
    Query(QueryCommand),
    Version(VersionCommand),
    Hook(HookCommand),
    Subscribe(SubscribeCommand),
}

#[derive(Debug, FromArgs)]
/// Launch workspace manager daemon
#[argh(subcommand, name = "server")]
pub struct ServerCommand {}

#[derive(Debug, FromArgs)]
/// Show version info
#[argh(subcommand, name = "version")]
pub struct VersionCommand {}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Switch to a specific tag on the focused monitor
#[argh(subcommand, name = "tag-view")]
pub struct TagViewCommand {
    #[argh(positional)]
    pub tag: u8,
}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Toggle a specific tag on the focused monitor
#[argh(subcommand, name = "tag-toggle")]
pub struct TagToggleCommand {
    #[argh(positional)]
    pub tag: u8,
}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Restore the last selected tags on the focused monitor
#[argh(subcommand, name = "tag-last")]
pub struct TagLastCommand {}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Set monitor tags to a specific bitmask [Primitive]
#[argh(subcommand, name = "tag-set")]
pub struct TagSetCommand {
    #[argh(positional)]
    pub mask: u32,
    #[argh(option)]
    /// monitor id (optional)
    pub monitor_id: Option<u32>,
}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Move focused window to a specific tag
#[argh(subcommand, name = "window-move")]
pub struct WindowMoveCommand {
    #[argh(positional)]
    pub tag: u8,
}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Toggle focused window to a specific tag
#[argh(subcommand, name = "window-toggle")]
pub struct WindowToggleCommand {
    #[argh(positional)]
    pub tag: u8,
}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Move focused window to next/prev monitor
#[argh(subcommand, name = "window-move-monitor")]
pub struct WindowMoveMonitorCommand {
    #[argh(positional)]
    pub target: String,
}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Set window tags to a specific bitmask [Primitive]
#[argh(subcommand, name = "window-set")]
pub struct WindowSetCommand {
    #[argh(positional)]
    pub mask: u32,
    #[argh(option)]
    /// window id (optional)
    pub window_id: Option<u32>,
}

#[derive(Debug, FromArgs)]
/// Query state information [Primitive]
#[argh(subcommand, name = "query")]
pub struct QueryCommand {
    #[argh(subcommand)]
    pub cmd: QuerySubCommand,
}

#[derive(Debug, FromArgs)]
#[argh(subcommand)]
pub enum QuerySubCommand {
    Window(QueryWindowCommand),
    Monitor(QueryMonitorCommand),
    State(QueryStateCommand),
}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Query window information
#[argh(subcommand, name = "window")]
pub struct QueryWindowCommand {
    #[argh(positional)]
    /// window id (optional)
    pub window_id: Option<u32>,
}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Query monitor information
#[argh(subcommand, name = "monitor")]
pub struct QueryMonitorCommand {
    #[argh(positional)]
    /// monitor id (optional)
    pub monitor_id: Option<u32>,
}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Query full state
#[argh(subcommand, name = "state")]
pub struct QueryStateCommand {}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Trigger synchronization (e.g., from AeroSpace exec-on-workspace-change)
#[argh(subcommand, name = "hook")]
pub struct HookCommand {}

#[derive(Debug, FromArgs, Serialize, Deserialize)]
/// Subscribe to state change events
#[argh(subcommand, name = "subscribe")]
pub struct SubscribeCommand {}
