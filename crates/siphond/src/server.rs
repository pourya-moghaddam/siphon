use crate::queue::QueueManager;
use anyhow::{Context, Result};
use siphon_core::models::Queue;
use siphon_core::protocol::{DaemonStatusInfo, Request, Response};
use siphon_core::storage::Database;
use std::path::PathBuf;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixListener};
use tracing::{error, info, warn};

pub struct Server {
    db: Database,
    queue_mgr: QueueManager,
    socket_path: PathBuf,
    tcp_addr: Option<String>,
    _auth_token: Option<String>,
    start_time: Instant,
}

impl Server {
    pub fn new(
        db: Database,
        queue_mgr: QueueManager,
        socket_path: PathBuf,
        tcp_addr: Option<String>,
        auth_token: Option<String>,
    ) -> Self {
        Self {
            db,
            queue_mgr,
            socket_path,
            tcp_addr,
            _auth_token: auth_token,
            start_time: Instant::now(),
        }
    }

    pub async fn run(self) -> Result<()> {
        // Prepare unix socket path
        if let Some(parent) = self.socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::remove_file(&self.socket_path);

        let unix_listener = UnixListener::bind(&self.socket_path)
            .with_context(|| format!("Failed to bind Unix socket at {:?}", self.socket_path))?;
        info!("Siphon daemon listening on Unix socket: {:?}", self.socket_path);

        let db = self.db.clone();
        let q_mgr = self.queue_mgr.clone();
        let start_time = self.start_time;

        // Spawn Unix socket listener loop
        let unix_task = tokio::spawn(async move {
            loop {
                match unix_listener.accept().await {
                    Ok((stream, _)) => {
                        let (reader, writer) = stream.into_split();
                        let db_c = db.clone();
                        let q_c = q_mgr.clone();
                        tokio::spawn(async move {
                            if let Err(e) = Self::handle_connection(reader, writer, db_c, q_c, start_time).await {
                                warn!("Unix client connection ended: {:?}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Failed to accept Unix socket connection: {:?}", e);
                        break;
                    }
                }
            }
        });

        // Optional TCP listener
        if let Some(ref addr) = self.tcp_addr {
            let tcp_listener = TcpListener::bind(addr).await
                .with_context(|| format!("Failed to bind TCP listener on {}", addr))?;
            info!("Siphon daemon listening on TCP: {}", addr);

            let db = self.db.clone();
            let q_mgr = self.queue_mgr.clone();
            tokio::spawn(async move {
                loop {
                    match tcp_listener.accept().await {
                        Ok((stream, _)) => {
                            let (reader, writer) = stream.into_split();
                            let db_c = db.clone();
                            let q_c = q_mgr.clone();
                            tokio::spawn(async move {
                                let _ = Self::handle_connection(reader, writer, db_c, q_c, start_time).await;
                            });
                        }
                        Err(e) => {
                            error!("Failed to accept TCP connection: {:?}", e);
                            break;
                        }
                    }
                }
            });
        }

        unix_task.await?;
        Ok(())
    }

    async fn handle_connection<R, W>(
        reader: R,
        mut writer: W,
        db: Database,
        queue_mgr: QueueManager,
        start_time: Instant,
    ) -> Result<()>
    where
        R: tokio::io::AsyncRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        let mut buf_reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();
            let bytes_read = buf_reader.read_line(&mut line).await?;
            if bytes_read == 0 {
                break; // Connection closed
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let req: Request = match serde_json::from_str(trimmed) {
                Ok(r) => r,
                Err(e) => {
                    let resp = Response::Error(format!("Invalid request format: {}", e));
                    writer.write_all(&resp.to_bytes()?).await?;
                    continue;
                }
            };

            let resp = match req {
                Request::AddTask {
                    url,
                    destination,
                    queue,
                    connections,
                    speed_limit,
                } => match queue_mgr
                    .add_task(
                        &url,
                        destination.as_deref(),
                        queue.as_deref(),
                        connections,
                        speed_limit,
                    )
                    .await
                {
                    Ok(task) => Response::TaskAdded(task),
                    Err(e) => Response::Error(e.to_string()),
                },

                Request::PauseTask { id } => match queue_mgr.pause_task(&id).await {
                    Ok(_) => Response::Ok,
                    Err(e) => Response::Error(e.to_string()),
                },

                Request::ResumeTask { id } => match queue_mgr.resume_task(&id).await {
                    Ok(_) => Response::Ok,
                    Err(e) => Response::Error(e.to_string()),
                },

                Request::DeleteTask { id, delete_file } => {
                    match queue_mgr.delete_task(&id, delete_file).await {
                        Ok(_) => Response::Ok,
                        Err(e) => Response::Error(e.to_string()),
                    }
                }

                Request::RefreshTaskUrl { id, new_url } => {
                    match queue_mgr.refresh_task_url(&id, &new_url).await {
                        Ok(_) => Response::Ok,
                        Err(e) => Response::Error(e.to_string()),
                    }
                }

                Request::ListTasks => match db.list_tasks() {
                    Ok(tasks) => Response::Tasks(tasks),
                    Err(e) => Response::Error(e.to_string()),
                },

                Request::GetTaskDetails { id } => match db.get_task(&id) {
                    Ok(Some(task)) => {
                        let segs = db.get_segments(&id).unwrap_or_default();
                        Response::TaskDetail { task, segments: segs }
                    }
                    Ok(None) => Response::Error("Task not found".to_string()),
                    Err(e) => Response::Error(e.to_string()),
                },

                Request::ListQueues => match db.list_queues() {
                    Ok(queues) => Response::Queues(queues),
                    Err(e) => Response::Error(e.to_string()),
                },

                Request::CreateQueue {
                    name,
                    max_concurrent,
                    speed_limit,
                } => {
                    let mut q = db
                        .list_queues()
                        .unwrap_or_default()
                        .into_iter()
                        .find(|q| q.name == name)
                        .unwrap_or_else(|| Queue {
                            name: name.clone(),
                            max_concurrent: 3,
                            speed_limit_bps: None,
                            is_active: true,
                            schedule_start: None,
                            schedule_end: None,
                        });
                    q.max_concurrent = max_concurrent;
                    q.speed_limit_bps = speed_limit;

                    match db.upsert_queue(&q) {
                        Ok(_) => {
                            queue_mgr.notify_tick();
                            Response::Ok
                        }
                        Err(e) => Response::Error(e.to_string()),
                    }
                }

                Request::ConfigureSchedule {
                    queue_name,
                    start_time,
                    stop_time,
                    on_complete: _,
                } => {
                    let mut q = db
                        .list_queues()
                        .unwrap_or_default()
                        .into_iter()
                        .find(|q| q.name == queue_name)
                        .unwrap_or_else(|| Queue {
                            name: queue_name.clone(),
                            max_concurrent: 3,
                            speed_limit_bps: None,
                            is_active: true,
                            schedule_start: None,
                            schedule_end: None,
                        });

                    q.schedule_start = start_time;
                    q.schedule_end = stop_time;
                    if q.schedule_start.is_none() && q.schedule_end.is_none() {
                        q.is_active = true;
                    }

                    match db.upsert_queue(&q) {
                        Ok(_) => {
                            queue_mgr.notify_tick();
                            Response::Ok
                        }
                        Err(e) => Response::Error(e.to_string()),
                    }
                }

                Request::SetGlobalSpeedLimit { limit_bps } => {
                    queue_mgr.set_global_speed_limit(limit_bps).await;
                    Response::Ok
                }

                Request::GetDaemonStatus => {
                    let uptime = start_time.elapsed().as_secs();
                    let active = queue_mgr.active_tasks_count().await;
                    let queues = db.list_queues().map(|q| q.len()).unwrap_or(0);
                    let tasks = db.list_tasks().unwrap_or_default();
                    let total_speed: u64 = tasks.iter().map(|t| t.speed_bps).sum();

                    Response::DaemonStatus(DaemonStatusInfo {
                        version: siphon_core::version().to_string(),
                        uptime_seconds: uptime,
                        active_downloads: active,
                        global_speed_bps: total_speed,
                        queues_count: queues,
                    })
                }
            };

            writer.write_all(&resp.to_bytes()?).await?;
            writer.flush().await?;
        }

        Ok(())
    }
}
