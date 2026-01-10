# AeroTag

**Tag-based workspace management for [AeroSpace](https://github.com/nikitabobko/AeroSpace).**

`aerotag` brings the power of dynamic tagging (inspired by AwesomeWM, dwm, etc.) to macOS. Instead of rigid workspaces, you assign tags to windows and dynamically view any combination of them.

## Overview

AeroSpace is a fantastic tiling window manager for macOS, but it uses a traditional workspace model (like i3). `aerotag` sits on top of AeroSpace and overrides its workspace management with a logic layer of **Tags**.

- **Dynamic Tagging:** Assign multiple tags to a window. View multiple tags simultaneously.
- **AwesomeWM Style:** Windows are shown or hidden based on currently selected tags.
- **Flicker-free Experience:** Uses a unique "per-window hidden workspace" strategy to minimize visual glitches during context switching.
- **State Persistence:** Server/Client architecture maintains the state of your tags and windows.

## Architecture

`aerotag` operates in two modes:

1.  **Server:** Runs in the background, maintaining the state (which window has which tag) in memory. It communicates with AeroSpace CLI to move windows into a single "Visible Workspace" per monitor or hide them in "Hidden Workspaces".
2.  **Client:** Sends commands (`switch`, `move`, `toggle`, etc.) to the server via Unix Domain Socket.

### Workspace Mapping
`aerotag` simplifies AeroSpace's workspace usage:
- **Visible Workspace:** Uses workspace names `1`, `2`, `3`... corresponding to Monitor IDs. (e.g., Monitor 1 always shows Workspace "1").
- **Hidden Workspaces:** Windows not currently in view are moved to `h-<window_id>` workspaces.

## Installation

```bash
cargo install --path .
```

## Configuration

Add the following to your `aerospace.toml`.

### Environment Setup
Ensure `aerotag` is in your `$PATH`.

```toml
[exec.env-vars]
PATH = '${HOME}/.cargo/bin:${PATH}'
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
# Switch to Tag (0-9)
alt-1 = 'exec-and-forget aerotag switch 0'
alt-2 = 'exec-and-forget aerotag switch 1'
# ...
alt-0 = 'exec-and-forget aerotag switch 9'

# Toggle Tag (Multi-tag view)
ctrl-alt-1 = 'exec-and-forget aerotag toggle 0'
# ...

# Move Window to Tag
alt-shift-1 = 'exec-and-forget aerotag move 0'
# ...

# History (Back-and-forth)
alt-x = 'exec-and-forget aerotag last'

# Move Window to Next/Prev Monitor (AwesomeWM style: moves to active tag on target monitor)
alt-ctrl-o = 'exec-and-forget aerotag move-monitor next'
```

## Commands

- `server`: Starts the daemon.
- `switch <tag_index>`: Exclusive switch to a tag (0-31).
- `toggle <tag_index>`: Toggle a tag's visibility (view multiple tags).
- `move <tag_index>`: Move the focused window to a tag.
- `last`: Restore the previous tag selection (history).
- `move-monitor <next|prev>`: Move the focused window to the next/previous monitor's currently active tag.
- `subscribe`: Stream state change events as JSON.
- `hook`: Trigger state synchronization (used internally by `exec-on-workspace-change`).

## External Integrations

`aerotag` can stream its state to external tools like **SketchyBar**.

### Event Stream

Running `aerotag subscribe` will output a JSON stream of state changes:

```json
{"event":"state_change","monitor_id":1,"selected_tags":1,"occupied_tags":5}
```

- `selected_tags`: Bitmask of currently active tags.
- `occupied_tags`: Bitmask of tags that contain at least one window.

You can use this to build a dynamic tag bar that highlights active and occupied tags in real-time.

## License

MIT
