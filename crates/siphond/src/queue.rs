use anyhow::{Context, Result};
use chrono::Utc;
use siphon_core::engine::{HttpProbe, SegmentEngine, TokenBucket};
use siphon_core::models::{DownloadTask, TaskStatus};
use siphon_core::storage::Database;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

pub struct ActiveTaskHandle {
    pub cancel_token: CancellationToken,
}

#[derive(Clone)]
pub struct QueueManager {
    db: Database,
    engine: Arc<SegmentEngine>,
    rate_limiter: TokenBucket,
    active_tasks: Arc<Mutex<HashMap<String, ActiveTaskHandle>>>,
    default_download_dir: PathBuf,
    tick_tx: mpsc::Sender<()>,
}

impl QueueManager {
    pub fn new(
        db: Database,
        engine: Arc<SegmentEngine>,
        rate_limiter: TokenBucket,
        default_download_dir: PathBuf,
    ) -> Self {
        let (tick_tx, mut tick_rx) = mpsc::channel::<()>(64);

        let q_mgr = Self {
            db,
            engine,
            rate_limiter,
            active_tasks: Arc::new(Mutex::new(HashMap::new())),
            default_download_dir,
            tick_tx,
        };

        // Background queue processing loop
        let loop_mgr = q_mgr.clone();
        tokio::spawn(async move {
            while let Some(_) = tick_rx.recv().await {
                // Drain any additional pending ticks to debounce
                while let Ok(_) = tick_rx.try_recv() {}
                loop_mgr.process_queues().await;
            }
        });

        q_mgr
    }

    pub fn notify_tick(&self) {
        let _ = self.tick_tx.try_send(());
    }

    pub async fn add_task(
        &self,
        url: &str,
        dest_path: Option<&str>,
        queue_name: Option<&str>,
        connections: Option<usize>,
        _speed_limit: Option<u64>,
    ) -> Result<DownloadTask> {
        let q_name = queue_name.unwrap_or("default").to_string();

        let probe = HttpProbe::new();
        let probe_result = probe.probe(url).await.context("Failed to probe download URL")?;

        let file_name = probe_result.file_name;
        let category = siphon_core::categorize_file(&file_name);

        let final_dest = match dest_path {
            Some(p) => PathBuf::from(p),
            None => {
                let cat_dir = self.default_download_dir.join(category.subfolder());
                cat_dir.join(&file_name)
            }
        };

        let task_id = uuid::Uuid::new_v4().to_string();
        let task = DownloadTask {
            id: task_id,
            url: probe_result.effective_url,
            file_name,
            destination_path: final_dest,
            total_bytes: probe_result.total_bytes,
            downloaded_bytes: 0,
            speed_bps: 0,
            eta_seconds: None,
            supports_range: probe_result.supports_range,
            connections: connections.unwrap_or(8),
            queue_name: q_name,
            status: TaskStatus::Queued,
            error_message: None,
            category: Some(category.as_str().to_string()),
            etag: probe_result.etag,
            created_at: Utc::now(),
            completed_at: None,
        };

        self.db.insert_task(&task)?;
        info!("Added new download task {}: {}", task.id, task.file_name);

        self.notify_tick();
        Ok(task)
    }

    pub async fn pause_task(&self, task_id: &str) -> Result<()> {
        let mut active = self.active_tasks.lock().await;
        if let Some(handle) = active.remove(task_id) {
            handle.cancel_token.cancel();
            info!("Cancelled active worker for task {}", task_id);
        } else {
            self.db.update_task_status(task_id, TaskStatus::Paused)?;
        }
        Ok(())
    }

    pub async fn resume_task(&self, task_id: &str) -> Result<()> {
        let task = self.db.get_task(task_id)?.context("Task not found")?;
        if task.status != TaskStatus::Completed {
            self.db.update_task_status(task_id, TaskStatus::Queued)?;
            self.notify_tick();
        }
        Ok(())
    }

    pub async fn delete_task(&self, task_id: &str, delete_file: bool) -> Result<()> {
        self.pause_task(task_id).await?;
        if let Ok(Some(task)) = self.db.get_task(task_id) {
            if delete_file {
                let _ = std::fs::remove_file(&task.destination_path);
                let _ = std::fs::remove_file(task.part_file_path());
            }
        }
        self.db.delete_task(task_id)?;
        info!("Deleted task {}", task_id);
        self.notify_tick();
        Ok(())
    }

    pub async fn refresh_task_url(&self, task_id: &str, new_url: &str) -> Result<()> {
        let task = self.db.get_task(task_id)?.context("Task not found")?;

        let probe = HttpProbe::new();
        let probe_result = probe.probe(new_url).await.context("Failed to probe new URL")?;

        if let (Some(orig_len), Some(new_len)) = (task.total_bytes, probe_result.total_bytes) {
            if orig_len != new_len {
                anyhow::bail!(
                    "File size mismatch: original was {} bytes, new URL has {} bytes",
                    orig_len,
                    new_len
                );
            }
        }

        self.pause_task(task_id).await?;
        self.db.update_task_url(task_id, &probe_result.effective_url)?;
        info!("Refreshed URL for task {} to {}", task_id, probe_result.effective_url);

        self.notify_tick();
        Ok(())
    }

    pub async fn set_global_speed_limit(&self, limit_bps: u64) {
        self.rate_limiter.set_rate(limit_bps).await;
        info!("Set global speed limit to {} bps", limit_bps);
    }

    async fn process_queues(&self) {
        let queues = match self.db.list_queues() {
            Ok(q) => q,
            Err(e) => {
                error!("Failed to list queues: {:?}", e);
                return;
            }
        };

        let tasks = match self.db.list_tasks() {
            Ok(t) => t,
            Err(e) => {
                error!("Failed to list tasks: {:?}", e);
                return;
            }
        };

        let mut active_lock = self.active_tasks.lock().await;

        for queue in queues {
            if !queue.is_active {
                continue;
            }

            let active_in_queue = tasks
                .iter()
                .filter(|t| t.queue_name == queue.name && active_lock.contains_key(&t.id))
                .count();

            let available_slots = queue.max_concurrent.saturating_sub(active_in_queue);
            if available_slots == 0 {
                continue;
            }

            let queued_tasks: Vec<DownloadTask> = tasks
                .iter()
                .filter(|t| t.queue_name == queue.name && t.status == TaskStatus::Queued)
                .take(available_slots)
                .cloned()
                .collect();

            for task in queued_tasks {
                let task_id = task.id.clone();
                let cancel_token = CancellationToken::new();

                active_lock.insert(
                    task_id.clone(),
                    ActiveTaskHandle {
                        cancel_token: cancel_token.clone(),
                    },
                );

                let engine = self.engine.clone();
                let active_map = self.active_tasks.clone();
                let tick_notifier = self.tick_tx.clone();

                tokio::spawn(async move {
                    let tid = task.id.clone();
                    let res = engine.run_task(task, cancel_token).await;

                    if let Err(e) = res {
                        warn!("Task {} finished with error: {:?}", tid, e);
                    }

                    {
                        let mut lock = active_map.lock().await;
                        lock.remove(&tid);
                    }

                    let _ = tick_notifier.try_send(());
                });
            }
        }
    }

    pub async fn active_tasks_count(&self) -> usize {
        self.active_tasks.lock().await.len()
    }

    pub async fn cancel_all(&self) {
        let mut active = self.active_tasks.lock().await;
        for (_, handle) in active.drain() {
            handle.cancel_token.cancel();
        }
    }
}
