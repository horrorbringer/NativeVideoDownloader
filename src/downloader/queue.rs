use uuid::Uuid;

use crate::downloader::job::DownloadJob;
use crate::models::DownloadStatus;

#[derive(Default)]
#[allow(dead_code)]
pub struct DownloadQueue {
    jobs: Vec<DownloadJob>,
}

#[allow(dead_code)]
impl DownloadQueue {
    pub fn new() -> Self {
        Self { jobs: Vec::new() }
    }

    pub fn add_job(&mut self, job: DownloadJob) -> Uuid {
        let id = job.id;
        self.jobs.push(job);
        id
    }

    pub fn get_job(&self, id: Uuid) -> Option<&DownloadJob> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn get_job_mut(&mut self, id: Uuid) -> Option<&mut DownloadJob> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    pub fn all_jobs(&self) -> &[DownloadJob] {
        &self.jobs
    }

    pub fn all_jobs_mut(&mut self) -> &mut [DownloadJob] {
        &mut self.jobs
    }

    pub fn next_queued_job(&self) -> Option<Uuid> {
        self.jobs
            .iter()
            .find(|j| j.status == DownloadStatus::Queued)
            .map(|j| j.id)
    }

    pub fn next_eligible_job(&self, sched_enabled: bool, in_window: bool) -> Option<Uuid> {
        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.jobs
            .iter()
            .find(|j| {
                if j.status != DownloadStatus::Queued {
                    return false;
                }
                if let Some(target_ts) = j.scheduled_at {
                    if now_ts < target_ts {
                        return false;
                    }
                }
                if sched_enabled && !in_window && !j.bypass_schedule {
                    return false;
                }
                true
            })
            .map(|j| j.id)
    }

