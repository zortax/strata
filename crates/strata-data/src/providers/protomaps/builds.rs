//! The Protomaps daily-build index (`builds.json`): a JSON array of build
//! entries, one per uploaded planet archive.

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// One entry of the builds index. Unknown fields (`size`, checksums, …)
/// are ignored.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct BuildEntry {
    /// File name, e.g. `20260609.pmtiles`.
    pub key: String,
    #[serde(default)]
    pub uploaded: Option<DateTime<Utc>>,
}

/// The newest `.pmtiles` build: latest upload timestamp, key (date-named,
/// so lexicographically chronological) as tie-breaker/fallback.
pub(super) fn latest(entries: &[BuildEntry]) -> Option<&BuildEntry> {
    entries
        .iter()
        .filter(|e| e.key.ends_with(".pmtiles"))
        .max_by(|a, b| (a.uploaded, &a.key).cmp(&(b.uploaded, &b.key)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shape verified against https://build-metadata.protomaps.dev/builds.json
    // on 2026-06-10.
    const FIXTURE: &str = r#"[
        {"key":"20230918.pmtiles","size":114683195425,"uploaded":"2023-09-18T11:02:48.409Z","version":"0.0.0"},
        {"key":"20260609.pmtiles","size":136193741494,"md5sum":"V1luNmv2bXTm2MEOsVRlkg==","b3sum":"edbf1ab711f642a90d33f09b9694cb502e7a2069026681e476b83eca7a0dc6c1","uploaded":"2026-06-09T21:30:33.628Z","version":"4.14.9"},
        {"key":"20260607.pmtiles","size":136161083036,"uploaded":"2026-06-07T08:51:10.446Z","version":"4.14.9"},
        {"key":"index.html","uploaded":"2026-06-10T00:00:00.000Z"}
    ]"#;

    #[test]
    fn picks_newest_pmtiles_build() {
        let entries: Vec<BuildEntry> = serde_json::from_str(FIXTURE).expect("parse fixture");
        assert_eq!(entries.len(), 4);
        let latest = latest(&entries).expect("non-empty");
        // index.html is newer but not a build; 20260609 wins by upload time.
        assert_eq!(latest.key, "20260609.pmtiles");
    }

    #[test]
    fn falls_back_to_key_order_without_timestamps() {
        let entries: Vec<BuildEntry> =
            serde_json::from_str(r#"[{"key":"20240101.pmtiles"},{"key":"20240201.pmtiles"}]"#)
                .expect("parse");
        assert_eq!(latest(&entries).expect("some").key, "20240201.pmtiles");
    }

    #[test]
    fn empty_or_buildless_index_yields_none() {
        assert!(latest(&[]).is_none());
        let entries: Vec<BuildEntry> =
            serde_json::from_str(r#"[{"key":"readme.txt"}]"#).expect("parse");
        assert!(latest(&entries).is_none());
    }
}
