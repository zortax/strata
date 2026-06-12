//! stdout summaries printed after each successful stage — output formats
//! are unchanged from the pre-library CLI (docs and automation rely on
//! them).

use std::fs;

use strata_ingest::{AeroSummary, BasemapSummary, ElevationSummary, TerrainSummary};
use indicatif::HumanBytes;

pub fn aero(summary: &AeroSummary) {
    println!();
    println!(
        "openAIP aero ingest — AIRAC {} (effective {}) → {}",
        summary.airac.id(),
        summary.airac.effective_date(),
        summary.store_path.display()
    );
    println!(
        "{:<8} {:<17} {:>8} {:>9} {:>8}",
        "country", "dataset", "fetched", "ingested", "skipped"
    );
    for o in &summary.datasets {
        println!(
            "{:<8} {:<17} {:>8} {:>9} {:>8}",
            o.country.code(),
            o.dataset.as_str(),
            o.fetched,
            o.ingested,
            o.skipped.len()
        );
    }

    let skipped_total: usize = summary.datasets.iter().map(|o| o.skipped.len()).sum();
    if skipped_total > 0 {
        // Partial failure is not fatal: the datasets were written, minus the
        // items that failed normalization (each already warned individually
        // by the provider).
        tracing::warn!(
            skipped = skipped_total,
            "some openAIP items failed normalization and were skipped; ingest succeeded without them"
        );
        for o in summary.datasets.iter().filter(|o| !o.skipped.is_empty()) {
            tracing::warn!(
                dataset = o.dataset.as_str(),
                country = o.country.code(),
                skipped = o.skipped.len(),
                first_reason = %o.skipped[0].1,
                "skipped items in dataset"
            );
        }
    }
}

pub fn terrain(summary: &TerrainSummary) {
    println!(
        "terrain: {} hillshade tiles (z{}–z{}) → {}",
        summary.rendered,
        summary.minzoom,
        summary.maxzoom,
        summary.store_path.display()
    );
    elevation(&summary.elevation);
}

pub fn elevation(summary: &ElevationSummary) {
    println!(
        "elevation: {} max-pooled tiles (6 arc-sec grid, {} DEM squares) → {}",
        summary.tiles_written,
        summary.dem_tiles,
        summary.store_path.display()
    );
}

pub fn basemap(summary: &BasemapSummary) {
    let file_size = fs::metadata(&summary.dest).map(|m| m.len()).unwrap_or(0);
    println!(
        "basemap: {} tiles up to z{} ({} written this run) → {} ({})",
        summary.tiles_done,
        summary.maxzoom,
        HumanBytes(summary.bytes_written),
        summary.dest.display(),
        HumanBytes(file_size)
    );
}
