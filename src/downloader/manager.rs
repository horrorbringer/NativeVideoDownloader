use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::database::Database;
use crate::downloader::job::DownloadJob;
use crate::downloader::queue::DownloadQueue;
use crate::downloader::retry::RetryPolicy;
use crate::error::AppError;
use crate::filesystem::{part_path_for, validate_destination_path};
use crate::models::DownloadStatus;
use crate::network::NetworkClient;

pub type StatusUpdateCallback = Arc<dyn Fn() + Send + Sync + 'static>;

pub struct DownloadManager {
    queue: Arc<Mutex<DownloadQueue>>,
    semaphore: Arc<Semaphore>,
    network_client: Arc<NetworkClient>,
    retry_policy: RetryPolicy,
    db: Arc<Database>,
    on_update: Mutex<Option<StatusUpdateCallback>>,
}

impl DownloadManager {
    pub fn new(max_concurrent: usize, db: Arc<Database>) -> Self {
        Self {
            queue: Arc::new(Mutex::new(DownloadQueue::new())),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            network_client: Arc::new(NetworkClient::new()),
            retry_policy: RetryPolicy::default(),
            db,
            on_update: Mutex::new(None),
        }
    }

    #[allow(dead_code)]
    pub fn database(&self) -> Arc<Database> {
        self.db.clone()
    }

    pub async fn set_update_listener(&self, callback: impl Fn() + Send + Sync + 'static) {
        let mut guard = self.on_update.lock().await;
        *guard = Some(Arc::new(callback));
    }

    async fn notify_update(&self) {
        let guard = self.on_update.lock().await;
        if let Some(cb) = guard.as_ref() {
            cb();
        }
    }

    pub async fn get_jobs_snapshot(&self) -> Vec<DownloadJob> {
        let queue = self.queue.lock().await;
        queue.all_jobs().to_vec()
    }

    /// Restore unfinished jobs from previous session (Crash Recovery)
    pub async fn restore_unfinished_jobs(&self) -> Result<usize, AppError> {
        let unfinished = self.db.get_unfinished_jobs().await?;
        let count = unfinished.len();
        if count == 0 {
            return Ok(0);
        }

        let mut queue = self.queue.lock().await;
        for mut job in unfinished {
            // Check if partial file exists on disk
            let part_path = part_path_for(&job.output_path);
            if part_path.exists() {
                if let Ok(metadata) = tokio::fs::metadata(&part_path).await {
                    job.downloaded_bytes = metadata.len();
                    if let Some(total) = job.total_bytes {
                        if total > 0 {
                            job.progress_ratio = (job.downloaded_bytes as f32 / total as f32).min(1.0);
                        }
                    }
                }
            }
            job.status = DownloadStatus::Paused; // Ready for user to resume
            queue.add_job(job);
        }
        drop(queue);

        info!("Crash recovery: restored {} unfinished downloads to queue", count);
        self.notify_update().await;
        Ok(count)
    }

    pub async fn add_download(
        self: &Arc<Self>,
        url: String,
        title: String,
        output_dir: &PathBuf,
        total_bytes: Option<u64>,
        is_extractor: bool,
        is_audio_only: bool,
        quality: Option<String>,
    ) -> Result<Uuid, AppError> {
        let destination = validate_destination_path(output_dir, &title)?;
        let job = DownloadJob::new(
            url,
            title,
            destination,
            total_bytes,
            is_extractor,
            is_audio_only,
            quality,
        );
        let id = job.id;

        // Persist to database
        if let Err(err) = self.db.upsert_job(&job).await {
            warn!("Failed to persist new job {} to database: {}", id, err);
        }

        {
            let mut queue = self.queue.lock().await;
            queue.add_job(job);
        }

        self.notify_update().await;
        self.spawn_worker(id).await;

        Ok(id)
    }

    pub async fn pause_job(&self, id: Uuid) {
        let mut queue = self.queue.lock().await;
        if let Some(job) = queue.get_job_mut(id) {
            if job.status == DownloadStatus::Downloading || job.status == DownloadStatus::Queued {
                info!("Pausing download job {}", id);
                if let Some(token) = job.cancel_token.take() {
                    token.cancel();
                }
                job.status = DownloadStatus::Paused;
                job.speed_bytes_sec = 0.0;
                job.eta_seconds = None;

                let _ = self.db.upsert_job(job).await;
            }
        }
        drop(queue);
        self.notify_update().await;
    }