    pub fn active_downloads_count(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| j.status == DownloadStatus::Downloading)
            .count()
    }

    pub fn remove_job(&mut self, id: Uuid) -> Option<DownloadJob> {
        if let Some(pos) = self.jobs.iter().position(|j| j.id == id) {
            Some(self.jobs.remove(pos))
        } else {
            None
        }
    }

    pub fn clear_completed(&mut self) {
        self.jobs
            .retain(|j| j.status != DownloadStatus::Completed);
    }

    pub fn clear_failed(&mut self) {
        self.jobs
            .retain(|j| !matches!(j.status, DownloadStatus::Failed(_) | DownloadStatus::Cancelled));
    }

    /// Moves a job up one position in the queue order
    pub fn move_job_up(&mut self, id: Uuid) -> bool {
        if let Some(pos) = self.jobs.iter().position(|j| j.id == id) {
            if pos > 0 {
                self.jobs.swap(pos, pos - 1);
                return true;
            }
        }
        false
    }

    /// Moves a job down one position in the queue order
    pub fn move_job_down(&mut self, id: Uuid) -> bool {
        if let Some(pos) = self.jobs.iter().position(|j| j.id == id) {
            if pos + 1 < self.jobs.len() {
                self.jobs.swap(pos, pos + 1);
                return true;
            }
        }
        false
    }

    /// Moves a queued job to the very top priority among queued jobs
    pub fn prioritize_job(&mut self, id: Uuid) -> bool {
        if let Some(pos) = self.jobs.iter().position(|j| j.id == id) {
            // Find index of first queued job
            let first_queued_idx = self
                .jobs
                .iter()
                .position(|j| j.status == DownloadStatus::Queued)
                .unwrap_or(0);
            if pos > first_queued_idx {
                let job = self.jobs.remove(pos);
                self.jobs.insert(first_queued_idx, job);
                return true;
            }
        }
        false
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_clear_completed_and_failed() {
        let mut queue = DownloadQueue::new();
        let make_job = |url: &str, title: &str| {
            DownloadJob::new(
                url.to_string(),
                title.to_string(),
                PathBuf::from(format!("{}.mp4", title)),
                None,
                true,
                false,
                None,
                false,
                None,
                false,
                None,
                None,
                None,
                false,
            )
        };

        let mut j1 = make_job("https://example.com/1", "Vid 1");
        j1.status = DownloadStatus::Completed;
        let id1 = queue.add_job(j1);

        let mut j2 = make_job("https://example.com/2", "Vid 2");
        j2.status = DownloadStatus::Failed("Network error".to_string());
        let id2 = queue.add_job(j2);

        let mut j3 = make_job("https://example.com/3", "Vid 3");
        j3.status = DownloadStatus::Downloading;
        let id3 = queue.add_job(j3);

        assert_eq!(queue.all_jobs().len(), 3);

        queue.clear_failed();
        assert_eq!(queue.all_jobs().len(), 2);
        assert!(queue.get_job(id2).is_none());
        assert!(queue.get_job(id1).is_some());
        assert!(queue.get_job(id3).is_some());

        queue.clear_completed();
        assert_eq!(queue.all_jobs().len(), 1);
        assert!(queue.get_job(id1).is_none());
        assert!(queue.get_job(id3).is_some());
    }

    #[test]
    fn test_reorder_and_prioritize() {
        let mut queue = DownloadQueue::new();
        let make_job = |url: &str, title: &str| {
            DownloadJob::new(
                url.to_string(),
                title.to_string(),
                PathBuf::from(format!("{}.mp4", title)),
                None,
                true,
                false,
                None,
                false,
                None,
                false,
                None,
                None,
                None,
                false,
            )
        };

        let id1 = queue.add_job(make_job("https://example.com/1", "Job 1"));
        let id2 = queue.add_job(make_job("https://example.com/2", "Job 2"));
        let id3 = queue.add_job(make_job("https://example.com/3", "Job 3"));

        // Initial order: [id1, id2, id3]
        assert_eq!(queue.all_jobs()[0].id, id1);
        assert_eq!(queue.all_jobs()[1].id, id2);
        assert_eq!(queue.all_jobs()[2].id, id3);

        // Move id3 up -> [id1, id3, id2]
        assert!(queue.move_job_up(id3));
        assert_eq!(queue.all_jobs()[1].id, id3);

        // Move id1 down -> [id3, id1, id2]
        assert!(queue.move_job_down(id1));
        assert_eq!(queue.all_jobs()[0].id, id3);
        assert_eq!(queue.all_jobs()[1].id, id1);

        // Prioritize id2 -> [id2, id3, id1]
        assert!(queue.prioritize_job(id2));
        assert_eq!(queue.all_jobs()[0].id, id2);
    }

    #[test]
    fn test_next_eligible_job_scheduling() {
        let mut queue = DownloadQueue::new();
        let make_job = |url: &str, title: &str| {
            DownloadJob::new(
                url.to_string(),
                title.to_string(),
                PathBuf::from(format!("{}.mp4", title)),
                None,
                true,
                false,
                None,
                false,
                None,
                false,
                None,
                None,
                None,
                false,
            )
        };

        let mut j1 = make_job("https://example.com/1", "Job 1");
        j1.status = DownloadStatus::Queued;
        let id1 = queue.add_job(j1);

        let mut j2 = make_job("https://example.com/2", "Job 2");
        j2.status = DownloadStatus::Queued;
        j2.bypass_schedule = true;
        let id2 = queue.add_job(j2);

        // When scheduler is enabled and outside window:
        // Job 1 cannot start without bypass_schedule, but Job 2 can start because bypass_schedule is true
        assert_eq!(queue.next_eligible_job(true, false), Some(id2));

        // When inside schedule window:
        // Job 1 is first in queue and can start
        assert_eq!(queue.next_eligible_job(true, true), Some(id1));

        // When scheduler is disabled:
        // Job 1 is first in queue and can start
        assert_eq!(queue.next_eligible_job(false, false), Some(id1));
    }
}

