//! `status` — show what is in the data dir, grouped by country:
//! per-dataset source, AIRAC cycle, effective date, age and staleness,
//! plus tile counts and file sizes for the shared basemap archive and the
//! global tile tables.

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result};
use chrono::{DateTime, NaiveDate, Utc};
use indicatif::HumanBytes;
use strata_data::domain::{AiracCycle, Country};
use strata_data::providers::protomaps::Mbtiles;
use strata_data::store::{Dataset, DatasetMeta, Store};
use strata_ingest::IngestConfig;
use strata_ingest::inspect::{elevation_tile_count, terrain_tile_count};

pub fn run(config: &IngestConfig) -> Result<()> {
    let now = Utc::now();
    println!("data dir:  {}", config.data_dir.display());
    let countries = config
        .countries
        .iter()
        .map(|c| format!("{} ({})", c.name(), c.code()))
        .collect::<Vec<_>>()
        .join(", ");
    println!("countries: {countries}");
    println!();

    print_store_status(config, now)?;
    println!();
    print_basemap_status(config);
    Ok(())
}

fn print_store_status(config: &IngestConfig, now: DateTime<Utc>) -> Result<()> {
    let store_path = config.store_path();
    if !store_path.exists() {
        println!(
            "store:     not found at {} — run `strata-ingest aero` first",
            store_path.display()
        );
        return Ok(());
    }

    println!(
        "store:     {} ({})",
        store_path.display(),
        HumanBytes(file_size(&store_path))
    );
    let store = Store::open(&store_path)
        .with_context(|| format!("opening store at {}", store_path.display()))?;

    for &country in &config.countries {
        println!();
        println!("── {} ({}) ──", country.name(), country.code());
        println!("{}", header_row());
        for dataset in Dataset::ALL {
            let line = match store.dataset_meta(dataset, country)? {
                Some(meta) => meta_row(&meta, now),
                None => absent_row(dataset),
            };
            println!("{line}");
        }
    }

    // Stray countries: data ingested for countries not in the current
    // selection still exists (selection scopes ingestion, never deletes).
    let mut stray: Vec<Country> = Vec::new();
    for dataset in Dataset::ALL {
        for meta in store.dataset_metas(dataset)? {
            if !config.countries.contains(&meta.country) && !stray.contains(&meta.country) {
                stray.push(meta.country);
            }
        }
    }
    if !stray.is_empty() {
        let codes = stray.iter().map(|c| c.code()).collect::<Vec<_>>().join(", ");
        println!();
        println!("also in store (not selected): {codes}");
    }

    match terrain_tile_count(&store_path) {
        Some(count) => println!("\nterrain tiles stored: {count}"),
        None => println!("\nterrain tiles stored: 0"),
    }
    println!(
        "elevation tiles stored: {}",
        elevation_tile_count(&store_path).unwrap_or(0)
    );
    Ok(())
}

fn print_basemap_status(config: &IngestConfig) {
    let path = config.basemap_path();
    let path = if path.exists() {
        path
    } else {
        let legacy = config.legacy_basemap_path();
        if legacy.exists() {
            legacy
        } else {
            println!(
                "basemap:   not present at {} — run `strata-ingest basemap`",
                path.display()
            );
            return;
        }
    };
    let detail = match Mbtiles::open(&path) {
        Ok(archive) => {
            let tiles = archive.existing_tiles().map(|t| t.len()).unwrap_or(0);
            let maxzoom = archive
                .metadata("maxzoom")
                .ok()
                .flatten()
                .map(|z| format!(", maxzoom {z}"))
                .unwrap_or_default();
            format!("{tiles} tiles{maxzoom}")
        }
        Err(err) => format!("unreadable ({err})"),
    };
    println!(
        "basemap:   {} ({}) — {detail}",
        path.display(),
        HumanBytes(file_size(&path))
    );
}

fn header_row() -> String {
    format_columns("DATASET", "SOURCE", "AIRAC", "EFFECTIVE", "AGE", "STATUS")
}

fn absent_row(dataset: Dataset) -> String {
    format_columns(dataset.as_str(), "—", "—", "—", "—", "not ingested")
}

