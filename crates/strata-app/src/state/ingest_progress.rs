//! View-model of the ingest progress panel — pure data + transitions, no
//! IO and no gpui types beyond [`SharedString`].
//!
//! The ingest orchestration populates it exclusively through
//! [`crate::state::AppState::update_ingest_progress`], which emits
//! [`crate::state::AppStateEvent::IngestProgressChanged`]; the panel in
//! `crate::ui::progress_panel` renders the current snapshot and drives its
//! mount/unmount animation from `visible`.

use std::fmt;
use std::rc::Rc;

use gpui::SharedString;

/// Lifecycle of a single ingest job as shown in the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Running,
    Done,
    Failed,
}

/// One ingest job (download, normalize, write, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobVm {
    /// Short name ("openAIP airspaces").
    pub label: SharedString,
    /// Free-form progress detail ("12.3 MB / 48.0 MB", "cancelled").
    pub detail: SharedString,
    /// Completed units (bytes, features, tiles, …).
    pub done: u64,
    /// Total units; `None` while unknown → the job is indeterminate.
    pub total: Option<u64>,
    pub state: JobState,
}

impl JobVm {
    /// Completion in `0.0..=1.0`. Finished jobs (done **or** failed) count
    /// as a full slot so they never stall the overall bar; a running job
    /// without a known total is indeterminate (`None`).
    pub fn fraction(&self) -> Option<f32> {
        match self.state {
            JobState::Done | JobState::Failed => Some(1.0),
            JobState::Running => self.total.map(|total| {
                if total == 0 {
                    0.0
                } else {
                    (self.done as f32 / total as f32).clamp(0.0, 1.0)
                }
            }),
        }
    }

}

/// Callback slot the ingest orchestration fills so the panel's ✕ button
/// can cancel the run (typically by setting a shared cancel flag the
/// orchestration polls).
pub type CancelCallback = Rc<dyn Fn()>;

/// Everything the progress panel renders. Stored on
/// [`crate::state::AppState`].
#[derive(Clone, Default)]
pub struct IngestProgressVm {
    /// Whether the panel should be shown. [`Self::dismiss`] flips this but
    /// keeps `jobs` intact so the panel's exit animation still has content
    /// to render.
    pub visible: bool,
    pub jobs: Vec<JobVm>,
    /// Invoked by the panel's ✕ button; while `None` the button falls back
    /// to plainly dismissing the panel.
    pub on_cancel: Option<CancelCallback>,
}

impl IngestProgressVm {
    /// A new job begins: shows the panel and returns the job's index for
    /// the follow-up calls. Starting fresh after a dismissal drops the
    /// previous run's rows.
    pub fn job_started(
        &mut self,
        label: impl Into<SharedString>,
        detail: impl Into<SharedString>,
        total: Option<u64>,
    ) -> usize {
        if !self.visible {
            self.jobs.clear();
        }
        self.visible = true;
        self.jobs.push(JobVm {
            label: label.into(),
            detail: detail.into(),
            done: 0,
            total,
            state: JobState::Running,
        });
        self.jobs.len() - 1
    }

    /// Progress tick: update completed units and the detail line.
    pub fn job_progress(&mut self, job: usize, done: u64, detail: impl Into<SharedString>) {
        if let Some(job) = self.jobs.get_mut(job) {
            job.done = done;
            job.detail = detail.into();
        }
    }

    /// The job's total became known (or changed, e.g. from Content-Length).
    pub fn job_total(&mut self, job: usize, total: Option<u64>) {
        if let Some(job) = self.jobs.get_mut(job) {
            job.total = total;
        }
    }

    /// The job finished successfully (snaps `done` to the total).
    pub fn job_done(&mut self, job: usize) {
        if let Some(job) = self.jobs.get_mut(job) {
            job.state = JobState::Done;
            if let Some(total) = job.total {
                job.done = total;
            }
        }
    }

    /// The job failed (or was cancelled); `detail` carries the reason.
    pub fn job_failed(&mut self, job: usize, detail: impl Into<SharedString>) {
        if let Some(job) = self.jobs.get_mut(job) {
            job.state = JobState::Failed;
            job.detail = detail.into();
        }
    }

    /// Hide the panel. Jobs are kept so the exit animation renders the
    /// final state; the next [`Self::job_started`] clears them.
    pub fn dismiss(&mut self) {
        self.visible = false;
    }

    /// The job the panel headlines: the first running one, else the last
    /// (so a finished/failed run keeps showing its outcome).
    pub fn active_job(&self) -> Option<&JobVm> {
        self.jobs
            .iter()
            .find(|job| job.state == JobState::Running)
            .or_else(|| self.jobs.last())
    }

    pub fn any_running(&self) -> bool {
        self.jobs.iter().any(|job| job.state == JobState::Running)
    }

