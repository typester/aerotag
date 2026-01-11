mod aerospace;
mod cli;
mod client;
mod ipc;
mod server;
mod state;
mod util;

use cli::{Args, SubCommand};
use ipc::IpcCommand;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args: Args = argh::from_env();

    match args.cmd {
        SubCommand::Server(_) => server::run_server().await,
        SubCommand::TagView(cmd) => {
            if cmd.tag == 0 {
                anyhow::bail!("Tag index must be 1-based");
            }
            client::send_client_command(IpcCommand::TagView(cmd.tag - 1)).await
        }
        SubCommand::TagToggle(cmd) => {
            if cmd.tag == 0 {
                anyhow::bail!("Tag index must be 1-based");
            }
            client::send_client_command(IpcCommand::TagToggle(cmd.tag - 1)).await
        }
        SubCommand::TagLast(_) => client::send_client_command(IpcCommand::TagLast).await,
        SubCommand::TagSet(cmd) => {
            client::send_client_command(IpcCommand::TagSet(cmd.mask, cmd.monitor_id)).await
        }
        SubCommand::WindowMove(cmd) => {
            if cmd.tag == 0 {
                anyhow::bail!("Tag index must be 1-based");
            }
            client::send_client_command(IpcCommand::WindowMove(cmd.tag - 1)).await
        }
        SubCommand::WindowToggle(cmd) => {
            if cmd.tag == 0 {
                anyhow::bail!("Tag index must be 1-based");
            }
            client::send_client_command(IpcCommand::WindowToggle(cmd.tag - 1)).await
        }
        SubCommand::WindowMoveMonitor(cmd) => {
            client::send_client_command(IpcCommand::WindowMoveMonitor(cmd.target)).await
        }
        SubCommand::WindowSet(cmd) => {
            client::send_client_command(IpcCommand::WindowSet(cmd.mask, cmd.window_id)).await
        }
        SubCommand::Query(cmd) => client::run_query(cmd).await,
        SubCommand::Version(_) => {
            println!("v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        SubCommand::Hook(_) => client::send_client_command(IpcCommand::Sync).await,
        SubCommand::Subscribe(_) => client::run_subscriber().await,
    }
}