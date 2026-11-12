use crate::models::{DownloadTask, OnCompleteAction, Queue, Segment};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    AddTask {
        url: String,
        destination: Option<String>,
        queue: Option<String>,
        connections: Option<usize>,
        speed_limit: Option<u64>,
    },
    PauseTask {
        id: String,
    },
    ResumeTask {
        id: String,
    },
    DeleteTask {
        id: String,
        delete_file: bool,
    },
    RefreshTaskUrl {
        id: String,
        new_url: String,
    },
    ListTasks,
    GetTaskDetails {
        id: String,
    },
    ListQueues,
    CreateQueue {
        name: String,
        max_concurrent: usize,
        speed_limit: Option<u64>,
    },
    ConfigureSchedule {
        queue_name: String,
        start_time: Option<String>,
        stop_time: Option<String>,
        on_complete: OnCompleteAction,
    },
    SetGlobalSpeedLimit {
        limit_bps: u64,
    },
    GetDaemonStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatusInfo {
    pub version: String,
    pub uptime_seconds: u64,
    pub active_downloads: usize,
    pub global_speed_bps: u64,
    pub queues_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Ok,
    TaskAdded(DownloadTask),
    Tasks(Vec<DownloadTask>),
    TaskDetail {
        task: DownloadTask,
        segments: Vec<Segment>,
    },
    Queues(Vec<Queue>),
    DaemonStatus(DaemonStatusInfo),
    Error(String),
}

impl Request {
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn from_str(s: &str) -> Result<Self> {
        serde_json::from_str(s).context("Failed to parse Request JSON")
    }
}

impl Response {
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn from_str(s: &str) -> Result<Self> {
        serde_json::from_str(s).context("Failed to parse Response JSON")
    }
}
