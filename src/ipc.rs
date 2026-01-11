use crate::aerospace::{AerospaceMonitor, AerospaceWindow, AerospaceWorkspace};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcCommand {
    TagView(u8),
    TagToggle(u8),
    TagLast,
    TagSet(u32, Option<u32>),
    WindowMove(u8),
    WindowToggle(u8),
    WindowMoveMonitor(String),
    WindowSet(u32, Option<u32>),
    Query(QueryTarget),
    Sync,
    Subscribe,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum QueryTarget {
    Window(Option<u32>),
    Monitor(Option<u32>),
    State,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateEvent {
    pub event: String,
    pub monitor_id: u32,
    pub selected_tags: u32,
    pub occupied_tags: u32,
}

// Internal commands for actor model
pub enum InternalCommand {
    TagView(Option<AerospaceMonitor>, u8),
    TagToggle(Option<AerospaceMonitor>, u8),
    TagLast(Option<AerospaceMonitor>),
    TagSet(Option<AerospaceMonitor>, u32, Option<u32>),
    WindowMove(Option<AerospaceWindow>, Option<AerospaceMonitor>, u8),
    WindowToggle(Option<AerospaceWindow>, Option<AerospaceMonitor>, u8),
    WindowMoveMonitor(Option<AerospaceWindow>, Option<AerospaceMonitor>, String),
    WindowSet(
        Option<AerospaceWindow>,
        Option<AerospaceMonitor>,
        u32,
        Option<u32>,
    ),
    Query(
        Option<AerospaceWindow>,
        Option<AerospaceMonitor>,
        QueryTarget,
        tokio::net::unix::OwnedWriteHalf,
    ),
    Sync(
        anyhow::Result<Vec<AerospaceWindow>>,
        anyhow::Result<Vec<AerospaceMonitor>>,
        std::collections::HashMap<u32, String>,
        anyhow::Result<Option<AerospaceWorkspace>>,
        anyhow::Result<Option<AerospaceWindow>>,
        anyhow::Result<Option<AerospaceMonitor>>,
    ),
}

pub enum ManagerMessage {
    Ipc(IpcCommand),
    SubscribeClient(tokio::net::unix::OwnedWriteHalf),
    QueryClient(QueryTarget, tokio::net::unix::OwnedWriteHalf),
    Internal(Box<InternalCommand>),
}
