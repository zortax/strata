//! Generic plumbing shared by the five feature tables: one row per feature
//! holding the postcard-encoded domain struct plus denormalized search
//! columns and the ingesting country (replace-all scope), mirrored into an
//! R*Tree by rowid for bbox queries. Reads stay country-agnostic — the map
//! renders whatever the store holds.

use rusqlite::{Connection, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{Feature, StoreError};
use crate::domain::{
    Airport, Airspace, BoundingBox, Country, IcaoCode, LatLon, Navaid, Obstacle, ReportingPoint,
};

/// A domain type stored in its own feature table.
pub(super) trait FeatureRecord: Serialize + DeserializeOwned {
    /// Base table name; the paired R*Tree is `{TABLE}_rtree`.
    const TABLE: &'static str;

    fn bbox(&self) -> BoundingBox;

    /// Searchable identifier (ICAO/navaid ident), if any.
    fn ident(&self) -> Option<&str>;

    /// Searchable display name, if any.
    fn name(&self) -> Option<&str>;

    /// Anchor position for distance ordering; `None` for area features.
    fn position(&self) -> Option<LatLon>;

    fn into_feature(self) -> Feature;

    /// Fly-to / label point: own position, or the bbox center for areas.
    fn label_position(&self) -> LatLon {
        self.position().unwrap_or_else(|| self.bbox().center())
    }

    /// Search-result label, e.g. `"EDDF — Frankfurt/Main"`.
    fn label(&self) -> String {
        let name = self.name().unwrap_or("(unnamed)");
        match self.ident() {
            Some(ident) => format!("{ident} — {name}"),
            None => name.to_owned(),
        }
    }
}

impl FeatureRecord for Airspace {
    const TABLE: &'static str = "airspaces";

    fn bbox(&self) -> BoundingBox {
        self.geometry.bounding_box()
    }

    fn ident(&self) -> Option<&str> {
        None
    }

    fn name(&self) -> Option<&str> {
        Some(&self.name)
    }

    fn position(&self) -> Option<LatLon> {
        None
    }

    fn into_feature(self) -> Feature {
        Feature::Airspace(self)
    }
}

impl FeatureRecord for Airport {
    const TABLE: &'static str = "airports";

    fn bbox(&self) -> BoundingBox {
        BoundingBox::around(self.position, 0.0)
    }

    fn ident(&self) -> Option<&str> {
        self.ident.as_ref().map(IcaoCode::as_str)
    }

    fn name(&self) -> Option<&str> {
        Some(&self.name)
    }

    fn position(&self) -> Option<LatLon> {
        Some(self.position)
    }

    fn into_feature(self) -> Feature {
        Feature::Airport(self)
    }
}

impl FeatureRecord for Navaid {
    const TABLE: &'static str = "navaids";

    fn bbox(&self) -> BoundingBox {
        BoundingBox::around(self.position, 0.0)
    }

    fn ident(&self) -> Option<&str> {
        Some(&self.ident)
    }

    fn name(&self) -> Option<&str> {
        Some(&self.name)
    }

    fn position(&self) -> Option<LatLon> {
        Some(self.position)
    }

    fn into_feature(self) -> Feature {
        Feature::Navaid(self)
    }
}

impl FeatureRecord for ReportingPoint {
    const TABLE: &'static str = "reporting_points";

    fn bbox(&self) -> BoundingBox {
        BoundingBox::around(self.position, 0.0)
    }

    fn ident(&self) -> Option<&str> {
        None
    }

    fn name(&self) -> Option<&str> {
        Some(&self.name)
    }

    fn position(&self) -> Option<LatLon> {
        Some(self.position)
    }

    fn into_feature(self) -> Feature {
        Feature::ReportingPoint(self)
    }
}

impl FeatureRecord for Obstacle {
    const TABLE: &'static str = "obstacles";

    fn bbox(&self) -> BoundingBox {
        BoundingBox::around(self.position, 0.0)
    }

    fn ident(&self) -> Option<&str> {
        None
    }

    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    fn position(&self) -> Option<LatLon> {
        Some(self.position)
    }

    fn into_feature(self) -> Feature {
        Feature::Obstacle(self)
    }
}

/// Replaces one country's slice of the dataset atomically: DELETE (scoped
/// to `country`) + INSERT in one transaction, base table and R*Tree in
/// lockstep. Other countries' rows are untouched — replacing DE never
/// drops AT data.
pub(super) fn replace_all<T: FeatureRecord>(
    conn: &mut Connection,
    country: Country,
    items: &[T],
) -> Result<usize, StoreError> {
    let tx = conn.transaction()?;
    // R*Tree rows first, while the base rows still identify the country.
    tx.execute(
        &format!(
            "DELETE FROM {table}_rtree
             WHERE id IN (SELECT id FROM {table} WHERE country = ?1)",
            table = T::TABLE
        ),
        params![country.code()],
    )?;
    tx.execute(
        &format!("DELETE FROM {} WHERE country = ?1", T::TABLE),
        params![country.code()],
    )?;
    {
        let mut insert = tx.prepare(&format!(
            "INSERT INTO {} (ident, name, data, country) VALUES (?1, ?2, ?3, ?4)",
            T::TABLE
        ))?;
        let mut insert_rtree = tx.prepare(&format!(
            "INSERT INTO {}_rtree (id, min_lon, max_lon, min_lat, max_lat)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            T::TABLE
        ))?;
        for item in items {
            let blob = postcard::to_stdvec(item)?;
            insert.execute(params![
                item.ident().map(str::to_uppercase),
                item.name().map(str::to_uppercase),
                blob,
                country.code(),
            ])?;
            let id = tx.last_insert_rowid();
            let bbox = item.bbox();
            insert_rtree.execute(params![
                id,
                bbox.west(),
                bbox.east(),
                bbox.south(),
                bbox.north(),
            ])?;
        }
    }
    tx.commit()?;
    tracing::debug!(
        table = T::TABLE,
        country = country.code(),
        count = items.len(),
        "dataset replaced"
    );
    Ok(items.len())
}

/// Number of features whose stored bounding box intersects `bbox`, straight
/// off the R*Tree — no blob decoding, so it stays cheap on arbitrarily large
/// stores. The stored boxes are outward-rounded f32, so the count is
/// conservative (may include edge features the exact f64 test would drop);
/// callers use it for thresholds/stats, not exact results.
pub(super) fn count_bbox<T: FeatureRecord>(
    conn: &Connection,
    bbox: BoundingBox,
) -> Result<usize, StoreError> {
    let sql = format!(
        "SELECT count(*) FROM {table}_rtree
         WHERE min_lon <= ?1 AND max_lon >= ?2
           AND min_lat <= ?3 AND max_lat >= ?4",
        table = T::TABLE
    );
    let count: i64 = conn.query_row(
        &sql,
        params![bbox.east(), bbox.west(), bbox.north(), bbox.south()],
        |row| row.get(0),
    )?;
    Ok(usize::try_from(count).unwrap_or(0))
}

/// All features whose bounding box intersects `bbox`. The R*Tree prefilter
/// is conservative (boxes are stored as outward-rounded f32), so results are
/// re-checked against the exact f64 bbox after decoding.
pub(super) fn query_bbox<T: FeatureRecord>(
    conn: &Connection,
    bbox: BoundingBox,
) -> Result<Vec<T>, StoreError> {
    let sql = format!(
        "SELECT t.data FROM {table} t
         JOIN {table}_rtree r ON r.id = t.id
         WHERE r.min_lon <= ?1 AND r.max_lon >= ?2
           AND r.min_lat <= ?3 AND r.max_lat >= ?4
         ORDER BY t.id",
        table = T::TABLE
    );
    let mut stmt = conn.prepare(&sql)?;
    let blobs = stmt
        .query_map(
            params![bbox.east(), bbox.west(), bbox.north(), bbox.south()],
            |row| row.get::<_, Vec<u8>>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let mut items = Vec::with_capacity(blobs.len());
    for blob in blobs {
        let item: T = postcard::from_bytes(&blob)?;
        if item.bbox().intersects(&bbox) {
            items.push(item);
        }
    }
    Ok(items)
}
