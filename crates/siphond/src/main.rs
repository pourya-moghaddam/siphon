mod queue;
mod scheduler;
mod server;

use anyhow::Result;
use clap::Parser;
use queue::QueueManager;
use scheduler::Scheduler;
use server::Server;
use siphon_core::engine::{SegmentEngine, TokenBucket};
use siphon_core::storage::Database;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "siphond")]
#[command(about = "High-performance background download daemon for Siphon", version)]
struct Args {
    /// Path to Unix domain socket for client communication
    #[arg(short, long)]
    socket: Option<PathBuf>,

    /// Path to SQLite database file
    #[arg(short, long)]
    db: Option<PathBuf>,

    /// Optional TCP address to listen on (e.g. 0.0.0.0:9898)
    #[arg(short, long)]
    listen: Option<String>,

    /// Optional authentication token for TCP connections
    #[arg(short, long)]
    token: Option<String>,

    /// Default download destination directory
    #[arg(long)]
    download_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    let socket_path = args.socket.unwrap_or_else(siphon_core::default_socket_path);
    let db_path = args.db.unwrap_or_else(siphon_core::default_db_path);
    let download_dir = args.download_dir.unwrap_or_else(siphon_core::default_download_dir);

    info!("Starting siphond v{}", siphon_core::version());
    info!("Database path: {:?}", db_path);
    info!("Default download directory: {:?}", download_dir);

    // Initialize database
    let db = Database::open(&db_path)?;

    // Initialize engine & queue manager
    let rate_limiter = TokenBucket::unlimited();
    let (engine, _progress_rx) = SegmentEngine::new(db.clone(), rate_limiter.clone());
    let engine = Arc::new(engine);

    let queue_mgr = QueueManager::new(db.clone(), engine.clone(), rate_limiter, download_dir);

    // Start background scheduler
    let scheduler = Arc::new(Scheduler::new(db.clone(), queue_mgr.clone()));
    scheduler.start();

    // Trigger initial queue tick
    queue_mgr.notify_tick();

    // Build IPC server
    let server = Server::new(
        db,
        queue_mgr.clone(),
        socket_path.clone(),
        args.listen,
        args.token,
    );

    // Setup graceful shutdown handler
    let q_mgr_shutdown = queue_mgr.clone();
    let sock_shutdown = socket_path.clone();

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Received shutdown signal. Stopping active downloads and cleaning up...");
        q_mgr_shutdown.cancel_all().await;
        let _ = std::fs::remove_file(&sock_shutdown);
        info!("Siphon daemon stopped cleanly.");
        std::process::exit(0);
    });

    server.run().await?;

    Ok(())
}
