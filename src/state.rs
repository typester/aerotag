use std::collections::HashMap;

pub type WindowId = u32;
pub type MonitorId = u32;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct WindowInfo {
    pub id: WindowId,
    pub app_name: String,
    pub title: String,
}

#[derive(Debug, Default, Clone)]
pub struct Tag {
    pub window_ids: Vec<WindowId>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Monitor {
    pub id: MonitorId,
    pub name: String,
    pub selected_tags: u32,
    pub previous_tags: u32,
    pub tags: Vec<Tag>,
    pub visible_workspace: String,
}

impl Monitor {
    pub fn new(id: MonitorId, name: String, visible_workspace: String) -> Self {
        Self {
            id,
            name,
            selected_tags: 1,
            previous_tags: 1,
            tags: vec![Tag::default(); 32],
            visible_workspace,
        }
    }

    pub fn select_tag(&mut self, tag_idx: u8) {
        if tag_idx >= 32 {
            return;
        }
        let new_selection = 1 << tag_idx;
        if self.selected_tags != new_selection {
            self.previous_tags = self.selected_tags;
            self.selected_tags = new_selection;
        }
    }

    pub fn toggle_tag(&mut self, tag_idx: u8) {
        if tag_idx >= 32 {
            return;
        }
        self.previous_tags = self.selected_tags;
        self.selected_tags ^= 1 << tag_idx;
    }

    pub fn restore_last_tags(&mut self) {
        let temp = self.selected_tags;
        self.selected_tags = self.previous_tags;
        self.previous_tags = temp;
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct State {
    pub windows: HashMap<WindowId, WindowInfo>,
    pub monitors: HashMap<MonitorId, Monitor>,
    pub hidden_workspace: String,
    pub focused_monitor_id: Option<MonitorId>,
}

impl State {
    pub fn new() -> Self {
        Self {
            windows: HashMap::new(),
            monitors: HashMap::new(),
            hidden_workspace: "h".to_string(), // TODO: Configurable?
            focused_monitor_id: None,
        }
    }

    pub fn get_monitor_mut(&mut self, id: MonitorId) -> Option<&mut Monitor> {
        self.monitors.get_mut(&id)
    }

    pub fn assign_window(&mut self, window_id: WindowId, tag_idx: u8, monitor_id: MonitorId) {
        if let Some(monitor) = self.monitors.get_mut(&monitor_id) {
            if (tag_idx as usize) < monitor.tags.len() {
                monitor.tags[tag_idx as usize].window_ids.push(window_id);
            }
        }
    }

    pub fn find_monitor_by_window(&self, window_id: WindowId) -> Option<MonitorId> {
        for monitor in self.monitors.values() {
            for tag in &monitor.tags {
                if tag.window_ids.contains(&window_id) {
                    return Some(monitor.id);
                }
            }
        }
        None
    }
}
