//! The progress contract between ingestion jobs and their consumers (GUI or
//! CLI): [`IngestEvent`]s sent over a tokio mpsc channel.
//!
//! Events are derived from the same underlying provider callbacks that used
//! to feed indicatif directly; the CLI now renders them through an adapter.

use tokio::sync::mpsc;

/// One progress-reporting unit of work — one progress bar in the CLI, one
/// progress row in the GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IngestJob {
    AeroAirspaces,
    AeroAirports,
    AeroNavaids,
    AeroReportingPoints,
    AeroObstacles,
    Basemap,
    Terrain,
    /// Max-pooling the DEM into the store's elevation grid (part of the
    /// terrain stage, also runnable standalone).
    Elevation,
}

impl IngestJob {
    /// Human label (also the CLI progress-bar prefix).
    pub fn label(self) -> &'static str {
        match self {
            Self::AeroAirspaces => "airspaces",
            Self::AeroAirports => "airports",
            Self::AeroNavaids => "navaids",
            Self::AeroReportingPoints => "reporting points",
            Self::AeroObstacles => "obstacles",
            Self::Basemap => "basemap",
            Self::Terrain => "terrain",
            Self::Elevation => "elevation",
        }
    }

    /// Whether the job reports a tile total (progress bar) or is an
    /// indeterminate fetch (spinner).
    pub fn has_total(self) -> bool {
        matches!(self, Self::Basemap | Self::Terrain | Self::Elevation)
    }
}

/// Progress stream of an [`Ingestion`](crate::Ingestion) run.
///
/// Every started job is guaranteed a terminal event ([`JobFinished`],
/// [`JobFailed`] — aborted jobs report `JobFailed` too), and every runner
/// entry point ends its stream with [`RunFinished`], success or not.
///
/// [`JobFinished`]: IngestEvent::JobFinished
/// [`JobFailed`]: IngestEvent::JobFailed
/// [`RunFinished`]: IngestEvent::RunFinished
#[derive(Debug, Clone, PartialEq)]
pub enum IngestEvent {
    JobStarted {
        job: IngestJob,
        label: String,
    },
    Progress {
        job: IngestJob,
        done: u64,
        /// `None` until the job has established its total (spinner phase).
        total: Option<u64>,
        /// Short human-readable status, e.g. "fetching…" or "1.43 MiB
        /// written".
        detail: String,
    },
    JobFinished {
        job: IngestJob,
        /// Completion message, e.g. "1234 fetched, 1230 normalized, 4
        /// skipped".
        summary: String,
    },
    JobFailed {
        job: IngestJob,
        /// Flattened error chain (see [`crate::error_chain`]), or "aborted"
        /// when the job was cut short by cancellation or a sibling failure.
        error: String,
    },
    /// Terminal event of every runner entry point — emitted after success,
    /// failure and cancellation alike.
    RunFinished,
}

pub type IngestEventReceiver = mpsc::UnboundedReceiver<IngestEvent>;

/// Sending half of the event stream, held by the
/// [`Ingestion`](crate::Ingestion) runner. Cloneable; sending never blocks
/// and silently drops events once the receiver is gone.
#[derive(Debug, Clone)]
pub struct EventSink {
    tx: mpsc::UnboundedSender<IngestEvent>,
}

impl EventSink {
    /// A fresh event channel: hand the sink to the runner, consume the
    /// receiver from the UI.
    pub fn channel() -> (Self, IngestEventReceiver) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    /// Wraps an existing sender (e.g. one feeding a fan-in channel).
    pub fn from_sender(tx: mpsc::UnboundedSender<IngestEvent>) -> Self {
        Self { tx }
    }

    /// A sink whose events go nowhere (tests, fire-and-forget runs).
    pub fn discard() -> Self {
        Self::channel().0
    }

    pub fn emit(&self, event: IngestEvent) {
        // A closed receiver means nobody is watching — that is fine.
        let _ = self.tx.send(event);
    }
}

/// RAII reporter for one job: emits `JobStarted` on creation, `Progress` on
/// demand and exactly one terminal event — `JobFinished`/`JobFailed` when
/// consumed, or `JobFailed("aborted")` if dropped mid-flight (cancellation,
/// sibling failure).
pub(crate) struct JobHandle<'a> {
    sink: &'a EventSink,
    job: IngestJob,
    terminal_sent: bool,
}

impl<'a> JobHandle<'a> {
    /// Starts the job. The label is what consumers display (CLI bar
    /// prefix); multi-country runs append the country, e.g.
    /// "airspaces DE", bbox-override smoke passes use the bare
    /// [`IngestJob::label`].
    pub(crate) fn start_with_label(
        sink: &'a EventSink,
        job: IngestJob,
        label: impl Into<String>,
    ) -> Self {
        sink.emit(IngestEvent::JobStarted {
            job,
            label: label.into(),
        });
        Self {
            sink,
            job,
            terminal_sent: false,
        }
    }

