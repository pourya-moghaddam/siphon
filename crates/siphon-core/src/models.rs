use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Queued,
    Probing,
    Downloading,
    Paused,
    Completed,
    Error(String),
    Expired, // Link expired (HTTP 401/403/410), needs link refresh
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Queued => write!(f, "Queued"),
            TaskStatus::Probing => write!(f, "Probing"),
            TaskStatus::Downloading => write!(f, "Downloading"),
            TaskStatus::Paused => write!(f, "Paused"),
            TaskStatus::Completed => write!(f, "Completed"),
            TaskStatus::Error(err) => write!(f, "Error: {}", err),
            TaskStatus::Expired => write!(f, "Expired"),
        }
    }
}

impl TaskStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, TaskStatus::Probing | TaskStatus::Downloading)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, TaskStatus::Completed | TaskStatus::Error(_))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadTask {
    pub id: String,
    pub url: String,
    pub file_name: String,
    pub destination_path: PathBuf,
    pub total_bytes: Option<u64>,
    pub downloaded_bytes: u64,
    pub speed_bps: u64,
    pub eta_seconds: Option<u64>,
    pub supports_range: bool,
    pub connections: usize,
    pub queue_name: String,
    pub status: TaskStatus,
    pub error_message: Option<String>,
    pub category: Option<String>,
    pub etag: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl DownloadTask {
    pub fn progress_percent(&self) -> f64 {
        if self.status == TaskStatus::Completed {
            return 100.0;
        }
        match self.total_bytes {
            Some(total) if total > 0 => {
                let p = (self.downloaded_bytes as f64 / total as f64) * 100.0;
                p.min(100.0)
            }
            _ => 0.0,
        }
    }

    pub fn part_file_path(&self) -> PathBuf {
        let mut path = self.destination_path.clone();
        let ext = path.extension().unwrap_or_default().to_string_lossy();
        let new_ext = if ext.is_empty() {
            "siphon.part".to_string()
        } else {
            format!("{}.siphon.part", ext)
        };
        path.set_extension(new_ext);
        path
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub id: usize,
    pub task_id: String,
    pub start_byte: u64,
    pub current_byte: u64,
    pub end_byte: u64,
    pub active: bool,
}

impl Segment {
    pub fn total_bytes(&self) -> u64 {
        if self.end_byte >= self.start_byte {
            self.end_byte - self.start_byte + 1
        } else {
            0
        }
    }

    pub fn downloaded_bytes(&self) -> u64 {
        if self.current_byte >= self.start_byte {
            self.current_byte - self.start_byte
        } else {
            0
        }
    }

    pub fn remaining_bytes(&self) -> u64 {
        if self.end_byte >= self.current_byte {
            self.end_byte - self.current_byte + 1
        } else {
            0
        }
    }

    pub fn progress_percent(&self) -> f64 {
        let total = self.total_bytes();
        if total == 0 {
            100.0
        } else {
            ((self.downloaded_bytes() as f64 / total as f64) * 100.0).min(100.0)
        }
    }

    pub fn is_finished(&self) -> bool {
        if self.start_byte == 0 && self.end_byte == 0 {
            self.current_byte > 0
        } else {
            self.current_byte > self.end_byte
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Queue {
    pub name: String,
    pub max_concurrent: usize,
    pub speed_limit_bps: Option<u64>,
    pub is_active: bool,
    pub schedule_start: Option<String>, // "HH:MM" e.g. "02:00"
    pub schedule_end: Option<String>,   // "HH:MM" e.g. "07:00"
}

impl Default for Queue {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            max_concurrent: 3,
            speed_limit_bps: None,
            is_active: true,
            schedule_start: None,
            schedule_end: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OnCompleteAction {
    None,
    Shutdown,
    Sleep,
    CustomScript(String),
    Webhook(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleConfig {
    pub queue_name: String,
    pub start_time: Option<String>,
    pub stop_time: Option<String>,
    pub on_complete: OnCompleteAction,
}
