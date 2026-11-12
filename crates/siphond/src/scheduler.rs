use crate::queue::QueueManager;
use chrono::Local;
use siphon_core::models::OnCompleteAction;
use siphon_core::storage::Database;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

pub struct Scheduler {
    db: Database,
    queue_mgr: QueueManager,
}

impl Scheduler {
    pub fn new(db: Database, queue_mgr: QueueManager) -> Self {
        Self { db, queue_mgr }
    }

    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(10));
            loop {
                ticker.tick().await;
                self.check_schedules().await;
            }
        });
    }

    async fn check_schedules(&self) {
        let queues = match self.db.list_queues() {
            Ok(q) => q,
            Err(_) => return,
        };

        let now_hm = Local::now().format("%H:%M").to_string();

        for mut queue in queues {
            if let (Some(start), Some(end)) = (&queue.schedule_start, &queue.schedule_end) {
                let should_be_active = is_in_time_window(&now_hm, start, end);

                if should_be_active != queue.is_active {
                    info!(
                        "Scheduler: Queue '{}' active state changing from {} to {} (Window: {} - {}, Now: {})",
                        queue.name, queue.is_active, should_be_active, start, end, now_hm
                    );
                    queue.is_active = should_be_active;
                    let _ = self.db.upsert_queue(&queue);

                    if should_be_active {
                        self.queue_mgr.notify_tick();
                    }
                }
            }
        }
    }

    #[allow(dead_code)]
    pub async fn execute_on_complete_action(&self, action: &OnCompleteAction, task_summary: &str) {
        match action {
            OnCompleteAction::None => {}
            OnCompleteAction::Shutdown => {
                info!("Executing system shutdown as requested by queue completion");
                let _ = std::process::Command::new("systemctl")
                    .arg("poweroff")
                    .spawn();
            }
            OnCompleteAction::Sleep => {
                info!("Executing system sleep/suspend as requested by queue completion");
                let _ = std::process::Command::new("systemctl")
                    .arg("suspend")
                    .spawn();
            }
            OnCompleteAction::CustomScript(script_path) => {
                info!("Executing completion script: {}", script_path);
                let _ = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(script_path)
                    .env("SIPHON_TASK", task_summary)
                    .spawn();
            }
            OnCompleteAction::Webhook(url) => {
                info!("Sending completion webhook to: {}", url);
                let client = reqwest::Client::new();
                let payload = serde_json::json!({
                    "event": "download_completed",
                    "details": task_summary,
                });
                let _ = client.post(url).json(&payload).send().await;
            }
        }
    }
}

pub fn is_in_time_window(current_hm: &str, start_hm: &str, end_hm: &str) -> bool {
    if start_hm <= end_hm {
        // Same-day window (e.g. 09:00 to 17:00)
        current_hm >= start_hm && current_hm < end_hm
    } else {
        // Overnight window (e.g. 23:00 to 07:00)
        current_hm >= start_hm || current_hm < end_hm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_window_checks() {
        // Normal daytime window: 09:00 to 17:00
        assert!(is_in_time_window("12:00", "09:00", "17:00"));
        assert!(is_in_time_window("09:00", "09:00", "17:00"));
        assert!(!is_in_time_window("17:00", "09:00", "17:00"));
        assert!(!is_in_time_window("08:59", "09:00", "17:00"));

        // Overnight window: 02:00 to 07:00
        assert!(is_in_time_window("03:30", "02:00", "07:00"));
        assert!(!is_in_time_window("08:00", "02:00", "07:00"));

        // Overnight wrap-around window: 23:00 to 06:00
        assert!(is_in_time_window("23:30", "23:00", "06:00"));
        assert!(is_in_time_window("04:00", "23:00", "06:00"));
        assert!(!is_in_time_window("12:00", "23:00", "06:00"));
    }
}