    /// Mean of the per-job fractions in `0.0..=1.0`; `None` (indeterminate)
    /// while there are no jobs or any running job lacks a total.
    pub fn overall_fraction(&self) -> Option<f32> {
        if self.jobs.is_empty() {
            return None;
        }
        let mut sum = 0.0f32;
        for job in &self.jobs {
            sum += job.fraction()?;
        }
        Some(sum / self.jobs.len() as f32)
    }
}

impl fmt::Debug for IngestProgressVm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IngestProgressVm")
            .field("visible", &self.visible)
            .field("jobs", &self.jobs)
            .field("on_cancel", &self.on_cancel.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_started_shows_the_panel_and_is_indeterminate_without_total() {
        let mut vm = IngestProgressVm::default();
        assert!(!vm.visible);
        assert_eq!(vm.overall_fraction(), None, "no jobs → indeterminate");

        let job = vm.job_started("openAIP airspaces", "connecting…", None);
        assert!(vm.visible);
        assert_eq!(job, 0);
        assert!(vm.any_running());
        assert_eq!(vm.overall_fraction(), None, "unknown total → indeterminate");
        assert_eq!(vm.active_job().unwrap().label.as_ref(), "openAIP airspaces");
    }

    #[test]
    fn progress_and_total_drive_the_fraction() {
        let mut vm = IngestProgressVm::default();
        let job = vm.job_started("airports", "downloading…", Some(200));
        vm.job_progress(job, 50, "5.0 MB / 20.0 MB");
        assert_eq!(vm.overall_fraction(), Some(0.25));
        assert_eq!(vm.active_job().unwrap().detail.as_ref(), "5.0 MB / 20.0 MB");

        // A total learned later flips indeterminate → determinate.
        let other = vm.job_started("navaids", "…", None);
        assert_eq!(vm.overall_fraction(), None);
        vm.job_total(other, Some(100));
        vm.job_progress(other, 75, "…");
        assert_eq!(vm.overall_fraction(), Some(0.5), "mean of 0.25 and 0.75");
    }

    #[test]
    fn done_snaps_to_full_and_stops_running() {
        let mut vm = IngestProgressVm::default();
        let job = vm.job_started("airspaces", "…", Some(100));
        vm.job_progress(job, 99, "…");
        vm.job_done(job);
        assert!(!vm.any_running());
        assert_eq!(vm.jobs[job].state, JobState::Done);
        assert_eq!(vm.jobs[job].done, 100, "done snaps to the total");
        assert_eq!(vm.overall_fraction(), Some(1.0));
    }

    #[test]
    fn failed_jobs_finish_their_slot_and_keep_the_reason() {
        let mut vm = IngestProgressVm::default();
        let job = vm.job_started("airspaces", "…", None);
        vm.job_failed(job, "cancelled");
        assert!(!vm.any_running());
        assert_eq!(
            vm.overall_fraction(),
            Some(1.0),
            "a failed indeterminate job must not stall the bar"
        );
        let active = vm.active_job().expect("finished job still headlined");
        assert_eq!(active.state, JobState::Failed);
        assert_eq!(active.detail.as_ref(), "cancelled");
    }

    #[test]
    fn active_job_prefers_the_first_running_one() {
        let mut vm = IngestProgressVm::default();
        let first = vm.job_started("airspaces", "…", Some(10));
        let second = vm.job_started("airports", "…", Some(10));
        assert_eq!(vm.active_job().unwrap().label.as_ref(), "airspaces");
        vm.job_done(first);
        assert_eq!(vm.active_job().unwrap().label.as_ref(), "airports");
        vm.job_done(second);
        assert_eq!(
            vm.active_job().unwrap().label.as_ref(),
            "airports",
            "all finished → the last job keeps the headline"
        );
    }

    #[test]
    fn dismiss_keeps_jobs_for_the_exit_animation_and_restart_clears_them() {
        let mut vm = IngestProgressVm::default();
        vm.job_started("airspaces", "…", Some(10));
        vm.dismiss();
        assert!(!vm.visible);
        assert_eq!(vm.jobs.len(), 1, "exit animation still has content");

        vm.job_started("airports", "…", None);
        assert!(vm.visible);
        assert_eq!(vm.jobs.len(), 1, "fresh run drops the previous batch");
        assert_eq!(vm.jobs[0].label.as_ref(), "airports");
    }

    #[test]
    fn out_of_range_job_indices_are_ignored() {
        let mut vm = IngestProgressVm::default();
        vm.job_progress(3, 1, "…");
        vm.job_total(3, Some(1));
        vm.job_done(3);
        vm.job_failed(3, "…");
        assert!(vm.jobs.is_empty());
    }

    #[test]
    fn zero_total_counts_as_unstarted_not_a_division_blowup() {
        let mut vm = IngestProgressVm::default();
        let job = vm.job_started("empty", "…", Some(0));
        assert_eq!(vm.overall_fraction(), Some(0.0));
        vm.job_done(job);
        assert_eq!(vm.overall_fraction(), Some(1.0));
    }
}
