//! Shared atomic-file-write primitive (temp file + fsync + rename) with
//! process-wide write ordering — the one implementation behind flight,
//! aircraft and config persistence (it used to exist twice and drift).
//!
//! Two concurrency hazards this module exists to kill:
//!
//! - **Temp-file collisions.** A pid-keyed temp name is constant for a
//!   given target within one process, so two concurrent writers to the
//!   same file (the aircraft editor commits on every parseable keystroke,
//!   each write detached onto the multithreaded background executor)
//!   could truncate each other's in-progress temp or rename a mixed file
//!   into place. Every write gets a process-unique temp name instead, and
//!   the failure-path cleanup only ever removes the writer's own temp.
//! - **Stale-snapshot-wins.** Detached background writes have no ordering
//!   guarantee: an older snapshot's rename can land last. Async callers
//!   capture a [`WriteTicket`] *together with the snapshot* (on the UI
//!   thread, where mutation order is defined) and commit through
//!   [`write_atomic_ordered`], which skips the rename when a newer ticket
//!   has already committed for the same path.

use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, PoisonError};

use anyhow::Context as _;

/// Monotonic process-wide ordering token for one snapshot's write.
/// Capture it with the snapshot — before spawning the background write —
/// so ticket order equals snapshot order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WriteTicket(u64);

impl WriteTicket {
    /// The next ticket (strictly increasing across the process).
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// What an ordered write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    /// The snapshot was renamed into place.
    Committed,
    /// A newer snapshot had already committed for this path; the write was
    /// dropped (its temp file removed) instead of clobbering newer content.
    SupersededByNewer,
}

/// Latest committed ticket per target path. Grows only with distinct
/// written paths — a handful per session.
static COMMITTED: LazyLock<Mutex<HashMap<PathBuf, u64>>> = LazyLock::new(Mutex::default);

fn committed_lock() -> std::sync::MutexGuard<'static, HashMap<PathBuf, u64>> {
    // A poisoned lock only means another writer panicked mid-commit; the
    // map itself is always in a valid state.
    COMMITTED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Writes `text` to `path` atomically: process-unique temp file in the
/// same directory (same filesystem, so the rename is atomic), fsync,
/// rename. Parent directories are created as needed; the temp file is
/// removed on failure. The ordering ticket is captured at call time —
/// right for synchronous/sequential callers; async snapshot writers use
/// [`write_atomic_ordered`] with a ticket captured at snapshot time.
pub fn write_atomic(path: &Path, text: &str) -> anyhow::Result<()> {
    write_atomic_ordered(path, text, WriteTicket::next()).map(|_| ())
}

/// [`write_atomic`] with a caller-captured [`WriteTicket`]: the rename is
/// skipped (and the temp file removed) when a newer ticket has already
/// committed for `path`, so an old snapshot can never overwrite a newer
/// one regardless of which background write finishes last.
pub fn write_atomic_ordered(
    path: &Path,
    text: &str,
    ticket: WriteTicket,
) -> anyhow::Result<WriteOutcome> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("create directory {}", parent.display()))?;
    }
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document");
    // pid + process-wide sequence: unique even when two writes target the
    // same path concurrently (the pid alone is not).
    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));

    let result = (|| -> anyhow::Result<WriteOutcome> {
        let mut file = fs::File::create(&tmp)
            .with_context(|| format!("create temp file {}", tmp.display()))?;
        file.write_all(text.as_bytes()).context("write document")?;
        file.sync_all().context("sync document to disk")?;
        drop(file);
        // Check-and-rename is one atomic step under the lock — otherwise
        // an older write could still rename after a newer one checked.
        let mut committed = committed_lock();
        if committed
            .get(path)
            .is_some_and(|&latest| latest > ticket.0)
        {
            drop(committed);
            let _ = fs::remove_file(&tmp);
            return Ok(WriteOutcome::SupersededByNewer);
        }
        fs::rename(&tmp, path)
            .with_context(|| format!("rename temp file into {}", path.display()))?;
        committed.insert(path.to_path_buf(), ticket.0);
        Ok(WriteOutcome::Committed)
    })();

    if result.is_err() {
        // Best effort; the rename consumed the temp file on success, and
        // the name is unique to this writer — no cross-writer hazard.
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Records `ticket` as committed for `path` **without writing** — for
/// diff-aware savers that found the on-disk content already equal to
/// their snapshot: an older in-flight snapshot must not later rename
/// stale content over it.
pub fn mark_committed(path: &Path, ticket: WriteTicket) {
    let mut committed = committed_lock();
    let latest = committed.entry(path.to_path_buf()).or_insert(0);
    *latest = (*latest).max(ticket.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_files_in(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn write_atomic_writes_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("doc.json");
        write_atomic(&path, "{}").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}");
        write_atomic(&path, "{\"a\":1}").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"a\":1}");

        let entries: Vec<String> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["doc.json".to_owned()]);
    }

    #[test]
    fn failed_write_cleans_up_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        // A directory squatting on the target path makes the rename fail.
        let path = dir.path().join("doc.json");
        fs::create_dir(&path).unwrap();
        assert!(write_atomic(&path, "{}").is_err());
        let entries: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["doc.json".to_owned()], "no temp turds");
    }

    #[test]
    fn stale_snapshot_never_overwrites_a_newer_commit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.json");
        // Tickets captured in snapshot order, writes landing in reverse.
        let older = WriteTicket::next();
        let newer = WriteTicket::next();
        assert_eq!(
            write_atomic_ordered(&path, "new", newer).unwrap(),
            WriteOutcome::Committed
        );
        assert_eq!(
            write_atomic_ordered(&path, "old", older).unwrap(),
            WriteOutcome::SupersededByNewer
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert!(temp_files_in(dir.path()).is_empty(), "superseded temp removed");
    }

    #[test]
    fn mark_committed_claims_the_ticket_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write_atomic(&path, "x = 1").unwrap();

        // A diff-aware saver found its (newer) snapshot already on disk
        // and only claimed the ticket; the older in-flight write must
        // still lose.
        let older = WriteTicket::next();
        let newer = WriteTicket::next();
        mark_committed(&path, newer);
        assert_eq!(
            write_atomic_ordered(&path, "x = 0", older).unwrap(),
            WriteOutcome::SupersededByNewer
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "x = 1");
    }

    /// The keystroke-burst scenario: many concurrent writers against one
    /// target. Every observed read must be a complete snapshot (rename is
    /// atomic; unique temp names keep writers from truncating each other),
    /// and no temp files may remain.
    #[test]
    fn concurrent_writers_to_one_target_never_corrupt_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.json");
        write_atomic(&path, "{\"thread\":0,\"write\":0}").unwrap();

        let writers: Vec<_> = (1..=4u32)
            .map(|thread| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for write in 0..25u32 {
                        let text = format!("{{\"thread\":{thread},\"write\":{write}}}");
                        write_atomic(&path, &text).unwrap();
                    }
                })
            })
            .collect();
        let reader = {
            let path = path.clone();
            std::thread::spawn(move || {
                for _ in 0..200 {
                    let text = fs::read_to_string(&path).unwrap();
                    let value: serde_json::Value =
                        serde_json::from_str(&text).expect("every read parses");
                    assert!(value.get("thread").is_some());
                }
            })
        };
        for writer in writers {
            writer.join().unwrap();
        }
        reader.join().unwrap();
        assert!(temp_files_in(dir.path()).is_empty(), "no temp turds");
        // The final content is one writer's last snapshot, intact.
        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(value.get("write").is_some());
    }
}