    pub async fn resume_job(self: &Arc<Self>, id: Uuid) {
        {
            let mut queue = self.queue.lock().await;
            if let Some(job) = queue.get_job_mut(id) {
                if job.status == DownloadStatus::Paused || matches!(job.status, DownloadStatus::Failed(_)) {
                    info!("Resuming download job {}", id);
                    job.status = DownloadStatus::Queued;
                    let _ = self.db.upsert_job(job).await;
                }
            }
        }
        self.notify_update().await;
        self.spawn_worker(id).await;
    }

    pub async fn cancel_job(&self, id: Uuid) {
        let mut queue = self.queue.lock().await;
        if let Some(job) = queue.get_job_mut(id) {
            info!("Cancelling download job {}", id);
            if let Some(token) = job.cancel_token.take() {
                token.cancel();
            }
            job.status = DownloadStatus::Cancelled;
            job.speed_bytes_sec = 0.0;
            job.eta_seconds = None;

            let _ = self.db.mark_cancelled(id).await;
        }
        drop(queue);
        self.notify_update().await;
    }

    pub async fn pause_all(&self) {
        let ids: Vec<Uuid> = {
            let queue = self.queue.lock().await;
            queue
                .all_jobs()
                .iter()
                .filter(|j| j.status == DownloadStatus::Downloading || j.status == DownloadStatus::Queued)
                .map(|j| j.id)
                .collect()
        };
        for id in ids {
            self.pause_job(id).await;
        }
    }

    pub async fn resume_all(self: &Arc<Self>) {
        let ids: Vec<Uuid> = {
            let queue = self.queue.lock().await;
            queue
                .all_jobs()
                .iter()
                .filter(|j| j.status == DownloadStatus::Paused)
                .map(|j| j.id)
                .collect()
        };
        for id in ids {
            self.resume_job(id).await;
        }
    }

    pub async fn clear_completed(&self) {
        let mut queue = self.queue.lock().await;
        queue.clear_completed();
        drop(queue);
        self.notify_update().await;
    }

