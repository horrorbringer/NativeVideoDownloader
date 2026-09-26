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

    pub fn next_queued_job(&self) -> Option<Uuid> {
        self.jobs
            .iter()
            .find(|j| j.status == DownloadStatus::Queued)
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
}
