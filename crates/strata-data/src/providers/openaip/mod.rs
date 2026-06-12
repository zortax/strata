//! openAIP REST provider (`https://api.core.openaip.net/api/`).
//!
//! Paginated fetches with `country=<alpha-2>` (one fetch per enabled
//! [`Country`](crate::domain::Country) — the parameter takes ISO 3166-1
//! alpha-2 codes, verified live for the curated set), header
//! `x-openaip-api-key`, strict
//! vertical-limit normalization (the classic-bug area — see
//! [`common::vertical_limit`]). Items that fail normalization are skipped
//! and counted per [`NormalizationReport`], never fatal. Data is CC BY-NC —
//! keep it behind the provider traits.

mod airports;
mod airspaces;
mod client;
mod common;
mod navaids;
mod obstacles;
mod reporting_points;

pub use client::OpenAipClient;

/// Outcome of normalizing one raw openAIP item list. Skipped items carry
/// their openAIP `_id` and a human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NormalizationReport {
    /// Raw items received from the API.
    pub total: usize,
    /// `(_id, reason)` per item that failed normalization.
    pub skipped: Vec<(String, String)>,
}

impl NormalizationReport {
    fn new(total: usize) -> Self {
        Self {
            total,
            skipped: Vec::new(),
        }
    }

    /// Records (and logs) one skipped item.
    fn skip(&mut self, id: &str, reason: String, what: &'static str) {
        tracing::warn!(id, %reason, "skipping {what}");
        self.skipped.push((id.to_owned(), reason));
    }

    /// Items that normalized successfully.
    pub fn normalized(&self) -> usize {
        self.total - self.skipped.len()
    }

    fn log_summary(&self, what: &'static str) {
        tracing::info!(
            total = self.total,
            normalized = self.normalized(),
            skipped = self.skipped.len(),
            "normalized {what}"
        );
    }
}

/// Parses a fixture file's `items` array for the per-type module tests.
#[cfg(test)]
fn fixture_items(fixture_json: &str) -> Vec<serde_json::Value> {
    let envelope: serde_json::Value =
        serde_json::from_str(fixture_json).expect("fixture parses as JSON");
    envelope
        .get("items")
        .and_then(serde_json::Value::as_array)
        .expect("fixture has an items array")
        .clone()
}
