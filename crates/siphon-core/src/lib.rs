pub mod engine;
pub mod models;
pub mod protocol;
pub mod storage;

pub use engine::{categorize_file, DiskWriter, HttpProbe, ProbeResult, SegmentEngine, TokenBucket};
pub use models::{DownloadTask, OnCompleteAction, Queue, ScheduleConfig, Segment, TaskStatus};
pub use protocol::{DaemonStatusInfo, Request, Response};
pub use storage::Database;

pub fn default_socket_path() -> std::path::PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        std::path::PathBuf::from(runtime_dir).join("siphon.sock")
    } else if let Some(home) = dirs::home_dir() {
        home.join(".local/share/siphon/siphon.sock")
    } else {
        std::path::PathBuf::from("/tmp/siphon.sock")
    }
}

pub fn default_db_path() -> std::path::PathBuf {
    if let Some(data_dir) = dirs::data_local_dir() {
        data_dir.join("siphon/siphon.db")
    } else if let Some(home) = dirs::home_dir() {
        home.join(".local/share/siphon/siphon.db")
    } else {
        std::path::PathBuf::from("/tmp/siphon.db")
    }
}

pub fn default_download_dir() -> std::path::PathBuf {
    dirs::download_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .map(|h| h.join("Downloads"))
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp/downloads"))
    })
}

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
