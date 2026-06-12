//! Per-(dataset, country) ingest bookkeeping (`meta` table).

use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params};

use super::{Dataset, DatasetMeta, StoreError};
use crate::domain::{AiracCycle, Country};

const COLUMNS: &str = "dataset, country, source, airac_id, airac_effective, ingested_at";

pub(super) fn read(
    conn: &Connection,
    dataset: Dataset,
    country: Country,
) -> Result<Option<DatasetMeta>, StoreError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM meta WHERE dataset = ?1 AND country = ?2"),
        params![dataset.as_str(), country.code()],
        decode_row,
    )
    .optional()?
    .transpose()
}

/// Every country's row for `dataset`, ordered by country code.
pub(super) fn read_all(
    conn: &Connection,
    dataset: Dataset,
) -> Result<Vec<DatasetMeta>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM meta WHERE dataset = ?1 ORDER BY country"
    ))?;
    let rows = stmt
        .query_map(params![dataset.as_str()], decode_row)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().collect()
}

/// One row decoded; the inner result carries schema-level decode failures.
fn decode_row(row: &Row<'_>) -> rusqlite::Result<Result<DatasetMeta, StoreError>> {
    let dataset_str = row.get::<_, String>(0)?;
    let country_str = row.get::<_, String>(1)?;
    let source = row.get::<_, String>(2)?;
    let airac_id = row.get::<_, Option<String>>(3)?;
    let airac_effective = row.get::<_, Option<String>>(4)?;
    let ingested_at = row.get::<_, String>(5)?;
    Ok(decode_meta(
        dataset_str,
        country_str,
        source,
        airac_id,
        airac_effective,
        ingested_at,
    ))
}

fn decode_meta(
    dataset_str: String,
    country_str: String,
    source: String,
    airac_id: Option<String>,
    airac_effective: Option<String>,
    ingested_at: String,
) -> Result<DatasetMeta, StoreError> {
    let dataset = Dataset::parse(&dataset_str).ok_or_else(|| {
        StoreError::Schema(format!("meta row has unknown dataset {dataset_str:?}"))
    })?;
    let country = Country::from_code(&country_str).ok_or_else(|| {
        StoreError::Schema(format!("meta row has unknown country {country_str:?}"))
    })?;
    let airac = match (airac_id, airac_effective) {
        (Some(id), Some(effective)) => {
            let effective = NaiveDate::parse_from_str(&effective, "%Y-%m-%d").map_err(|e| {
                StoreError::Schema(format!("meta.airac_effective {effective:?} unparseable: {e}"))
            })?;
            Some(AiracCycle::new(id, effective))
        }
        (None, None) => None,
        _ => {
            return Err(StoreError::Schema(format!(
                "meta row for {dataset}/{country_str} has a partial AIRAC cycle"
            )));
        }
    };
    let ingested_at = DateTime::parse_from_rfc3339(&ingested_at)
        .map_err(|e| {
            StoreError::Schema(format!("meta.ingested_at {ingested_at:?} unparseable: {e}"))
        })?
        .with_timezone(&Utc);

    Ok(DatasetMeta {
        dataset,
        country,
        source,
        airac,
        ingested_at,
    })
}

pub(super) fn write(conn: &Connection, meta: &DatasetMeta) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO meta (dataset, country, source, airac_id, airac_effective, ingested_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(dataset, country) DO UPDATE SET
             source = excluded.source,
             airac_id = excluded.airac_id,
             airac_effective = excluded.airac_effective,
             ingested_at = excluded.ingested_at",
        params![
            meta.dataset.as_str(),
            meta.country.code(),
            meta.source,
            meta.airac.as_ref().map(|c| c.id().to_owned()),
            meta.airac
                .as_ref()
                .map(|c| c.effective_date().format("%Y-%m-%d").to_string()),
            meta.ingested_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}
