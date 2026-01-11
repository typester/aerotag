use async_trait::async_trait;
use serde::Deserialize;
use tokio::process::Command;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct AerospaceMonitor {
    pub monitor_id: u32,
    pub monitor_name: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct AerospaceWindow {
    pub window_id: u32,
    pub app_name: String,
    pub window_title: String,
    pub workspace: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
pub struct AerospaceWorkspace {
    pub workspace: String,
    #[serde(default)]
    pub monitor: Option<String>,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait AerospaceClient: Send + Sync {
    async fn list_monitors(&self) -> anyhow::Result<Vec<AerospaceMonitor>>;
    async fn list_windows(&self) -> anyhow::Result<Vec<AerospaceWindow>>;
    async fn list_workspaces(&self) -> anyhow::Result<Vec<AerospaceWorkspace>>;
    async fn get_focused_monitor(&self) -> anyhow::Result<Option<AerospaceMonitor>>;
    async fn get_focused_window(&self) -> anyhow::Result<Option<AerospaceWindow>>;
    async fn get_focused_workspace(&self) -> anyhow::Result<Option<AerospaceWorkspace>>;
    async fn get_visible_workspace(&self, monitor_id: u32) -> anyhow::Result<String>;
    async fn move_node_to_workspace(&self, window_id: u32, workspace: &str) -> anyhow::Result<()>;
    async fn focus_workspace(&self, workspace: &str) -> anyhow::Result<()>;
    async fn focus_window(&self, window_id: u32) -> anyhow::Result<()>;
    async fn focus_monitor(&self, monitor_id: u32) -> anyhow::Result<()>;
    async fn move_workspace_to_monitor(
        &self,
        workspace: &str,
        monitor_id: u32,
    ) -> anyhow::Result<()>;
}

pub struct RealClient;

#[async_trait]
impl AerospaceClient for RealClient {
    async fn list_monitors(&self) -> anyhow::Result<Vec<AerospaceMonitor>> {
        let output = run_command(&["list-monitors", "--json"]).await?;
        let monitors: Vec<AerospaceMonitor> = serde_json::from_str(&output)?;
        Ok(monitors)
    }

    async fn list_windows(&self) -> anyhow::Result<Vec<AerospaceWindow>> {
        let output = run_command(&["list-windows", "--all", "--json"]).await?;
        let windows: Vec<AerospaceWindow> = serde_json::from_str(&output)?;
        Ok(windows)
    }

    async fn list_workspaces(&self) -> anyhow::Result<Vec<AerospaceWorkspace>> {
        let output = run_command(&["list-workspaces", "--all", "--json"]).await?;
        let workspaces: Vec<AerospaceWorkspace> = serde_json::from_str(&output)?;
        Ok(workspaces)
    }

    async fn get_focused_monitor(&self) -> anyhow::Result<Option<AerospaceMonitor>> {
        let output = run_command(&["list-monitors", "--focused", "--json"]).await?;
        let monitors: Vec<AerospaceMonitor> = serde_json::from_str(&output)?;
        Ok(monitors.into_iter().next())
    }

    async fn get_focused_window(&self) -> anyhow::Result<Option<AerospaceWindow>> {
        let output = run_command(&["list-windows", "--focused", "--json"]).await?;
        let windows: Vec<AerospaceWindow> = serde_json::from_str(&output)?;
        Ok(windows.into_iter().next())
    }

    async fn get_focused_workspace(&self) -> anyhow::Result<Option<AerospaceWorkspace>> {
        let output = run_command(&["list-workspaces", "--focused", "--json"]).await?;
        let workspaces: Vec<AerospaceWorkspace> = serde_json::from_str(&output)?;
        Ok(workspaces.into_iter().next())
    }

    async fn get_visible_workspace(&self, monitor_id: u32) -> anyhow::Result<String> {
        let output = run_command(&[
            "list-workspaces",
            "--monitor",
            &monitor_id.to_string(),
            "--visible",
            "--json",
        ])
        .await?;
        let workspaces: Vec<AerospaceWorkspace> = serde_json::from_str(&output)?;
        workspaces
            .into_iter()
            .next()
            .map(|w| w.workspace)
            .ok_or_else(|| anyhow::anyhow!("No visible workspace found for monitor {}", monitor_id))
    }

    async fn move_node_to_workspace(&self, window_id: u32, workspace: &str) -> anyhow::Result<()> {
        run_command(&[
            "move-node-to-workspace",
            "--window-id",
            &window_id.to_string(),
            workspace,
        ])
        .await?;
        Ok(())
    }

    async fn focus_workspace(&self, workspace: &str) -> anyhow::Result<()> {
        run_command(&["workspace", workspace]).await?;
        Ok(())
    }

    async fn focus_window(&self, window_id: u32) -> anyhow::Result<()> {
        run_command(&["focus", "--window-id", &window_id.to_string()]).await?;
        Ok(())
    }

    async fn focus_monitor(&self, monitor_id: u32) -> anyhow::Result<()> {
        run_command(&["focus-monitor", &monitor_id.to_string()]).await?;
        Ok(())
    }

    async fn move_workspace_to_monitor(
        &self,
        workspace: &str,
        monitor_id: u32,
    ) -> anyhow::Result<()> {
        run_command(&[
            "move-workspace-to-monitor",
            "--workspace",
            workspace,
            &monitor_id.to_string(),
        ])
        .await?;
        Ok(())
    }
}

async fn run_command(args: &[&str]) -> anyhow::Result<String> {
    tracing::debug!("Running command: aerospace {}", args.join(" "));

    let output = Command::new("aerospace").args(args).output().await?;

    if output.status.success() {
        let stdout = String::from_utf8(output.stdout)?;
        tracing::debug!("Command success. Output: {}", stdout.trim());
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!(
            "Command failed: aerospace {}
Error: {}",
            args.join(" "),
            stderr
        );
        anyhow::bail!("Aerospace command failed: {}", stderr);
    }
}
