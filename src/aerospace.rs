use serde::Deserialize;
use tokio::process::Command;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct AerospaceMonitor {
    pub monitor_id: u32,
    pub monitor_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct AerospaceWindow {
    pub window_id: u32,
    pub app_name: String,
    pub window_title: String,
    pub workspace: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct AerospaceWorkspace {
    pub workspace: String,
    #[serde(default)]
    pub monitor: Option<String>,
}

pub async fn list_monitors() -> anyhow::Result<Vec<AerospaceMonitor>> {
    let output = run_command(&["list-monitors", "--json"]).await?;
    let monitors: Vec<AerospaceMonitor> = serde_json::from_str(&output)?;
    Ok(monitors)
}

pub async fn list_windows() -> anyhow::Result<Vec<AerospaceWindow>> {
    let output = run_command(&["list-windows", "--all", "--json"]).await?;
    let windows: Vec<AerospaceWindow> = serde_json::from_str(&output)?;
    Ok(windows)
}

pub async fn list_workspaces() -> anyhow::Result<Vec<AerospaceWorkspace>> {
    let output = run_command(&["list-workspaces", "--all", "--json"]).await?;
    let workspaces: Vec<AerospaceWorkspace> = serde_json::from_str(&output)?;
    Ok(workspaces)
}

pub async fn get_focused_monitor() -> anyhow::Result<Option<AerospaceMonitor>> {
    let output = run_command(&["list-monitors", "--focused", "--json"]).await?;
    let monitors: Vec<AerospaceMonitor> = serde_json::from_str(&output)?;
    Ok(monitors.into_iter().next())
}

pub async fn get_focused_window() -> anyhow::Result<Option<AerospaceWindow>> {
    let output = run_command(&["list-windows", "--focused", "--json"]).await?;
    let windows: Vec<AerospaceWindow> = serde_json::from_str(&output)?;
    Ok(windows.into_iter().next())
}

pub async fn get_focused_workspace() -> anyhow::Result<Option<AerospaceWorkspace>> {
    let output = run_command(&["list-workspaces", "--focused", "--json"]).await?;
    let workspaces: Vec<AerospaceWorkspace> = serde_json::from_str(&output)?;
    Ok(workspaces.into_iter().next())
}

pub async fn move_node_to_workspace(window_id: u32, workspace: &str) -> anyhow::Result<()> {
    run_command(&[
        "move-node-to-workspace",
        "--window-id",
        &window_id.to_string(),
        workspace,
    ])
    .await?;
    Ok(())
}

pub async fn focus_workspace(workspace: &str) -> anyhow::Result<()> {
    run_command(&["workspace", workspace]).await?;
    Ok(())
}

pub async fn focus_window(window_id: u32) -> anyhow::Result<()> {
    run_command(&["focus", "--window-id", &window_id.to_string()]).await?;
    Ok(())
}

async fn run_command(args: &[&str]) -> anyhow::Result<String> {
    tracing::debug!("Running command: aerospace {}", args.join(" "));

    let output = Command::new("aerospace")
        .args(args)
        .output()
        .await?;

    if output.status.success() {
        let stdout = String::from_utf8(output.stdout)?;
        tracing::debug!("Command success. Output: {}", stdout.trim());
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!(
            "Command failed: aerospace {}\nError: {}",
            args.join(" "),
            stderr
        );
        anyhow::bail!("Aerospace command failed: {}", stderr);
    }
}
