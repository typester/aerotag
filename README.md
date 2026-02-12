# AeroTag

> **Note:** This project is no longer actively developed. Development has moved to [Yashiki](https://github.com/typester/yashiki).

**Tag-based workspace management for [AeroSpace](https://github.com/nikitabobko/AeroSpace).**

`aerotag` brings the power of dynamic tagging (inspired by [AwesomeWM](https://awesomewm.org/), [River](https://awesomewm.org/), etc.) to macOS. Instead of rigid workspaces, you assign tags to windows and dynamically view any combination of them.

## Overview

AeroSpace is a fantastic tiling window manager for macOS, but it uses a traditional workspace model (like i3). `aerotag` sits on top of AeroSpace and overrides its workspace management with a logic layer of **Tags**.

- **Dynamic Tagging:** Assign multiple tags to a window. View multiple tags simultaneously.
- **AwesomeWM Style:** Windows are shown or hidden based on currently selected tags.
- **Flicker-free Experience:** Uses a unique "per-window hidden workspace" strategy to minimize visual glitches during context switching.
- **State Persistence:** Server/Client architecture maintains the state of your tags and windows.

## Architecture

`aerotag` operates in two modes:

1.  **Server:** Runs in the background, maintaining the state (which window has which tag) in memory. It communicates with AeroSpace CLI to move windows into a single "Visible Workspace" per monitor or hide them in "Hidden Workspaces".
2.  **Client:** Sends commands (`tag-view`, `window-move`, etc.) to the server via Unix Domain Socket.

### Workspace Mapping
`aerotag` simplifies AeroSpace's workspace usage:
- **Visible Workspace:** Uses workspace names `1`, `2`, `3`... corresponding to Monitor IDs. (e.g., Monitor 1 always shows Workspace "1").
- **Hidden Workspaces:** Windows not currently in view are moved to `h-<window_id>` workspaces.

## Installation

```bash
cargo install aerotag
```

## Configuration

Add the following to your `aerospace.toml`.

### Environment Setup
Ensure both `aerotag` and `aerospace` commands are in your `$PATH`. This is required for `aerotag` to communicate with AeroSpace when started automatically.

```toml
[exec.env-vars]
# Ensure aerotag (in .cargo/bin) and aerospace (in /opt/homebrew/bin) are in PATH
PATH = '/opt/homebrew/bin:${HOME}/.cargo/bin:${PATH}'
```

### Hooks & Startup

```toml
# Start the server on login
after-startup-command = [
  'exec-and-forget aerotag server'
]

# Sync state when workspace changes (e.g. clicking dock icons, monitor changes)
exec-on-workspace-change = ['/bin/bash', '-c', 'aerotag hook']

# Sync state when new windows are detected
[[on-window-detected]]
run = ['exec-and-forget aerotag hook']
```

### Keybindings

```toml
[mode.main.binding]
# View Tag (Switch to 1-9)
alt-1 = 'exec-and-forget aerotag tag-view 1'
alt-2 = 'exec-and-forget aerotag tag-view 2'
# ...
alt-0 = 'exec-and-forget aerotag tag-view 10'

# Toggle Tag Visibility (Multi-tag view)
ctrl-alt-1 = 'exec-and-forget aerotag tag-toggle 1'
# ...

# Move Window to Tag
alt-shift-1 = 'exec-and-forget aerotag window-move 1'
# ...

# Toggle Window Tag (Assign window to multiple tags)
alt-shift-ctrl-1 = 'exec-and-forget aerotag window-toggle 1'

# History (Back-and-forth)
alt-x = 'exec-and-forget aerotag tag-last'

# Move Window to Next/Prev Monitor (AwesomeWM style: moves to active tag on target monitor)
alt-ctrl-o = 'exec-and-forget aerotag window-move-monitor next'
```

## Commands

### Tag Operations
- `tag-view <index>`: Exclusive switch to a tag (1-32).
- `tag-toggle <index>`: Toggle a tag's visibility (view multiple tags).
- `tag-last`: Restore the previous tag selection (history).

### Window Operations
- `window-move <index>`: Move the focused window to a tag (remove from others).
- `window-toggle <index>`: Toggle the focused window's membership in a tag.
  - *Safety Guard:* Will not remove the tag if it's the window's only tag.
- `window-move-monitor <next|prev>`: Move the focused window to the next/previous monitor.

### Primitive API (For Advanced Scripting)
`aerotag` provides low-level commands to query and manipulate state via bitmasks, useful for building custom scripts.

- `query window [window_id]`: Get window state (tags, monitor) as JSON.
- `query monitor [monitor_id]`: Get monitor state (selected tags, occupied tags) as JSON.
- `query state`: Get full system state as JSON.
- `window-set <mask> [--window-id <id>]`: Set window tags directly using a bitmask.
- `tag-set <mask> [--monitor-id <id>]`: Set monitor visible tags directly using a bitmask.

### System
- `server`: Starts the daemon.
- `subscribe`: Stream state change events as JSON.
- `hook`: Trigger state synchronization (used internally).

## External Integrations

`aerotag` can stream its state to external tools like **SketchyBar**.

### Event Stream

Running `aerotag subscribe` will output a JSON stream of state changes:

```json
{"event":"state_change","monitor_id":1,"selected_tags":1,"occupied_tags":5}
```

- `selected_tags`: Bitmask of currently active tags.
- `occupied_tags`: Bitmask of tags that contain at least one window.

## License

MIT