    pub(crate) fn progress(&self, done: u64, total: Option<u64>, detail: impl Into<String>) {
        self.sink.emit(IngestEvent::Progress {
            job: self.job,
            done,
            total,
            detail: detail.into(),
        });
    }

    pub(crate) fn finish(mut self, summary: impl Into<String>) {
        self.terminal_sent = true;
        self.sink.emit(IngestEvent::JobFinished {
            job: self.job,
            summary: summary.into(),
        });
    }

    pub(crate) fn fail(mut self, error: impl Into<String>) {
        self.terminal_sent = true;
        self.sink.emit(IngestEvent::JobFailed {
            job: self.job,
            error: error.into(),
        });
    }
}

impl Drop for JobHandle<'_> {
    fn drop(&mut self) {
        if !self.terminal_sent {
            self.sink.emit(IngestEvent::JobFailed {
                job: self.job,
                error: "aborted".to_string(),
            });
        }
    }
}

/// indicatif-compatible binary byte formatting ("15 B", "1.46 KiB") so event
/// detail strings match what the CLI used to render with `HumanBytes`.
pub(crate) fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(rx: &mut IngestEventReceiver) -> Vec<IngestEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    #[test]
    fn mocked_job_emits_lifecycle_events_in_order() {
        let (sink, mut rx) = EventSink::channel();

        let handle = JobHandle::start_with_label(&sink, IngestJob::Terrain, "terrain");
        handle.progress(3, Some(10), "z5");
        handle.finish("done");

        assert_eq!(
            drain(&mut rx),
            vec![
                IngestEvent::JobStarted {
                    job: IngestJob::Terrain,
                    label: "terrain".to_string(),
                },
                IngestEvent::Progress {
                    job: IngestJob::Terrain,
                    done: 3,
                    total: Some(10),
                    detail: "z5".to_string(),
                },
                IngestEvent::JobFinished {
                    job: IngestJob::Terrain,
                    summary: "done".to_string(),
                },
            ]
        );
    }

    #[test]
    fn failing_job_emits_job_failed() {
        let (sink, mut rx) = EventSink::channel();

        let handle = JobHandle::start_with_label(&sink, IngestJob::AeroAirports, "airports");
        handle.fail("boom");

        let events = drain(&mut rx);
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[1],
            IngestEvent::JobFailed {
                job: IngestJob::AeroAirports,
                error: "boom".to_string(),
            }
        );
    }

    #[test]
    fn dropped_job_reports_aborted() {
        let (sink, mut rx) = EventSink::channel();

        let handle = JobHandle::start_with_label(&sink, IngestJob::Basemap, "basemap");
        handle.progress(1, None, "");
        drop(handle);

        let events = drain(&mut rx);
        assert_eq!(
            events.last(),
            Some(&IngestEvent::JobFailed {
                job: IngestJob::Basemap,
                error: "aborted".to_string(),
            })
        );
    }

    #[test]
    fn finished_job_does_not_double_report_on_drop() {
        let (sink, mut rx) = EventSink::channel();

        JobHandle::start_with_label(&sink, IngestJob::Terrain, "terrain").finish("done");

        let terminals = drain(&mut rx)
            .into_iter()
            .filter(|e| !matches!(e, IngestEvent::JobStarted { .. }))
            .count();
        assert_eq!(terminals, 1);
    }

    #[test]
    fn emitting_without_a_receiver_is_silent() {
        let sink = EventSink::discard();
        sink.emit(IngestEvent::RunFinished); // must not panic
        JobHandle::start_with_label(&sink, IngestJob::Terrain, "terrain").finish("done");
    }

    #[test]
    fn job_labels_and_kinds() {
        assert_eq!(IngestJob::AeroReportingPoints.label(), "reporting points");
        assert_eq!(IngestJob::Elevation.label(), "elevation");
        assert!(IngestJob::Terrain.has_total());
        assert!(IngestJob::Basemap.has_total());
        assert!(IngestJob::Elevation.has_total());
        assert!(!IngestJob::AeroAirspaces.has_total());
    }

    #[test]
    fn human_bytes_matches_indicatif() {
        // Reference values from indicatif's `HumanBytes` docs.
        assert_eq!(human_bytes(15), "15 B");
        assert_eq!(human_bytes(1_500), "1.46 KiB");
        assert_eq!(human_bytes(1_500_000), "1.43 MiB");
        assert_eq!(human_bytes(1_500_000_000), "1.40 GiB");
        assert_eq!(human_bytes(1_500_000_000_000), "1.36 TiB");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.00 KiB");
    }
}
