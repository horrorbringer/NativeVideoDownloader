use std::path::{Path, PathBuf};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Pool, Row, Sqlite};
use std::str::FromStr;
use tracing::info;
use uuid::Uuid;

use crate::downloader::job::DownloadJob;
use crate::error::Result;
use crate::models::DownloadStatus;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct HistoryRecord {
    pub id: Uuid,
    pub url: String,
    pub title: String,
    pub filename: String,
    pub output_path: String,
    pub status: String,
    pub total_size: Option<u64>,
    pub downloaded_size: u64,
    pub error: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

pub struct Database {
    pool: Pool<Sqlite>,
}

impl Database {
    pub async fn init(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let db_str = db_path.to_string_lossy();
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_str))?
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;

        // Initialize schema
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS downloads (
                id TEXT PRIMARY KEY,
                url TEXT NOT NULL,
                title TEXT NOT NULL,
                filename TEXT NOT NULL,
                output_path TEXT NOT NULL,
                status TEXT NOT NULL,
                total_size INTEGER,
                downloaded_size INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                created_at TEXT NOT NULL,
                completed_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_downloads_status ON downloads(status);
            CREATE INDEX IF NOT EXISTS idx_downloads_created_at ON downloads(created_at);

            CREATE TABLE IF NOT EXISTS app_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )
        .execute(&pool)
        .await?;

        info!("SQLite database initialized at: {:?}", db_path);

        // Auto-reconcile existing files on disk that may have failed to mark completed due to past schema mismatch
        if let Ok(rows) = sqlx::query("SELECT id, output_path FROM downloads WHERE status != 'Completed'")
            .fetch_all(&pool)
            .await
        {
            for row in rows {
                let id_raw: String = row.get("id");
                let path_raw: String = row.get("output_path");
                let p = Path::new(&path_raw);
                if p.exists() {
                    if let Ok(meta) = std::fs::metadata(p) {
                        let len = meta.len();
                        if len > 0 {
                            info!("Auto-reconciling completed file in history: {:?}", p);
                            let now_str = chrono_or_now();
                            let _ = sqlx::query(
                                "UPDATE downloads SET status = 'Completed', downloaded_size = ?, total_size = ?, completed_at = coalesce(completed_at, ?) WHERE id = ?"
                            )
                            .bind(len as i64)
                            .bind(len as i64)
                            .bind(&now_str)
                            .bind(&id_raw)
                            .execute(&pool)
                            .await;
                        }
                    }
                }
            }
        }

        Ok(Self { pool })
    }

    pub fn default_db_path() -> PathBuf {
        if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home)
                .join(".native_video_downloader")
                .join("downloads.db")
        } else {
            PathBuf::from("downloads.db")
        }
    }

    pub async fn upsert_job(&self, job: &DownloadJob) -> Result<()> {
        let id_str = job.id.to_string();
        let path_str = job.output_path.to_string_lossy().to_string();
        let filename = job
            .output_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&job.title)
            .to_string();
        let status_str = job.status.as_str();
        let total_size_val = job.total_bytes.map(|b| b as i64);
        let downloaded_val = job.downloaded_bytes as i64;
        let now_str = chrono_or_now();

        sqlx::query(
            r#"
            INSERT INTO downloads (id, url, title, filename, output_path, status, total_size, downloaded_size, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                status = excluded.status,
                downloaded_size = excluded.downloaded_size,
                total_size = coalesce(excluded.total_size, downloads.total_size)
            "#,
        )
        .bind(id_str)
        .bind(&job.url)
        .bind(&job.title)
        .bind(filename)
        .bind(path_str)
        .bind(status_str)
        .bind(total_size_val)
        .bind(downloaded_val)
        .bind(now_str)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_completed(&self, id: Uuid, final_size: u64, final_path: Option<&std::path::Path>) -> Result<()> {
        let id_str = id.to_string();
        let now_str = chrono_or_now();

        if let Some(path) = final_path {
            let path_str = path.to_string_lossy().to_string();
            sqlx::query(
                r#"
                UPDATE downloads
                SET status = 'Completed',
                    downloaded_size = ?,
                    total_size = ?,
                    completed_at = ?,
                    output_path = ?
                WHERE id = ?
                "#,
            )
            .bind(final_size as i64)
            .bind(final_size as i64)
            .bind(now_str)
            .bind(path_str)
            .bind(id_str)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query(
                r#"
                UPDATE downloads
                SET status = 'Completed',
                    downloaded_size = ?,
                    total_size = ?,
                    completed_at = ?
                WHERE id = ?
                "#,
            )
            .bind(final_size as i64)
            .bind(final_size as i64)
            .bind(now_str)
            .bind(id_str)
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    pub async fn mark_failed(&self, id: Uuid, error_msg: &str) -> Result<()> {
        let id_str = id.to_string();
        sqlx::query(
            r#"
            UPDATE downloads
            SET status = 'Failed',
                error = ?
            WHERE id = ?
            "#,
        )
        .bind(error_msg)
        .bind(id_str)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_cancelled(&self, id: Uuid) -> Result<()> {
        let id_str = id.to_string();
        sqlx::query(
            r#"
            UPDATE downloads
            SET status = 'Cancelled'
            WHERE id = ?
            "#,
        )
        .bind(id_str)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_history(&self, search: Option<&str>) -> Result<Vec<HistoryRecord>> {
        let query_str = match search {
            Some(s) if !s.trim().is_empty() => {
                let pattern = format!("%{}%", s.trim());
                sqlx::query(
                    r#"
                    SELECT id, url, title, filename, output_path, status, total_size, downloaded_size, error, created_at, completed_at
                    FROM downloads
                    WHERE title LIKE ? OR filename LIKE ? OR url LIKE ?
                    ORDER BY created_at DESC
                    "#,
                )
                .bind(pattern.clone())
                .bind(pattern.clone())
                .bind(pattern)
            }
            _ => sqlx::query(
                r#"
                SELECT id, url, title, filename, output_path, status, total_size, downloaded_size, error, created_at, completed_at
                FROM downloads
                ORDER BY created_at DESC
                "#,
            ),
        };

        let rows = query_str.fetch_all(&self.pool).await?;
        let records = rows
            .into_iter()
            .filter_map(|r| {
                let id_raw: String = r.get("id");
                let id = Uuid::parse_str(&id_raw).ok()?;
                let url: String = r.get("url");
                let title: String = r.get("title");
                let filename: String = r.get("filename");
                let output_path: String = r.get("output_path");
                let status: String = r.get("status");
                let total_size: Option<i64> = r.get("total_size");
                let downloaded_size: i64 = r.get("downloaded_size");
                let error: Option<String> = r.get("error");
                let created_at: String = r.get("created_at");
                let completed_at: Option<String> = r.get("completed_at");

                Some(HistoryRecord {
                    id,
                    url,
                    title,
                    filename,
                    output_path,
                    status,
                    total_size: total_size.map(|s| s.max(0) as u64),
                    downloaded_size: downloaded_size.max(0) as u64,
                    error,
                    created_at,
                    completed_at,
                })
            })
            .collect();

        Ok(records)
    }

    pub async fn delete_record(&self, id: Uuid) -> Result<()> {
        let id_str = id.to_string();
        sqlx::query("DELETE FROM downloads WHERE id = ?")
            .bind(id_str)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn clear_all_history(&self) -> Result<()> {
        sqlx::query("DELETE FROM downloads WHERE status = 'Completed' OR status = 'Failed' OR status = 'Cancelled'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Finds incomplete downloads from previous session to restore on startup
    pub async fn get_unfinished_jobs(&self) -> Result<Vec<DownloadJob>> {
        let rows = sqlx::query(
            r#"
            SELECT id, url, title, output_path, status, total_size, downloaded_size
            FROM downloads
            WHERE status IN ('Downloading', 'Queued', 'Paused')
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut jobs = Vec::new();
        for r in rows {
            let id_raw: String = r.get("id");
            if let Ok(id) = Uuid::parse_str(&id_raw) {
                let url: String = r.get("url");
                let title: String = r.get("title");
                let path_str: String = r.get("output_path");
                let total_size: Option<i64> = r.get("total_size");
                let downloaded: i64 = r.get("downloaded_size");

                let is_extractor = crate::downloader::is_streaming_platform(&url);
                let mut job = DownloadJob::new(
                    url,
                    title,
                    PathBuf::from(path_str),
                    total_size.map(|s| s.max(0) as u64),
                    is_extractor,
                    false,
                    None,
                    true,
                    None,
                    true,
                    None,
                    None,
                    None,
                    false,
                );
                job.id = id;
                job.status = DownloadStatus::Paused; // Restore in paused state
                job.downloaded_bytes = downloaded.max(0) as u64;
                jobs.push(job);
            }
        }

        Ok(jobs)
    }

    pub async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let row = sqlx::query_scalar::<_, String>(
            "SELECT value FROM app_settings WHERE key = ?"
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row)
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO app_settings (key, value)
            VALUES (?, ?)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            "#,
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

pub fn chrono_or_now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

pub fn format_history_date(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(sec_str) = trimmed.split('.').next() {
        if let Ok(secs) = sec_str.parse::<i64>() {
            if secs > 100_000_000 {
                if let Some(dt) = chrono::DateTime::from_timestamp(secs, 0) {
                    let local: chrono::DateTime<chrono::Local> = chrono::DateTime::from(dt);
                    return local.format("%Y-%m-%d %H:%M").to_string();
                }
            }
        }
    }
    trimmed.to_string()
}
