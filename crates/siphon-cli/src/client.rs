use anyhow::{Context, Result};
use siphon_core::protocol::{Request, Response};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub struct IpcClient {
    socket_path: PathBuf,
}

impl IpcClient {
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

}

impl Default for IpcClient {
    fn default() -> Self {
        Self::new(siphon_core::default_socket_path())
    }
}

impl IpcClient {

    pub async fn send_request(&self, request: &Request) -> Result<Response> {
        let stream = UnixStream::connect(&self.socket_path).await.with_context(|| {
            format!(
                "Cannot connect to siphond daemon at {:?}. Is siphond running?",
                self.socket_path
            )
        })?;

        let (reader, mut writer) = stream.into_split();
        let req_bytes = request.to_bytes()?;
        writer.write_all(&req_bytes).await?;
        writer.flush().await?;

        let mut buf_reader = BufReader::new(reader);
        let mut response_line = String::new();
        let bytes_read = buf_reader.read_line(&mut response_line).await?;

        if bytes_read == 0 {
            anyhow::bail!("Daemon closed connection unexpectedly");
        }

        Response::from_str(response_line.trim())
    }
}