    async fn spawn_worker(self: &Arc<Self>, id: Uuid) {
        let manager = self.clone();

        tokio::spawn(async move {
            // Acquire permit from bounded concurrency semaphore
            let _permit = match manager.semaphore.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => return,
            };

            // Check if job is still in Queued status (it might have been cancelled while waiting)
            let (url, destination, _total_bytes, is_extractor, is_audio_only, quality) = {
                let mut queue = manager.queue.lock().await;
                let job = match queue.get_job_mut(id) {
                    Some(j) => j,
                    None => return,
                };

                if job.status != DownloadStatus::Queued {
                    return;
                }

                let cancel_token = CancellationToken::new();
                job.cancel_token = Some(cancel_token.clone());
                job.status = DownloadStatus::Downloading;
                (
                    job.url.clone(),
                    job.output_path.clone(),
                    job.total_bytes,
                    job.is_extractor,
                    job.is_audio_only,
                    job.quality.clone(),
                )
            };

            manager.notify_update().await;

            let mut retry_count = 0;
            loop {
                let token = {
                    let queue = manager.queue.lock().await;
                    queue.get_job(id).and_then(|j| j.cancel_token.clone())
                };

                let cancel_token = match token {
                    Some(t) => t,
                    None => CancellationToken::new(),
                };

                let last_notify_stream = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
                let last_notify_net = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));

                let mgr_progress = manager.clone();
                let download_res = if is_extractor {
                    let last_time = last_notify_stream.clone();
                    crate::downloader::extractor::download_stream(
                        &url,
                        &destination,
                        is_audio_only,
                        quality.as_deref(),
                        cancel_token,
                        move |progress| {
                            let mgr = mgr_progress.clone();
                            let last_t = last_time.clone();
                            tokio::spawn(async move {
                                let mut queue = mgr.queue.lock().await;
                                if let Some(j) = queue.get_job_mut(id) {
                                    if j.status == DownloadStatus::Downloading {
                                        j.update_progress(progress);
                                    }
                                }
                                drop(queue);

                                let should_notify = {
                                    let mut guard = last_t.lock().unwrap();
                                    if guard.elapsed() >= std::time::Duration::from_millis(60) {
                                        *guard = std::time::Instant::now();
                                        true
                                    } else {
                                        false
                                    }
                                };
                                if should_notify {
                                    mgr.notify_update().await;
                                }
                            });
                        },
                    )
                    .await
                    .map(|resolved_path| {
                        let mgr_res = manager.clone();
                        tokio::spawn(async move {
                            let mut queue = mgr_res.queue.lock().await;
                            if let Some(j) = queue.get_job_mut(id) {
                                j.output_path = resolved_path;
                            }
                        });
                        ()
                    })
                } else {
                    let last_time = last_notify_net.clone();
                    manager
                        .network_client
                        .download_file(
                            &url,
                            &destination,
                            cancel_token,
                            true, // Preserve .part file on cancel/pause for resume
                            move |progress| {
                                let mgr = mgr_progress.clone();
                                let last_t = last_time.clone();
                                tokio::spawn(async move {
                                    let mut queue = mgr.queue.lock().await;
                                    if let Some(j) = queue.get_job_mut(id) {
                                        if j.status == DownloadStatus::Downloading {
                                            j.update_progress(progress);
                                        }
                                    }
                                    drop(queue);

                                    let should_notify = {
                                        let mut guard = last_t.lock().unwrap();
                                        if guard.elapsed() >= std::time::Duration::from_millis(60) {
                                            *guard = std::time::Instant::now();
                                            true
                                        } else {
                                            false
                                        }
                                    };
                                    if should_notify {
                                        mgr.notify_update().await;
                                    }
                                });
                            },
                        )
                        .await
                };

                match download_res {
                    Ok(()) => {
                        let (final_size, out_path) = {
                            let mut queue = manager.queue.lock().await;
                            if let Some(j) = queue.get_job_mut(id) {
                                (j.downloaded_bytes, j.output_path.clone())
                            } else {
                                (0, destination.clone())
                            }
                        };

                        // Auto-cleaner: If downloaded file is 0 bytes, discard and fail
                        if final_size == 0 {
                            warn!("Job {} resulted in 0 bytes; cleaning up corrupt file {:?}", id, out_path);
                            let _ = tokio::fs::remove_file(&out_path).await;
                            let err_msg = "Download aborted: server returned 0 bytes of media".to_string();
                            {
                                let mut queue = manager.queue.lock().await;
                                if let Some(j) = queue.get_job_mut(id) {
                                    j.status = DownloadStatus::Failed(err_msg.clone());
                                    j.speed_bytes_sec = 0.0;
                                    j.eta_seconds = None;
                                }
                            }
                            let _ = manager.db.mark_failed(id, &err_msg).await;
                            manager.notify_update().await;
                            break;
                        }

                        info!("Job {} completed successfully with size {} bytes", id, final_size);
                        let job_title = {
                            let mut queue = manager.queue.lock().await;
                            if let Some(j) = queue.get_job_mut(id) {
                                j.status = DownloadStatus::Completed;
                                j.progress_ratio = 1.0;
                                j.speed_bytes_sec = 0.0;
                                j.eta_seconds = Some(0);
                                j.title.clone()
                            } else {
                                "Media file".to_string()
                            }
                        };
                        crate::notifications::send_notification(
                            "Native Video Downloader",
                            "Download Complete",
                            &format!("\"{}\" has finished downloading.", job_title),
                            false,
                        );
                        let _ = manager.db.mark_completed(id, final_size).await;
                        manager.notify_update().await;
                        break;
                    }
                    Err(AppError::Cancelled) => {
                        info!("Job {} was cancelled or paused", id);
                        break;
                    }
                    Err(err) => {
                        warn!("Job {} failed with error: {}", id, err);

                        if manager.retry_policy.should_retry(&err, retry_count) {
                            retry_count += 1;
                            let delay = manager.retry_policy.delay_for_retry(retry_count);
                            warn!("Retrying job {} in {:?} (attempt {}/{})", id, delay, retry_count, manager.retry_policy.max_retries);

                            {
                                let mut queue = manager.queue.lock().await;
                                if let Some(j) = queue.get_job_mut(id) {
                                    j.retry_count = retry_count;
                                }
                            }
                            manager.notify_update().await;

                            tokio::time::sleep(delay).await;
                            continue;
                        } else {
                            error!("Job {} permanently failed: {}", id, err);
                            let err_str = err.to_string();
                            let job_title = {
                                let mut queue = manager.queue.lock().await;
                                if let Some(j) = queue.get_job_mut(id) {
                                    j.status = DownloadStatus::Failed(err_str.clone());
                                    j.speed_bytes_sec = 0.0;
                                    j.eta_seconds = None;
                                    j.title.clone()
                                } else {
                                    "Media download".to_string()
                                }
                            };
                            crate::notifications::send_notification(
                                "Native Video Downloader",
                                "Download Failed",
                                &format!("\"{}\" failed: {}", job_title, err_str),
                                true,
                            );
                            let _ = manager.db.mark_failed(id, &err_str).await;
                            manager.notify_update().await;
                            break;
                        }
                    }
                }
            }
        });
    }
}
