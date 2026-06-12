//! Name/ident search across all feature tables.
//!
//! Ranking: exact ident match (0) < ident prefix match (1) < name substring
//! match (2); ties break on the display label. The denormalized `ident` and
//! `name` columns are stored uppercased (Rust Unicode uppercasing, so
//! umlauts fold correctly), making plain SQL comparisons case-insensitive.

use rusqlite::{Connection, params};

use super::records::FeatureRecord;
use super::{SearchHit, StoreError};
use crate::domain::{Airport, Airspace, Navaid, Obstacle, ReportingPoint};

pub(super) fn search(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, StoreError> {
    let needle = query.trim().to_uppercase();
    if needle.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let mut ranked: Vec<(u8, String, SearchHit)> = Vec::new();
    collect::<Airport>(conn, &needle, limit, &mut ranked)?;
    collect::<Navaid>(conn, &needle, limit, &mut ranked)?;
    collect::<Airspace>(conn, &needle, limit, &mut ranked)?;
    collect::<ReportingPoint>(conn, &needle, limit, &mut ranked)?;
    collect::<Obstacle>(conn, &needle, limit, &mut ranked)?;

    ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    Ok(ranked
        .into_iter()
        .take(limit)
        .map(|(_, _, hit)| hit)
        .collect())
}

/// Takes the best `limit` matches from one table (safe to cap per table:
/// the global cut keeps at most `limit` overall, ordered the same way).
fn collect<T: FeatureRecord>(
    conn: &Connection,
    needle: &str,
    limit: usize,
    out: &mut Vec<(u8, String, SearchHit)>,
) -> Result<(), StoreError> {
    let sql = format!(
        "SELECT data,
                CASE
                    WHEN ident = ?1 THEN 0
                    WHEN substr(ident, 1, length(?1)) = ?1 THEN 1
                    ELSE 2
                END AS rank
         FROM {table}
         WHERE ident = ?1
            OR substr(ident, 1, length(?1)) = ?1
            OR instr(name, ?1) > 0
         ORDER BY rank, name
         LIMIT ?2",
        table = T::TABLE
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(
            params![needle, i64::try_from(limit).unwrap_or(i64::MAX)],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, u8>(1)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    for (blob, rank) in rows {
        let item: T = postcard::from_bytes(&blob)?;
        let label = item.label();
        let hit = SearchHit {
            label: label.clone(),
            position: item.label_position(),
            feature: item.into_feature(),
        };
        out.push((rank, label, hit));
    }
    Ok(())
}
