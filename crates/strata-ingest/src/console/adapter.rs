//! indicatif renderer for the library's [`IngestEvent`] stream — recreates
//! exactly the bars the subcommands used to drive directly.

use std::collections::HashMap;
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use strata_ingest::{IngestEvent, IngestEventReceiver, IngestJob};

const TICK: Duration = Duration::from_millis(100);

/// Renders events until the stream closes (the runner — the only sender —
/// was dropped).
pub async fn render(mut events: IngestEventReceiver) {
    let multi = MultiProgress::new();
    let mut bars: HashMap<IngestJob, ProgressBar> = HashMap::new();
    while let Some(event) = events.recv().await {
        match event {
            IngestEvent::JobStarted { job, label } => {
                bars.insert(job, multi.add(bar_for(job, label)));
            }
            IngestEvent::Progress {
                job,
                done,
                total,
                detail,
            } => {
                if let Some(pb) = bars.get(&job) {
                    if let Some(total) = total
                        && pb.length() != Some(total)
                    {
                        pb.set_length(total);
                    }
                    pb.set_position(done);
                    pb.set_message(detail);
                }
            }
            IngestEvent::JobFinished { job, summary } => {
                if let Some(pb) = bars.remove(&job) {
                    pb.finish_with_message(summary);
                }
            }
            IngestEvent::JobFailed { job, .. } => {
                if let Some(pb) = bars.remove(&job) {
                    pb.abandon_with_message("failed");
                }
            }
            IngestEvent::RunFinished => {}
        }
    }
}

fn bar_for(job: IngestJob, label: String) -> ProgressBar {
    if job.has_total() {
        tile_bar(label)
    } else {
        spinner(label)
    }
}

/// Label + spinner + message, used for fetches without a known total.
fn spinner(label: String) -> ProgressBar {
    let pb = ProgressBar::new_spinner()
        .with_style(
            // Static template: cannot fail to parse.
            ProgressStyle::with_template("{spinner:.cyan} {prefix:<17} {msg}")
                .expect("static progress template"),
        )
        .with_prefix(label);
    pb.enable_steady_tick(TICK);
    pb
}

/// Tile-count bar; starts without a length (set once the total is known).
fn tile_bar(label: String) -> ProgressBar {
    let pb = ProgressBar::no_length()
        .with_style(
            // Static template: cannot fail to parse.
            ProgressStyle::with_template(
                "{prefix:<8} {wide_bar:.cyan/blue} {pos}/{len} {msg} [{elapsed_precise} ETA {eta}]",
            )
            .expect("static progress template"),
        )
        .with_prefix(label);
    pb.enable_steady_tick(TICK);
    pb
}
