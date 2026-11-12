use super::disk::DiskWriter;
use super::limiter::TokenBucket;
use crate::models::{DownloadTask, Segment, TaskStatus};
use crate::storage::Database;
use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use reqwest::header::{RANGE, USER_AGENT};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

const MIN_SPLIT_THRESHOLD: u64 = 2 * 1024 * 1024; // 2 MB

#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    pub task_id: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bps: u64,
    pub eta_seconds: Option<u64>,
    pub segments: Vec<Segment>,
    pub status: TaskStatus,
}

pub struct SegmentEngine {
    client: reqwest::Client,
    db: Database,
    rate_limiter: TokenBucket,
    progress_tx: broadcast::Sender<ProgressUpdate>,
}

impl SegmentEngine {
    pub fn new(db: Database, rate_limiter: TokenBucket) -> (Self, broadcast::Receiver<ProgressUpdate>) {
        let (progress_tx, progress_rx) = broadcast::channel(128);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .expect("Failed to build HTTP client for engine");

        (
            Self {
                client,
                db,
                rate_limiter,
                progress_tx,
            },
            progress_rx,
        )
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ProgressUpdate> {
        self.progress_tx.subscribe()
    }

    pub async fn run_task(
        &self,
        task: DownloadTask,
        cancel_token: CancellationToken,
    ) -> Result<()> {
        info!("Starting download task {}: {}", task.id, task.file_name);
        self.db.update_task_status(&task.id, TaskStatus::Downloading)?;

        let total_bytes = task.total_bytes;
        let disk_writer = DiskWriter::open_or_create(&task.destination_path, total_bytes)?;

        // Prepare initial segments
        let mut segments = self.db.get_segments(&task.id)?;
        if segments.is_empty() {
            segments = self.create_initial_segments(&task.id, total_bytes, task.connections, task.supports_range);
            self.db.save_segments(&task.id, &segments)?;
        }

        // Shared segment state
        let segments_state = Arc::new(Mutex::new(segments));
        let total_downloaded = Arc::new(AtomicU64::new(task.downloaded_bytes));
        let is_failed = Arc::new(AtomicBool::new(false));
        let failure_reason = Arc::new(Mutex::new(None));

        // Start progress tracking loop
        let progress_cancel = CancellationToken::new();
        let loop_progress_cancel = progress_cancel.clone();
        let loop_task_cancel = cancel_token.clone();
        let p_task_id = task.id.clone();
        let p_total_bytes = total_bytes;
        let p_segments = segments_state.clone();
        let p_downloaded = total_downloaded.clone();
        let p_tx = self.progress_tx.clone();
        let p_db = self.db.clone();

        let progress_handle = tokio::spawn(async move {
            let mut last_bytes = p_downloaded.load(Ordering::Relaxed);
            let mut last_time = Instant::now();
            let mut ticker = tokio::time::interval(Duration::from_millis(500));

            loop {
                tokio::select! {
                    _ = loop_progress_cancel.cancelled() => break,
                    _ = loop_task_cancel.cancelled() => break,
                    _ = ticker.tick() => {
                        let current_bytes = p_downloaded.load(Ordering::Relaxed);
                        let now = Instant::now();
                        let elapsed = now.duration_since(last_time).as_secs_f64();
                        last_time = now;

                        let delta = current_bytes.saturating_sub(last_bytes);
                        last_bytes = current_bytes;
                        let speed_bps = if elapsed > 0.0 { (delta as f64 / elapsed) as u64 } else { 0 };

                        let eta = if speed_bps > 0 {
                            p_total_bytes.map(|total| {
                                let rem = total.saturating_sub(current_bytes);
                                rem / speed_bps
                            })
                        } else {
                            None
                        };

                        let seg_snapshot = {
                            let lock = p_segments.lock().await;
                            lock.clone()
                        };

                        let _ = p_db.update_task_progress(&p_task_id, current_bytes, speed_bps, eta);
                        let _ = p_db.save_segments(&p_task_id, &seg_snapshot);

                        let _ = p_tx.send(ProgressUpdate {
                            task_id: p_task_id.clone(),
                            downloaded_bytes: current_bytes,
                            total_bytes: p_total_bytes,
                            speed_bps,
                            eta_seconds: eta,
                            segments: seg_snapshot,
                            status: TaskStatus::Downloading,
                        });
                    }
                }
            }
        });

        // Worker coordination
        let num_workers = if task.supports_range && total_bytes.is_some() {
            task.connections.max(1)
        } else {
            1
        };

        let mut worker_handles = Vec::new();

        for worker_id in 0..num_workers {
            let client = self.client.clone();
            let url = task.url.clone();
            let disk = disk_writer.clone();
            let limiter = self.rate_limiter.clone();
            let segs = segments_state.clone();
            let total_dl = total_downloaded.clone();
            let cancel = cancel_token.clone();
            let failed = is_failed.clone();
            let fail_msg = failure_reason.clone();
            let supports_range = task.supports_range;

            let handle = tokio::spawn(async move {
                loop {
                    if cancel.is_cancelled() || failed.load(Ordering::Relaxed) {
                        break;
                    }

                    // Find or claim next segment to work on
                    let target_segment = {
                        let mut lock = segs.lock().await;
                        // First check if an unfinished segment is unassigned or inactive
                        let mut found = None;
                        for seg in lock.iter_mut() {
                            if !seg.is_finished() && !seg.active {
                                seg.active = true;
                                found = Some(seg.clone());
                                break;
                            }
                        }

                        // If all existing segments are active or finished, attempt dynamic chunk stealing!
                        if found.is_none() && supports_range {
                            let mut best_idx = None;
                            let mut max_rem = 0;

                            for (idx, seg) in lock.iter().enumerate() {
                                if seg.active && !seg.is_finished() {
                                    let rem = seg.remaining_bytes();
                                    if rem > max_rem && rem >= MIN_SPLIT_THRESHOLD {
                                        max_rem = rem;
                                        best_idx = Some(idx);
                                    }
                                }
                            }

                            if let Some(donor_idx) = best_idx {
                                let new_id = lock.len();
                                let donor = &mut lock[donor_idx];
                                let split_point = donor.current_byte + (max_rem / 2);
                                let old_end = donor.end_byte;
                                let donor_task_id = donor.task_id.clone();

                                // Shorten donor
                                donor.end_byte = split_point.saturating_sub(1);

                                // Create new child segment
                                let new_seg = Segment {
                                    id: new_id,
                                    task_id: donor_task_id,
                                    start_byte: split_point,
                                    current_byte: split_point,
                                    end_byte: old_end,
                                    active: true,
                                };
                                lock.push(new_seg.clone());
                                found = Some(new_seg);
                            }
                        }

                        found
                    };

                    let seg = match target_segment {
                        Some(s) => s,
                        None => {
                            // No more work for this worker
                            break;
                        }
                    };

                    // Perform download for this segment
                    let result = Self::download_segment_stream(
                        &client,
                        &url,
                        seg.id,
                        seg.current_byte,
                        seg.end_byte,
                        supports_range,
                        &disk,
                        &limiter,
                        &segs,
                        &total_dl,
                        &cancel,
                    )
                    .await;

                    // Mark segment inactive
                    {
                        let mut lock = segs.lock().await;
                        if let Some(s) = lock.iter_mut().find(|s| s.id == seg.id) {
                            s.active = false;
                        }
                    }

                    if let Err(e) = result {
                        if !cancel.is_cancelled() {
                            warn!("Worker {} encountered error: {:?}", worker_id, e);
                            failed.store(true, Ordering::Relaxed);
                            let mut lock = fail_msg.lock().await;
                            if lock.is_none() {
                                *lock = Some(format!("{}", e));
                            }
                        }
                        break;
                    }
                }
            });

            worker_handles.push(handle);
        }

        // Wait for workers
        for handle in worker_handles {
            let _ = handle.await;
        }

        progress_cancel.cancel();
        let _ = progress_handle.await;

        if cancel_token.is_cancelled() {
            info!("Task {} was paused/cancelled", task.id);
            self.db.update_task_status(&task.id, TaskStatus::Paused)?;
            let final_segs = segments_state.lock().await.clone();
            self.db.save_segments(&task.id, &final_segs)?;
            return Ok(());
        }

        if is_failed.load(Ordering::Relaxed) {
            let err_msg = failure_reason
                .lock()
                .await
                .take()
                .unwrap_or_else(|| "Download failed".to_string());

            let status = if err_msg.contains("401") || err_msg.contains("403") || err_msg.contains("410") {
                TaskStatus::Expired
            } else {
                TaskStatus::Error(err_msg.clone())
            };

            error!("Task {} failed: {:?}", task.id, status);
            self.db.update_task_status(&task.id, status.clone())?;
            let final_segs = segments_state.lock().await.clone();
            self.db.save_segments(&task.id, &final_segs)?;

            let _ = self.progress_tx.send(ProgressUpdate {
                task_id: task.id.clone(),
                downloaded_bytes: total_downloaded.load(Ordering::Relaxed),
                total_bytes,
                speed_bps: 0,
                eta_seconds: None,
                segments: final_segs,
                status,
            });

            bail!("Download error: {}", err_msg);
        }

        // Verify completion
        let final_segs = segments_state.lock().await.clone();
        let all_finished = final_segs.iter().all(|s| s.is_finished());

        if all_finished {
            info!("Task {} completed successfully. Finalizing file...", task.id);
            disk_writer.finish()?;
            let final_downloaded = total_bytes.unwrap_or_else(|| total_downloaded.load(Ordering::Relaxed));
            let _ = self.db.update_task_progress(&task.id, final_downloaded, 0, Some(0));
            self.db.update_task_status(&task.id, TaskStatus::Completed)?;
            self.db.save_segments(&task.id, &final_segs)?;

            let _ = self.progress_tx.send(ProgressUpdate {
                task_id: task.id.clone(),
                downloaded_bytes: total_downloaded.load(Ordering::Relaxed),
                total_bytes,
                speed_bps: 0,
                eta_seconds: Some(0),
                segments: final_segs,
                status: TaskStatus::Completed,
            });
        }

        Ok(())
    }

    async fn download_segment_stream(
        client: &reqwest::Client,
        url: &str,
        segment_id: usize,
        start_byte: u64,
        end_byte: u64,
        supports_range: bool,
        disk: &DiskWriter,
        limiter: &TokenBucket,
        segs: &Arc<Mutex<Vec<Segment>>>,
        total_dl: &Arc<AtomicU64>,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let user_agent = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36 Siphon/0.1.0";
        let mut req = client.get(url).header(USER_AGENT, user_agent);

        if supports_range {
            if end_byte > 0 {
                let range_val = format!("bytes={}-{}", start_byte, end_byte);
                req = req.header(RANGE, range_val);
            } else {
                let range_val = format!("bytes={}-", start_byte);
                req = req.header(RANGE, range_val);
            }
        }

        let res = req.send().await.context("Failed to connect to download URL")?;
        let status = res.status();

        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
            || status == reqwest::StatusCode::GONE
        {
            bail!("HTTP {} link expired or forbidden", status);
        }

        if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
            bail!("HTTP error {}", status);
        }

        let mut stream = res.bytes_stream();
        let mut curr = start_byte;

        while let Some(chunk_res) = stream.next().await {
            if cancel.is_cancelled() {
                break;
            }

            let chunk = chunk_res.context("Network stream error while reading chunk")?;
            let chunk_len = chunk.len();

            // Rate limiting
            limiter.acquire(chunk_len).await;

            // Direct random-access disk write
            disk.write_at(curr, &chunk)?;

            curr += chunk_len as u64;
            total_dl.fetch_add(chunk_len as u64, Ordering::Relaxed);

            // Update segment current_byte
            {
                let mut lock = segs.lock().await;
                if let Some(s) = lock.iter_mut().find(|s| s.id == segment_id) {
                    s.current_byte = curr;
                    // Check if end_byte was adjusted dynamically by another worker
                    if curr > s.end_byte && s.end_byte != 0 {
                        break;
                    }
                }
            }
        }

        // Mark segment done
        {
            let mut lock = segs.lock().await;
            if let Some(s) = lock.iter_mut().find(|s| s.id == segment_id) {
                s.current_byte = curr;
            }
        }

        Ok(())
    }

    fn create_initial_segments(
        &self,
        task_id: &str,
        total_bytes: Option<u64>,
        num_connections: usize,
        supports_range: bool,
    ) -> Vec<Segment> {
        let mut segments = Vec::new();

        if let (Some(total), true) = (total_bytes, supports_range) {
            let connections = if total == 0 {
                1
            } else {
                num_connections.max(1).min(total as usize)
            };
            let chunk_size = (total / connections as u64).max(1);

            for i in 0..connections {
                let start_byte = i as u64 * chunk_size;
                let end_byte = if i == connections - 1 {
                    total - 1
                } else {
                    (i as u64 + 1) * chunk_size - 1
                };

                segments.push(Segment {
                    id: i,
                    task_id: task_id.to_string(),
                    start_byte,
                    current_byte: start_byte,
                    end_byte,
                    active: false,
                });
            }
        } else {
            // Single stream fallback
            segments.push(Segment {
                id: 0,
                task_id: task_id.to_string(),
                start_byte: 0,
                current_byte: 0,
                end_byte: total_bytes.unwrap_or(0),
                active: false,
            });
        }

        segments
    }
}
