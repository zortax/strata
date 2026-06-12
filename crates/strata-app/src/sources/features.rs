//! [`ObstacleSource`] / [`AirspaceSource`] over the store's R*Tree bbox
//! queries.
//!
//! Both sources memoize per query bbox. A source instance lives for exactly
//! one compute run (one route generation — see `state::flight`), so the
//! memo is the "cached per route generation" of plan §5.3: the corridor
//! engine queries one padded bbox per run, and any repeat of the same bbox
//! within the run is served without touching SQLite. The memo is a plain
//! `RefCell` — sources are constructed and used on one thread by contract.

use std::cell::RefCell;
use std::sync::Arc;

use strata_data::domain::{Airspace, BoundingBox, Obstacle};
use strata_data::store::Store;
use strata_plan::sources::{AirspaceSource, ObstacleSource, SourceError};

/// Store-backed obstacles with a per-run bbox memo.
pub struct StoreObstacleSource {
    store: Arc<Store>,
    memo: RefCell<Vec<(BoundingBox, Vec<Obstacle>)>>,
}

impl StoreObstacleSource {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            memo: RefCell::new(Vec::new()),
        }
    }
}

impl ObstacleSource for StoreObstacleSource {
    fn obstacles_in_bbox(&self, bbox: BoundingBox) -> Result<Vec<Obstacle>, SourceError> {
        if let Some((_, hit)) = self.memo.borrow().iter().find(|(b, _)| *b == bbox) {
            return Ok(hit.clone());
        }
        let result = self
            .store
            .obstacles_in_bbox(bbox)
            .map_err(|err| SourceError::with_source("obstacle bbox query", err))?;
        self.memo.borrow_mut().push((bbox, result.clone()));
        Ok(result)
    }
}

/// Store-backed airspaces with a per-run bbox memo.
pub struct StoreAirspaceSource {
    store: Arc<Store>,
    memo: RefCell<Vec<(BoundingBox, Vec<Airspace>)>>,
}

impl StoreAirspaceSource {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            memo: RefCell::new(Vec::new()),
        }
    }
}

impl AirspaceSource for StoreAirspaceSource {
    fn airspaces_in_bbox(&self, bbox: BoundingBox) -> Result<Vec<Airspace>, SourceError> {
        if let Some((_, hit)) = self.memo.borrow().iter().find(|(b, _)| *b == bbox) {
            return Ok(hit.clone());
        }
        let result = self
            .store
            .airspaces_in_bbox(bbox)
            .map_err(|err| SourceError::with_source("airspace bbox query", err))?;
        self.memo.borrow_mut().push((bbox, result.clone()));
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use strata_data::domain::{
        Airspace, AirspaceClass, AirspaceKind, LatLon, ObstacleKind, Polygon, VerticalLimit,
    };

    use super::*;

    fn temp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("store.sqlite")).unwrap();
        (dir, store)
    }

    fn p(lat: f64, lon: f64) -> LatLon {
        LatLon::new(lat, lon).unwrap()
    }

    fn obstacle(lat: f64, lon: f64) -> Obstacle {
        Obstacle {
            name: Some("Mast".to_owned()),
            kind: ObstacleKind::Mast,
            position: p(lat, lon),
            height: strata_data::domain::MetersAgl(120.0),
            elevation_top: strata_data::domain::MetersAmsl(420.0),
            lighted: true,
        }
    }

    fn airspace(name: &str, west: f64, south: f64, east: f64, north: f64) -> Airspace {
        Airspace {
            name: name.to_owned(),
            class: AirspaceClass::D,
            kind: AirspaceKind::Ctr,
            lower: VerticalLimit::gnd(),
            upper: VerticalLimit::fl(100),
            geometry: Polygon::new(
                vec![
                    p(south, west),
                    p(south, east),
                    p(north, east),
                    p(north, west),
                ],
                Vec::new(),
            )
            .unwrap(),
            airac: None,
        }
    }

    #[test]
    fn obstacles_query_store_and_memoize() {
        let (_dir, mut store) = temp_store();
        store
            .insert_obstacles(strata_data::domain::Country::DE, &[obstacle(50.0, 10.0)])
            .unwrap();
        let store = Arc::new(store);

        let source = StoreObstacleSource::new(store);
        let inside = BoundingBox::new(9.5, 49.5, 10.5, 50.5).unwrap();
        let hit = source.obstacles_in_bbox(inside).unwrap();
        assert_eq!(hit.len(), 1);
        // Memoized: identical bbox served from the memo (same content).
        assert_eq!(source.obstacles_in_bbox(inside).unwrap(), hit);
        assert_eq!(source.memo.borrow().len(), 1);

        let elsewhere = BoundingBox::new(6.0, 47.0, 6.5, 47.5).unwrap();
        assert!(source.obstacles_in_bbox(elsewhere).unwrap().is_empty());
        assert_eq!(source.memo.borrow().len(), 2);
    }

    #[test]
    fn airspaces_query_store_and_memoize() {
        let (_dir, mut store) = temp_store();
        store
            .insert_airspaces(
                strata_data::domain::Country::DE,
                &[airspace("TEST CTR", 9.8, 49.8, 10.2, 50.2)],
            )
            .unwrap();
        let store = Arc::new(store);

        let source = StoreAirspaceSource::new(store);
        let inside = BoundingBox::new(9.9, 49.9, 10.1, 50.1).unwrap();
        let hit = source.airspaces_in_bbox(inside).unwrap();
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].name, "TEST CTR");
        assert_eq!(source.airspaces_in_bbox(inside).unwrap(), hit);
        assert_eq!(source.memo.borrow().len(), 1);

        let elsewhere = BoundingBox::new(6.0, 47.0, 6.5, 47.5).unwrap();
        assert!(source.airspaces_in_bbox(elsewhere).unwrap().is_empty());
    }
}