/// One status-table line for an ingested dataset.
fn meta_row(meta: &DatasetMeta, now: DateTime<Utc>) -> String {
    let (airac_id, effective, status) = match &meta.airac {
        Some(cycle) => (
            cycle.id().to_string(),
            cycle.effective_date().to_string(),
            airac_status(cycle, now.date_naive()),
        ),
        None => ("—".to_string(), "—".to_string(), "ok".to_string()),
    };
    format_columns(
        meta.dataset.as_str(),
        &meta.source,
        &airac_id,
        &effective,
        &format_age(now - meta.ingested_at),
        &status,
    )
}

fn format_columns(
    dataset: &str,
    source: &str,
    airac: &str,
    effective: &str,
    age: &str,
    status: &str,
) -> String {
    // Single formatting point so header and rows cannot drift apart.
    format!("{dataset:<17} {source:<28} {airac:<6} {effective:<11} {age:>8}  {status}")
}

/// AIRAC staleness for the STATUS column: `current` while the cycle is in
/// effect, `STALE (superseded YYYY-MM-DD)` once a newer cycle applies.
fn airac_status(cycle: &AiracCycle, today: NaiveDate) -> String {
    if cycle.is_stale_at(today) {
        format!("STALE (superseded {})", cycle.supersession_date())
    } else {
        "current".to_string()
    }
}

/// Compact age like `12m`, `3h 20m` or `5d 7h`. Future timestamps clamp
/// to zero.
fn format_age(age: chrono::Duration) -> String {
    let age = age.max(chrono::Duration::zero());
    let days = age.num_days();
    let hours = age.num_hours();
    if days > 0 {
        format!("{days}d {}h", hours - days * 24)
    } else if hours > 0 {
        format!("{hours}h {}m", age.num_minutes() - hours * 60)
    } else {
        format!("{}m", age.num_minutes())
    }
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;
    use strata_data::store::Dataset;

    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn airac_status_current_until_superseded() {
        let cycle = AiracCycle::new("2506", date(2026, 5, 14));
        assert_eq!(airac_status(&cycle, date(2026, 5, 14)), "current");
        assert_eq!(airac_status(&cycle, date(2026, 6, 10)), "current");
        // Supersession day itself is stale (a newer cycle is in effect).
        assert_eq!(
            airac_status(&cycle, date(2026, 6, 11)),
            "STALE (superseded 2026-06-11)"
        );
        assert_eq!(
            airac_status(&cycle, date(2026, 9, 1)),
            "STALE (superseded 2026-06-11)"
        );
    }

    #[test]
    fn format_age_buckets() {
        let minutes = |m: i64| chrono::Duration::minutes(m);
        assert_eq!(format_age(minutes(0)), "0m");
        assert_eq!(format_age(minutes(59)), "59m");
        assert_eq!(format_age(minutes(60)), "1h 0m");
        assert_eq!(format_age(minutes(3 * 60 + 20)), "3h 20m");
        assert_eq!(format_age(minutes(5 * 24 * 60 + 7 * 60)), "5d 7h");
        assert_eq!(format_age(minutes(-10)), "0m"); // clock skew clamps
    }

    #[test]
    fn meta_row_shows_airac_and_staleness() {
        let now = Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap();
        let meta = DatasetMeta {
            dataset: Dataset::Airspaces,
            country: Country::DE,
            source: "openAIP".to_string(),
            airac: Some(AiracCycle::new("2506", date(2026, 5, 14))),
            ingested_at: Utc.with_ymd_and_hms(2026, 6, 28, 12, 0, 0).unwrap(),
        };
        let row = meta_row(&meta, now);
        assert!(row.contains("airspaces"), "got: {row}");
        assert!(row.contains("openAIP"), "got: {row}");
        assert!(row.contains("2506"), "got: {row}");
        assert!(row.contains("2026-05-14"), "got: {row}");
        assert!(row.contains("3d 0h"), "got: {row}");
        assert!(row.contains("STALE (superseded 2026-06-11)"), "got: {row}");
    }

    #[test]
    fn meta_row_without_airac_is_ok() {
        let now = Utc.with_ymd_and_hms(2026, 7, 1, 12, 30, 0).unwrap();
        let meta = DatasetMeta {
            dataset: Dataset::TerrainTiles,
            country: Country::DE,
            source: "Copernicus GLO-30 hillshade".to_string(),
            airac: None,
            ingested_at: Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap(),
        };
        let row = meta_row(&meta, now);
        assert!(row.contains("terrain_tiles"), "got: {row}");
        assert!(row.ends_with("ok"), "got: {row}");
        assert!(!row.contains("STALE"), "got: {row}");
    }
}
