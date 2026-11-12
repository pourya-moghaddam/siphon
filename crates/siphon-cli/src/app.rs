use crate::client::IpcClient;
use anyhow::Result;
use siphon_core::models::{DownloadTask, Queue, Segment, TaskStatus};
use siphon_core::protocol::{DaemonStatusInfo, Request, Response};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    All = 0,
    Downloading = 1,
    Completed = 2,
    Queues = 3,
    Help = 4,
}

impl Tab {
    pub fn titles() -> Vec<&'static str> {
        vec!["[1] All Tasks", "[2] Downloading", "[3] Completed", "[4] Queues", "[5] Help"]
    }

    pub fn from_index(index: usize) -> Self {
        match index {
            1 => Tab::Downloading,
            2 => Tab::Completed,
            3 => Tab::Queues,
            4 => Tab::Help,
            _ => Tab::All,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModalState {
    None,
    AddDownload {
        url_input: String,
        dest_input: String,
        queue_input: String,
        focused_field: usize, // 0: URL, 1: Dest, 2: Queue
    },
    RefreshLink {
        task_id: String,
        task_name: String,
        new_url_input: String,
    },
    ConfirmDelete {
        task_id: String,
        file_name: String,
        delete_disk_file: bool,
    },
    ConfigureQueue {
        name_input: String,
        concurrency_input: String,
        start_time_input: String,
        stop_time_input: String,
        focused_field: usize,
    },
    Help,
}

pub struct App {
    pub client: IpcClient,
    pub current_tab: Tab,
    pub tasks: Vec<DownloadTask>,
    pub queues: Vec<Queue>,
    pub selected_index: usize,
    pub selected_segments: Vec<Segment>,
    pub daemon_status: Option<DaemonStatusInfo>,
    pub modal: ModalState,
    pub status_message: Option<(String, Instant)>,
    pub should_quit: bool,
    pub last_poll: Instant,
}

impl App {
    pub fn new(client: IpcClient) -> Self {
        Self {
            client,
            current_tab: Tab::All,
            tasks: Vec::new(),
            queues: Vec::new(),
            selected_index: 0,
            selected_segments: Vec::new(),
            daemon_status: None,
            modal: ModalState::None,
            status_message: None,
            should_quit: false,
            last_poll: Instant::now() - Duration::from_secs(10),
        }
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some((msg.into(), Instant::now()));
    }

    pub async fn refresh_data(&mut self) -> Result<()> {
        // Fetch tasks
        if let Ok(Response::Tasks(tasks)) = self.client.send_request(&Request::ListTasks).await {
            self.tasks = tasks;
        }

        // Fetch queues
        if let Ok(Response::Queues(queues)) = self.client.send_request(&Request::ListQueues).await {
            self.queues = queues;
        }

        // Fetch daemon status
        if let Ok(Response::DaemonStatus(status)) = self.client.send_request(&Request::GetDaemonStatus).await {
            self.daemon_status = Some(status);
        }

        // Fetch selected task segments
        let filtered_count = self.filtered_tasks().len();
        if filtered_count > 0 {
            if self.selected_index >= filtered_count {
                self.selected_index = filtered_count - 1;
            }
            let selected_task_id = self.filtered_tasks()[self.selected_index].id.clone();
            if let Ok(Response::TaskDetail { segments, .. }) = self
                .client
                .send_request(&Request::GetTaskDetails {
                    id: selected_task_id,
                })
                .await
            {
                self.selected_segments = segments;
            }
        } else {
            self.selected_segments.clear();
        }

        self.last_poll = Instant::now();
        Ok(())
    }

    pub fn filtered_tasks(&self) -> Vec<&DownloadTask> {
        match self.current_tab {
            Tab::All => self.tasks.iter().collect(),
            Tab::Downloading => self
                .tasks
                .iter()
                .filter(|t| t.status == TaskStatus::Downloading || t.status == TaskStatus::Queued)
                .collect(),
            Tab::Completed => self
                .tasks
                .iter()
                .filter(|t| t.status == TaskStatus::Completed)
                .collect(),
            _ => self.tasks.iter().collect(),
        }
    }

    pub fn select_next(&mut self) {
        let count = self.filtered_tasks().len();
        if count > 0 {
            self.selected_index = (self.selected_index + 1) % count;
        }
    }

    pub fn select_prev(&mut self) {
        let count = self.filtered_tasks().len();
        if count > 0 {
            if self.selected_index == 0 {
                self.selected_index = count - 1;
            } else {
                self.selected_index -= 1;
            }
        }
    }

    pub fn selected_task(&self) -> Option<&DownloadTask> {
        let filtered = self.filtered_tasks();
        filtered.get(self.selected_index).copied()
    }

    pub async fn toggle_pause_selected(&mut self) -> Result<()> {
        if let Some(task) = self.selected_task() {
            let task_id = task.id.clone();
            let name = task.file_name.clone();
            if task.status.is_active() {
                self.client.send_request(&Request::PauseTask { id: task_id }).await?;
                self.set_status(format!("Paused: {}", name));
            } else if task.status == TaskStatus::Paused || matches!(task.status, TaskStatus::Error(_)) {
                self.client.send_request(&Request::ResumeTask { id: task_id }).await?;
                self.set_status(format!("Resumed: {}", name));
            }
            self.refresh_data().await?;
        }
        Ok(())
    }

    pub fn open_add_modal(&mut self) {
        self.modal = ModalState::AddDownload {
            url_input: String::new(),
            dest_input: String::new(),
            queue_input: "default".to_string(),
            focused_field: 0,
        };
    }

    pub fn open_refresh_modal(&mut self) {
        if let Some(task) = self.selected_task() {
            self.modal = ModalState::RefreshLink {
                task_id: task.id.clone(),
                task_name: task.file_name.clone(),
                new_url_input: String::new(),
            };
        }
    }

    pub fn open_delete_modal(&mut self) {
        if let Some(task) = self.selected_task() {
            self.modal = ModalState::ConfirmDelete {
                task_id: task.id.clone(),
                file_name: task.file_name.clone(),
                delete_disk_file: false,
            };
        }
    }

    pub fn close_modal(&mut self) {
        self.modal = ModalState::None;
    }

    pub async fn submit_add_modal(&mut self) -> Result<()> {
        if let ModalState::AddDownload {
            ref url_input,
            ref dest_input,
            ref queue_input,
            ..
        } = self.modal
        {
            let url = url_input.trim();
            if !url.is_empty() {
                let dest = if dest_input.trim().is_empty() {
                    None
                } else {
                    Some(dest_input.trim().to_string())
                };
                let queue = if queue_input.trim().is_empty() {
                    None
                } else {
                    Some(queue_input.trim().to_string())
                };

                let resp = self
                    .client
                    .send_request(&Request::AddTask {
                        url: url.to_string(),
                        destination: dest,
                        queue,
                        connections: Some(8),
                        speed_limit: None,
                    })
                    .await?;

                match resp {
                    Response::TaskAdded(task) => {
                        self.set_status(format!("Added download: {}", task.file_name));
                    }
                    Response::Error(err) => {
                        self.set_status(format!("Error: {}", err));
                    }
                    _ => {}
                }
            }
        }
        self.close_modal();
        self.refresh_data().await?;
        Ok(())
    }

    pub async fn submit_refresh_modal(&mut self) -> Result<()> {
        if let ModalState::RefreshLink {
            ref task_id,
            ref new_url_input,
            ..
        } = self.modal
        {
            let url = new_url_input.trim();
            if !url.is_empty() {
                let resp = self
                    .client
                    .send_request(&Request::RefreshTaskUrl {
                        id: task_id.clone(),
                        new_url: url.to_string(),
                    })
                    .await?;

                match resp {
                    Response::Ok => {
                        self.set_status("Link refreshed successfully! Resuming download...");
                    }
                    Response::Error(err) => {
                        self.set_status(format!("Refresh error: {}", err));
                    }
                    _ => {}
                }
            }
        }
        self.close_modal();
        self.refresh_data().await?;
        Ok(())
    }

    pub async fn submit_delete_modal(&mut self) -> Result<()> {
        if let ModalState::ConfirmDelete {
            ref task_id,
            delete_disk_file,
            ..
        } = self.modal
        {
            let resp = self
                .client
                .send_request(&Request::DeleteTask {
                    id: task_id.clone(),
                    delete_file: delete_disk_file,
                })
                .await?;

            match resp {
                Response::Ok => {
                    self.set_status("Task deleted");
                }
                Response::Error(err) => {
                    self.set_status(format!("Delete error: {}", err));
                }
                _ => {}
            }
        }
        self.close_modal();
        self.refresh_data().await?;
        Ok(())
    }

    pub fn open_queue_modal(&mut self) {
        self.modal = ModalState::ConfigureQueue {
            name_input: "night".to_string(),
            concurrency_input: "3".to_string(),
            start_time_input: "02:00".to_string(),
            stop_time_input: "07:00".to_string(),
            focused_field: 0,
        };
    }

    pub async fn submit_queue_modal(&mut self) -> Result<()> {
        if let ModalState::ConfigureQueue {
            ref name_input,
            ref concurrency_input,
            ref start_time_input,
            ref stop_time_input,
            ..
        } = self.modal
        {
            let name = name_input.trim();
            if !name.is_empty() {
                let concurrency = concurrency_input.trim().parse::<usize>().unwrap_or(3);
                let _ = self
                    .client
                    .send_request(&Request::CreateQueue {
                        name: name.to_string(),
                        max_concurrent: concurrency,
                        speed_limit: None,
                    })
                    .await;

                let start = if start_time_input.trim().is_empty() {
                    None
                } else {
                    Some(start_time_input.trim().to_string())
                };
                let stop = if stop_time_input.trim().is_empty() {
                    None
                } else {
                    Some(stop_time_input.trim().to_string())
                };

                let _ = self
                    .client
                    .send_request(&Request::ConfigureSchedule {
                        queue_name: name.to_string(),
                        start_time: start,
                        stop_time: stop,
                        on_complete: siphon_core::models::OnCompleteAction::None,
                    })
                    .await;

                self.set_status(format!("Configured queue '{}'", name));
            }
        }
        self.close_modal();
        self.refresh_data().await?;
        Ok(())
    }
}
