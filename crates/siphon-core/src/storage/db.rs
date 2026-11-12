use crate::models::{DownloadTask, Queue, Segment, TaskStatus};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create DB directory: {:?}", parent))?;
        }

        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open SQLite DB at {:?}", db_path))?;

        // Enable WAL mode & foreign keys for high concurrent performance & safety
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        db.init_schema()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS queues (
                name TEXT PRIMARY KEY,
                max_concurrent INTEGER NOT NULL DEFAULT 3,
                speed_limit_bps INTEGER,
                is_active INTEGER NOT NULL DEFAULT 1,
                schedule_start TEXT,
                schedule_end TEXT
            );

            INSERT OR IGNORE INTO queues (name, max_concurrent, is_active)
            VALUES ('default', 3, 1);

            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                url TEXT NOT NULL,
                file_name TEXT NOT NULL,
                destination_path TEXT NOT NULL,
                total_bytes INTEGER,
                downloaded_bytes INTEGER NOT NULL DEFAULT 0,
                speed_bps INTEGER NOT NULL DEFAULT 0,
                eta_seconds INTEGER,
                supports_range INTEGER NOT NULL DEFAULT 0,
                connections INTEGER NOT NULL DEFAULT 8,
                queue_name TEXT NOT NULL REFERENCES queues(name) ON DELETE RESTRICT,
                status TEXT NOT NULL,
                error_message TEXT,
                category TEXT,
                etag TEXT,
                created_at TEXT NOT NULL,
                completed_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_tasks_queue ON tasks(queue_name);
            CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);

            CREATE TABLE IF NOT EXISTS segments (
                task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                segment_id INTEGER NOT NULL,
                start_byte INTEGER NOT NULL,
                current_byte INTEGER NOT NULL,
                end_byte INTEGER NOT NULL,
                active INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (task_id, segment_id)
            );

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;

        Ok(())
    }

    pub fn insert_task(&self, task: &DownloadTask) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let status_json = serde_json::to_string(&task.status)?;
        let created_at_str = task.created_at.to_rfc3339();
        let completed_at_str = task.completed_at.map(|d| d.to_rfc3339());

        conn.execute(
            r#"
            INSERT INTO tasks (
                id, url, file_name, destination_path, total_bytes, downloaded_bytes,
                speed_bps, eta_seconds, supports_range, connections, queue_name,
                status, error_message, category, etag, created_at, completed_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
            "#,
            params![
                task.id,
                task.url,
                task.file_name,
                task.destination_path.to_string_lossy(),
                task.total_bytes,
                task.downloaded_bytes,
                task.speed_bps,
                task.eta_seconds,
                if task.supports_range { 1 } else { 0 },
                task.connections,
                task.queue_name,
                status_json,
                task.error_message,
                task.category,
                task.etag,
                created_at_str,
                completed_at_str,
            ],
        )?;

        Ok(())
    }

    pub fn update_task(&self, task: &DownloadTask) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let status_json = serde_json::to_string(&task.status)?;
        let completed_at_str = task.completed_at.map(|d| d.to_rfc3339());

        conn.execute(
            r#"
            UPDATE tasks SET
                url = ?2,
                file_name = ?3,
                destination_path = ?4,
                total_bytes = ?5,
                downloaded_bytes = ?6,
                speed_bps = ?7,
                eta_seconds = ?8,
                supports_range = ?9,
                connections = ?10,
                queue_name = ?11,
                status = ?12,
                error_message = ?13,
                category = ?14,
                etag = ?15,
                completed_at = ?16
            WHERE id = ?1
            "#,
            params![
                task.id,
                task.url,
                task.file_name,
                task.destination_path.to_string_lossy(),
                task.total_bytes,
                task.downloaded_bytes,
                task.speed_bps,
                task.eta_seconds,
                if task.supports_range { 1 } else { 0 },
                task.connections,
                task.queue_name,
                status_json,
                task.error_message,
                task.category,
                task.etag,
                completed_at_str,
            ],
        )?;

        Ok(())
    }

    pub fn update_task_url(&self, task_id: &str, new_url: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let queued_status = serde_json::to_string(&TaskStatus::Queued)?;
        conn.execute(
            "UPDATE tasks SET url = ?2, status = ?3, error_message = NULL WHERE id = ?1",
            params![task_id, new_url, queued_status],
        )?;
        Ok(())
    }

    pub fn update_task_status(&self, task_id: &str, status: TaskStatus) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let status_json = serde_json::to_string(&status)?;
        let error_msg = if let TaskStatus::Error(ref err) = status {
            Some(err.clone())
        } else {
            None
        };
        let completed_at_str = if status == TaskStatus::Completed {
            Some(Utc::now().to_rfc3339())
        } else {
            None
        };

        conn.execute(
            r#"
            UPDATE tasks SET
                status = ?2,
                error_message = COALESCE(?3, error_message),
                completed_at = COALESCE(?4, completed_at)
            WHERE id = ?1
            "#,
            params![task_id, status_json, error_msg, completed_at_str],
        )?;
        Ok(())
    }

    pub fn update_task_progress(
        &self,
        task_id: &str,
        downloaded_bytes: u64,
        speed_bps: u64,
        eta_seconds: Option<u64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tasks SET downloaded_bytes = ?2, speed_bps = ?3, eta_seconds = ?4 WHERE id = ?1",
            params![task_id, downloaded_bytes, speed_bps, eta_seconds],
        )?;
        Ok(())
    }

    pub fn resolve_task_id(&self, id_or_prefix: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM tasks WHERE id LIKE ?1 || '%' LIMIT 2")?;
        let rows: Vec<String> = stmt
            .query_map(params![id_or_prefix], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        if rows.is_empty() {
            Ok(None)
        } else if rows.len() == 1 {
            Ok(Some(rows[0].clone()))
        } else if rows.contains(&id_or_prefix.to_string()) {
            Ok(Some(id_or_prefix.to_string()))
        } else {
            anyhow::bail!("Ambiguous task ID prefix '{}'", id_or_prefix);
        }
    }

    pub fn get_task(&self, id_or_prefix: &str) -> Result<Option<DownloadTask>> {
        let resolved_id = match self.resolve_task_id(id_or_prefix)? {
            Some(id) => id,
            None => return Ok(None),
        };

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, url, file_name, destination_path, total_bytes, downloaded_bytes,
                   speed_bps, eta_seconds, supports_range, connections, queue_name,
                   status, error_message, category, etag, created_at, completed_at
            FROM tasks WHERE id = ?1
            "#,
        )?;

        let mut rows = stmt.query(params![resolved_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Self::row_to_task(&row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_tasks(&self) -> Result<Vec<DownloadTask>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, url, file_name, destination_path, total_bytes, downloaded_bytes,
                   speed_bps, eta_seconds, supports_range, connections, queue_name,
                   status, error_message, category, etag, created_at, completed_at
            FROM tasks ORDER BY created_at DESC
            "#,
        )?;

        let rows = stmt.query_map([], |row| Self::row_to_task(&row))?;
        let mut tasks = Vec::new();
        for task in rows {
            tasks.push(task?);
        }
        Ok(tasks)
    }

    pub fn delete_task(&self, id_or_prefix: &str) -> Result<()> {
        let resolved_id = match self.resolve_task_id(id_or_prefix)? {
            Some(id) => id,
            None => return Ok(()),
        };
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM segments WHERE task_id = ?1", params![resolved_id])?;
        conn.execute("DELETE FROM tasks WHERE id = ?1", params![resolved_id])?;
        Ok(())
    }

    pub fn save_segments(&self, task_id: &str, segments: &[Segment]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        tx.execute("DELETE FROM segments WHERE task_id = ?1", params![task_id])?;

        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO segments (task_id, segment_id, start_byte, current_byte, end_byte, active)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
            )?;

            for seg in segments {
                stmt.execute(params![
                    task_id,
                    seg.id,
                    seg.start_byte,
                    seg.current_byte,
                    seg.end_byte,
                    if seg.active { 1 } else { 0 },
                ])?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    pub fn get_segments(&self, task_id: &str) -> Result<Vec<Segment>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT segment_id, start_byte, current_byte, end_byte, active
            FROM segments WHERE task_id = ?1 ORDER BY segment_id ASC
            "#,
        )?;

        let rows = stmt.query_map(params![task_id], |row| {
            let active_int: i32 = row.get(4)?;
            Ok(Segment {
                id: row.get(0)?,
                task_id: task_id.to_string(),
                start_byte: row.get(1)?,
                current_byte: row.get(2)?,
                end_byte: row.get(3)?,
                active: active_int != 0,
            })
        })?;

        let mut segments = Vec::new();
        for seg in rows {
            segments.push(seg?);
        }
        Ok(segments)
    }

    pub fn upsert_queue(&self, queue: &Queue) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO queues (name, max_concurrent, speed_limit_bps, is_active, schedule_start, schedule_end)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(name) DO UPDATE SET
                max_concurrent = excluded.max_concurrent,
                speed_limit_bps = excluded.speed_limit_bps,
                is_active = excluded.is_active,
                schedule_start = excluded.schedule_start,
                schedule_end = excluded.schedule_end
            "#,
            params![
                queue.name,
                queue.max_concurrent,
                queue.speed_limit_bps,
                if queue.is_active { 1 } else { 0 },
                queue.schedule_start,
                queue.schedule_end,
            ],
        )?;
        Ok(())
    }

    pub fn list_queues(&self) -> Result<Vec<Queue>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT name, max_concurrent, speed_limit_bps, is_active, schedule_start, schedule_end
            FROM queues ORDER BY name ASC
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            let is_active_int: i32 = row.get(3)?;
            Ok(Queue {
                name: row.get(0)?,
                max_concurrent: row.get(1)?,
                speed_limit_bps: row.get(2)?,
                is_active: is_active_int != 0,
                schedule_start: row.get(4)?,
                schedule_end: row.get(5)?,
            })
        })?;

        let mut queues = Vec::new();
        for q in rows {
            queues.push(q?);
        }
        Ok(queues)
    }

    fn row_to_task(row: &rusqlite::Row) -> rusqlite::Result<DownloadTask> {
        let path_str: String = row.get(3)?;
        let status_json: String = row.get(11)?;
        let status: TaskStatus = serde_json::from_str(&status_json).unwrap_or(TaskStatus::Queued);
        let supports_range_int: i32 = row.get(8)?;
        let created_at_str: String = row.get(15)?;
        let completed_at_str: Option<String> = row.get(16)?;

        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        let completed_at = completed_at_str.and_then(|s| {
            DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&Utc))
                .ok()
        });

        Ok(DownloadTask {
            id: row.get(0)?,
            url: row.get(1)?,
            file_name: row.get(2)?,
            destination_path: PathBuf::from(path_str),
            total_bytes: row.get(4)?,
            downloaded_bytes: row.get(5)?,
            speed_bps: row.get(6)?,
            eta_seconds: row.get(7)?,
            supports_range: supports_range_int != 0,
            connections: row.get(9)?,
            queue_name: row.get(10)?,
            status,
            error_message: row.get(12)?,
            category: row.get(13)?,
            etag: row.get(14)?,
            created_at,
            completed_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_db_lifecycle_and_link_refresh() -> Result<()> {
        let db = Database::open_in_memory()?;

        let task = DownloadTask {
            id: "task-123".to_string(),
            url: "https://example.com/expired-link?token=abc".to_string(),
            file_name: "test.iso".to_string(),
            destination_path: PathBuf::from("/tmp/test.iso"),
            total_bytes: Some(1024 * 1024 * 10),
            downloaded_bytes: 1024 * 1024 * 5,
            speed_bps: 0,
            eta_seconds: None,
            supports_range: true,
            connections: 4,
            queue_name: "default".to_string(),
            status: TaskStatus::Downloading,
            error_message: None,
            category: Some("Archives".to_string()),
            etag: Some("\"etag123\"".to_string()),
            created_at: Utc::now(),
            completed_at: None,
        };

        db.insert_task(&task)?;

        let loaded = db.get_task("task-123")?.expect("Task should exist");
        assert_eq!(loaded.id, "task-123");
        assert_eq!(loaded.status, TaskStatus::Downloading);

        // Save segments
        let segments = vec![
            Segment {
                id: 0,
                task_id: "task-123".to_string(),
                start_byte: 0,
                current_byte: 2500,
                end_byte: 5000,
                active: true,
            },
            Segment {
                id: 1,
                task_id: "task-123".to_string(),
                start_byte: 5001,
                current_byte: 6000,
                end_byte: 10000,
                active: true,
            },
        ];
        db.save_segments("task-123", &segments)?;

        let loaded_segs = db.get_segments("task-123")?;
        assert_eq!(loaded_segs.len(), 2);
        assert_eq!(loaded_segs[0].current_byte, 2500);

        // Mark as Expired
        db.update_task_status("task-123", TaskStatus::Expired)?;
        let expired_task = db.get_task("task-123")?.unwrap();
        assert_eq!(expired_task.status, TaskStatus::Expired);

        // IDM / ADM Link Refresh: update URL with fresh link!
        db.update_task_url("task-123", "https://example.com/refreshed-link?token=xyz")?;
        let refreshed_task = db.get_task("task-123")?.unwrap();
        assert_eq!(
            refreshed_task.url,
            "https://example.com/refreshed-link?token=xyz"
        );
        assert_eq!(refreshed_task.status, TaskStatus::Queued);

        // Segments are preserved intact!
        let preserved_segs = db.get_segments("task-123")?;
        assert_eq!(preserved_segs.len(), 2);
        assert_eq!(preserved_segs[0].current_byte, 2500);

        Ok(())
    }
}
