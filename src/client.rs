use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use crate::ipc::{IpcCommand, QueryTarget};
use crate::cli::{QueryCommand, QuerySubCommand};
use crate::util::get_socket_path;

pub async fn send_client_command(cmd: IpcCommand) -> anyhow::Result<()> {
    let socket_path = get_socket_path();
    let mut stream = UnixStream::connect(socket_path).await?;
    let data = serde_json::to_vec(&cmd)?;
    stream.write_all(&data).await?;
    stream.shutdown().await?;
    Ok(())
}

pub async fn run_query(cmd: QueryCommand) -> anyhow::Result<()> {
    let socket_path = get_socket_path();
    let stream = UnixStream::connect(socket_path).await?;
    let (rx, mut tx) = stream.into_split();

    let target = match cmd.cmd {
        QuerySubCommand::Window(c) => QueryTarget::Window(c.window_id),
        QuerySubCommand::Monitor(c) => QueryTarget::Monitor(c.monitor_id),
        QuerySubCommand::State(_) => QueryTarget::State,
    };

    let data = serde_json::to_vec(&IpcCommand::Query(target))?;
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

pub async fn run_subscriber() -> anyhow::Result<()> {
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
