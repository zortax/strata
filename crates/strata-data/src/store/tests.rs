//! Store integration tests (tempfile-backed).

use chrono::{NaiveDate, TimeZone, Utc};
use tempfile::TempDir;

use super::*;
use crate::domain::{
    AirportKind, AirspaceClass, AirspaceKind, Frequency, FrequencyKind, IcaoCode, Meters,
    MetersAgl, MetersAmsl, NavaidKind, ObstacleKind, Polygon, RadioFrequency, Runway,
    RunwaySurface, VerticalLimit,
};

/// Canary: the bundled SQLite build must ship the R*Tree module — the whole
/// schema design depends on it.
#[test]
fn bundled_sqlite_has_rtree() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE VIRTUAL TABLE canary USING rtree(id, min_lon, max_lon, min_lat, max_lat);
         INSERT INTO canary VALUES (1, 8.0, 9.0, 49.0, 50.0);",
    )
    .unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM canary WHERE min_lon <= 8.5 AND max_lon >= 8.5",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
}

// --- fixtures -------------------------------------------------------------

fn ll(lat: f64, lon: f64) -> LatLon {
    LatLon::new(lat, lon).unwrap()
}

/// Opens a store under a nested, not-yet-existing directory (exercises
/// directory creation in `open`).
fn open_store() -> (TempDir, Store) {
    let dir = TempDir::new().unwrap();
    let store = Store::open(&dir.path().join("data/nested/store.sqlite")).unwrap();
    (dir, store)
}

fn square(south: f64, west: f64, north: f64, east: f64) -> Polygon {
    Polygon::new(
        vec![
            ll(south, west),
            ll(south, east),
            ll(north, east),
            ll(north, west),
        ],
        vec![],
    )
    .unwrap()
}

fn airspace(name: &str, class: AirspaceClass, kind: AirspaceKind, geometry: Polygon) -> Airspace {
    Airspace {
        name: name.to_owned(),
        class,
        kind,
        lower: VerticalLimit::gnd(),
        upper: VerticalLimit::amsl(MetersAmsl::from_feet(3500.0)),
        geometry,
        airac: Some(AiracCycle::new(
            "2506",
            NaiveDate::from_ymd_opt(2025, 6, 12).unwrap(),
        )),
    }
}

fn eddf() -> Airport {
    Airport {
        ident: Some(IcaoCode::new("EDDF").unwrap()),
        name: "Frankfurt/Main".to_owned(),
        kind: AirportKind::International,
        position: ll(50.0379, 8.5622),
        elevation: MetersAmsl::from_feet(364.0),
        runways: vec![Runway {
            designator: "07C".to_owned(),
            true_heading_deg: Some(69),
            length: Some(Meters(4000.0)),
            width: Some(Meters(60.0)),
            surface: RunwaySurface::Asphalt,
            main: true,
        }],
        frequencies: vec![Frequency {
            frequency: RadioFrequency::from_mhz(119.9),
            name: "FRANKFURT TOWER".to_owned(),
            kind: FrequencyKind::Tower,
            primary: true,
        }],
    }
}

fn eddb() -> Airport {
    Airport {
        ident: Some(IcaoCode::new("EDDB").unwrap()),
        name: "Berlin Brandenburg".to_owned(),
        kind: AirportKind::International,
        position: ll(52.3667, 13.5033),
        elevation: MetersAmsl::from_feet(157.0),
        runways: vec![],
        frequencies: vec![],
    }
}

fn ffm_navaid() -> Navaid {
    Navaid {
        ident: "FFM".to_owned(),
        name: "Frankfurt".to_owned(),
        kind: NavaidKind::VorDme,
        frequency: Some(RadioFrequency::from_mhz(114.9)),
        channel: Some("96X".to_owned()),
        position: ll(50.0528, 8.6411),
        elevation: MetersAmsl(110.0),
    }
}

fn reporting_point_november() -> ReportingPoint {
    ReportingPoint {
        name: "NOVEMBER".to_owned(),
        mandatory: true,
        position: ll(50.10, 8.50),
        airports: vec![IcaoCode::new("EDDF").unwrap()],
    }
}

fn wind_turbine() -> Obstacle {
    Obstacle {
        name: Some("Windpark Hofgut".to_owned()),
        kind: ObstacleKind::WindTurbine,
        position: ll(50.01, 8.61),
        height: MetersAgl(150.0),
        elevation_top: MetersAmsl(260.0),
        lighted: true,
    }
}

fn germany() -> BoundingBox {
    BoundingBox::new(5.5, 47.0, 15.5, 55.2).unwrap()
}

// --- round trips ----------------------------------------------------------

#[test]
fn airspace_round_trip() {
    let (_dir, mut store) = open_store();
    let ctr = airspace(
        "FRANKFURT CTR",
        AirspaceClass::D,
        AirspaceKind::Ctr,
        square(49.9, 8.4, 50.15, 8.75),
    );
    assert_eq!(
        store
            .insert_airspaces(Country::DE, std::slice::from_ref(&ctr))
            .unwrap(),
        1
    );
    let got = store.airspaces_in_bbox(germany()).unwrap();
    assert_eq!(got, vec![ctr]);
}

#[test]
fn airport_round_trip_survives_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("store.sqlite");
    let airport = eddf();
    {
        let mut store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .insert_airports(Country::DE, std::slice::from_ref(&airport))
                .unwrap(),
            1
        );
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(store.airports_in_bbox(germany()).unwrap(), vec![airport]);
}

#[test]
fn navaid_round_trip() {
    let (_dir, mut store) = open_store();
    let navaid = ffm_navaid();
    store
        .insert_navaids(Country::DE, std::slice::from_ref(&navaid))
        .unwrap();
    assert_eq!(store.navaids_in_bbox(germany()).unwrap(), vec![navaid]);
}

#[test]
fn reporting_point_round_trip() {
    let (_dir, mut store) = open_store();
    let point = reporting_point_november();
    store
        .insert_reporting_points(Country::DE, std::slice::from_ref(&point))
        .unwrap();
    assert_eq!(
        store.reporting_points_in_bbox(germany()).unwrap(),
        vec![point]
    );
}

#[test]
fn obstacle_round_trip() {
    let (_dir, mut store) = open_store();
    let obstacle = wind_turbine();
    store
        .insert_obstacles(Country::DE, std::slice::from_ref(&obstacle))
        .unwrap();
    assert_eq!(store.obstacles_in_bbox(germany()).unwrap(), vec![obstacle]);
}

// --- bbox queries ---------------------------------------------------------

#[test]
fn bbox_query_filters_by_position() {
    let (_dir, mut store) = open_store();
    store
        .insert_airports(Country::DE, &[eddf(), eddb()])
        .unwrap();

    let around_frankfurt = BoundingBox::new(8.0, 49.5, 9.0, 50.5).unwrap();
    let got = store.airports_in_bbox(around_frankfurt).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].name, "Frankfurt/Main");

    assert_eq!(store.airports_in_bbox(germany()).unwrap().len(), 2);

    let north_sea = BoundingBox::new(6.0, 54.5, 8.0, 55.0).unwrap();
    assert!(store.airports_in_bbox(north_sea).unwrap().is_empty());
}

#[test]
fn bbox_query_returns_partially_overlapping_airspace() {
    let (_dir, mut store) = open_store();
    let ctr = airspace(
        "FRANKFURT CTR",
        AirspaceClass::D,
        AirspaceKind::Ctr,
        square(49.9, 8.4, 50.15, 8.75),
    );
    store.insert_airspaces(Country::DE, &[ctr]).unwrap();

    // Corner overlap only.
    let corner = BoundingBox::new(8.7, 50.1, 9.5, 50.5).unwrap();
    assert_eq!(store.airspaces_in_bbox(corner).unwrap().len(), 1);

    let disjoint = BoundingBox::new(12.0, 53.0, 13.0, 54.0).unwrap();
    assert!(store.airspaces_in_bbox(disjoint).unwrap().is_empty());
}

#[test]
fn airspace_count_matches_bbox_query() {
    let (_dir, mut store) = open_store();
    store
        .insert_airspaces(
            Country::DE,
            &[
                airspace(
                    "FRANKFURT CTR",
                    AirspaceClass::D,
                    AirspaceKind::Ctr,
                    square(49.9, 8.4, 50.15, 8.75),
                ),
                airspace(
                    "BERLIN CTR",
                    AirspaceClass::D,
                    AirspaceKind::Ctr,
                    square(52.3, 13.3, 52.5, 13.7),
                ),
            ],
        )
        .unwrap();

    assert_eq!(store.airspace_count_in_bbox(germany()).unwrap(), 2);
    let around_frankfurt = BoundingBox::new(8.0, 49.5, 9.0, 50.5).unwrap();
    assert_eq!(store.airspace_count_in_bbox(around_frankfurt).unwrap(), 1);
    let north_sea = BoundingBox::new(6.0, 54.5, 8.0, 55.0).unwrap();
    assert_eq!(store.airspace_count_in_bbox(north_sea).unwrap(), 0);
}

// --- replace-all semantics ------------------------------------------------

#[test]
fn insert_replaces_dataset_atomically() {
    let (_dir, mut store) = open_store();
    store
        .insert_airports(Country::DE, &[eddf(), eddb()])
        .unwrap();
    assert_eq!(store.airports_in_bbox(germany()).unwrap().len(), 2);

    let edhl = Airport {
        ident: Some(IcaoCode::new("EDHL").unwrap()),
        name: "Lübeck-Blankensee".to_owned(),
        kind: AirportKind::Regional,
        position: ll(53.8054, 10.7192),
        elevation: MetersAmsl::from_feet(53.0),
        runways: vec![],
        frequencies: vec![],
    };
    assert_eq!(
        store
            .insert_airports(Country::DE, std::slice::from_ref(&edhl))
            .unwrap(),
        1
    );

    let got = store.airports_in_bbox(germany()).unwrap();
    assert_eq!(got, vec![edhl]);
    // The R*Tree mirror must be replaced too: nothing left near Frankfurt.
    let around_frankfurt = BoundingBox::new(8.0, 49.5, 9.0, 50.5).unwrap();
    assert!(store.airports_in_bbox(around_frankfurt).unwrap().is_empty());
}

// --- hit-testing ----------------------------------------------------------

#[test]
fn feature_at_returns_stacked_airspaces_and_nearby_points_by_distance() {
    let (_dir, mut store) = open_store();
    let click = ll(50.0, 8.6);

    store
        .insert_airspaces(
            Country::DE,
            &[
                airspace(
                    "FRANKFURT CTR",
                    AirspaceClass::D,
                    AirspaceKind::Ctr,
                    square(49.9, 8.4, 50.15, 8.75),
                ),
                // Stacked C over E, both containing the click point.
                airspace(
                    "FRANKFURT C",
                    AirspaceClass::C,
                    AirspaceKind::Other(7),
                    square(49.5, 8.0, 50.5, 9.2),
                ),
                airspace(
                    "LANGEN E",
                    AirspaceClass::E,
                    AirspaceKind::Other(7),
                    square(49.0, 7.5, 51.0, 10.0),
                ),
                // Its bbox (49..53, 8..12) contains the click point, the
                // triangle itself (lat+lon >= 61 half-plane) does not —
                // verifies the exact polygon test after the R*Tree prefilter.
                airspace(
                    "TRIANGLE D",
                    AirspaceClass::D,
                    AirspaceKind::Danger,
                    Polygon::new(vec![ll(53.0, 12.0), ll(53.0, 8.0), ll(49.0, 12.0)], vec![])
                        .unwrap(),
                ),
            ],
        )
        .unwrap();
    store.insert_airports(Country::DE, &[eddf()]).unwrap(); // distance ~0.054°
    store
        .insert_navaids(Country::DE, &[ffm_navaid()]) // distance ~0.067°
        .unwrap();
    store
        .insert_reporting_points(Country::DE, &[reporting_point_november()]) // ~0.141°, outside tolerance
        .unwrap();
    store
        .insert_obstacles(Country::DE, &[wind_turbine()])
        .unwrap(); // ~0.014°

    let hits = store.feature_at(click, 0.08).unwrap();

    let airspace_names: Vec<&str> = hits
        .iter()
        .filter_map(|f| match f {
            Feature::Airspace(a) => Some(a.name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(airspace_names, ["FRANKFURT CTR", "FRANKFURT C", "LANGEN E"]);

    // Airspaces first, then point features ordered by increasing distance.
    let point_names: Vec<&str> = hits[airspace_names.len()..]
        .iter()
        .map(Feature::name)
        .collect();
    assert_eq!(
        point_names,
        ["Windpark Hofgut", "Frankfurt/Main", "Frankfurt"]
    );
}

#[test]
fn feature_at_misses_outside_everything() {
    let (_dir, mut store) = open_store();
    store.insert_airports(Country::DE, &[eddf()]).unwrap();
    assert!(store.feature_at(ll(53.0, 13.0), 0.05).unwrap().is_empty());
}

// --- search ---------------------------------------------------------------

#[test]
fn search_ranks_exact_ident_then_prefix_then_name() {
    let (_dir, mut store) = open_store();
    store
        .insert_airports(Country::DE, &[eddb(), eddf()])
        .unwrap();
    store
        .insert_navaids(
            Country::DE,
            &[
                // Synthetic longer ident so "EDDF" is a proper prefix.
                Navaid {
                    ident: "EDDFN".to_owned(),
                    name: "Frankfurt Practice".to_owned(),
                    kind: NavaidKind::Ndb,
                    frequency: Some(RadioFrequency::from_khz(341.0)),
                    channel: None,
                    position: ll(50.2, 8.4),
                    elevation: MetersAmsl(120.0),
                },
                Navaid {
                    ident: "MUN".to_owned(),
                    name: "München".to_owned(),
                    kind: NavaidKind::VorDme,
                    frequency: Some(RadioFrequency::from_mhz(112.3)),
                    channel: Some("70X".to_owned()),
                    position: ll(48.18, 11.82),
                    elevation: MetersAmsl(450.0),
                },
            ],
        )
        .unwrap();
    store
        .insert_airspaces(
            Country::DE,
            &[airspace(
                "EDDF CTR",
                AirspaceClass::D,
                AirspaceKind::Ctr,
                square(49.9, 8.4, 50.15, 8.75),
            )],
        )
        .unwrap();

    let hits = store.search("eddf", 10).unwrap();
    let labels: Vec<&str> = hits.iter().map(|h| h.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "EDDF — Frankfurt/Main",      // exact ident
            "EDDFN — Frankfurt Practice", // ident prefix
            "EDDF CTR",                   // name substring
        ]
    );
    assert!(matches!(hits[0].feature, Feature::Airport(_)));
    assert_eq!(hits[0].position, ll(50.0379, 8.5622));

    // Prefix query matches both airports ahead of any name match.
    let labels: Vec<String> = store
        .search("EDD", 10)
        .unwrap()
        .into_iter()
        .map(|h| h.label)
        .collect();
    assert_eq!(
        labels,
        [
            "EDDB — Berlin Brandenburg",
            "EDDF — Frankfurt/Main",
            "EDDFN — Frankfurt Practice",
            "EDDF CTR",
        ]
    );

    // Case-insensitive beyond ASCII (Unicode uppercasing at insert + query).
    let hits = store.search("münchen", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].label, "MUN — München");
}

#[test]
fn search_respects_limit_and_empty_query() {
    let (_dir, mut store) = open_store();
    store
        .insert_airports(Country::DE, &[eddf(), eddb()])
        .unwrap();
    assert_eq!(store.search("EDD", 1).unwrap().len(), 1);
    assert!(store.search("   ", 10).unwrap().is_empty());
    assert!(store.search("EDD", 0).unwrap().is_empty());
    assert!(store.search("ZZZZ", 10).unwrap().is_empty());
}

// --- terrain tiles ----------------------------------------------------------

#[test]
fn terrain_tile_round_trip() {
    let (_dir, mut store) = open_store();
    assert_eq!(store.terrain_tile(7, 66, 43).unwrap(), None);

    store.put_terrain_tile(7, 66, 43, b"png-one").unwrap();
    assert_eq!(
        store.terrain_tile(7, 66, 43).unwrap().as_deref(),
        Some(b"png-one".as_slice())
    );
    assert_eq!(store.terrain_tile(7, 66, 44).unwrap(), None);
    assert_eq!(store.terrain_tile(8, 66, 43).unwrap(), None);

    // Re-putting the same tile replaces it.
    store.put_terrain_tile(7, 66, 43, b"png-two").unwrap();
    assert_eq!(
        store.terrain_tile(7, 66, 43).unwrap().as_deref(),
        Some(b"png-two".as_slice())
    );
}

// --- elevation tiles --------------------------------------------------------

const ELEVATION_CELLS: usize = ELEVATION_TILE_SIDE * ELEVATION_TILE_SIDE;

fn uniform_elevation(id: ElevationTileId, value: i16) -> ElevationTile {
    ElevationTile::new(id, vec![value; ELEVATION_CELLS]).unwrap()
}

#[test]
fn elevation_tile_round_trip_preserves_cells_and_sentinel() {
    let (_dir, mut store) = open_store();
    let id = ElevationTileId::containing(50.5, 10.5);
    assert_eq!(store.elevation_tile(id).unwrap(), None);

    let mut cells = vec![ELEVATION_NO_DATA; ELEVATION_CELLS];
    cells[0] = -430;
    cells[1] = 0;
    cells[ELEVATION_CELLS - 1] = 2962;
    let tile = ElevationTile::new(id, cells).unwrap();
    store.put_elevation_tile(&tile).unwrap();

    assert_eq!(store.elevation_tile(id).unwrap(), Some(tile));

    // Re-putting replaces.
    let other = uniform_elevation(id, 7);
    store.put_elevation_tile(&other).unwrap();
    assert_eq!(store.elevation_tile(id).unwrap(), Some(other));
}

#[test]
fn elevation_tiles_in_bbox_returns_only_intersecting_tiles() {
    let (_dir, mut store) = open_store();
    let near = ElevationTileId::containing(50.5, 10.5);
    let far = ElevationTileId::containing(48.0, 13.0);
    store
        .put_elevation_tile(&uniform_elevation(near, 1))
        .unwrap();
    store
        .put_elevation_tile(&uniform_elevation(far, 2))
        .unwrap();

    let hits = store
        .elevation_tiles_in_bbox(BoundingBox::new(10.4, 50.4, 10.6, 50.6).unwrap())
        .unwrap();
    assert_eq!(
        hits.iter().map(ElevationTile::id).collect::<Vec<_>>(),
        vec![near]
    );

    let all = store.elevation_tiles_in_bbox(germany()).unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn max_elevation_at_reads_data_and_reports_none_for_sentinel_or_missing() {
    let (_dir, mut store) = open_store();
    let id = ElevationTileId::containing(50.5, 10.5);
    store
        .put_elevation_tile(&uniform_elevation(id, 873))
        .unwrap();

    assert_eq!(
        store.max_elevation_at(50.5, 10.5).unwrap(),
        Some(MetersAmsl(873.0))
    );
    // No tile there at all.
    assert_eq!(store.max_elevation_at(47.5, 7.5).unwrap(), None);

    // Sentinel cells inside a stored tile.
    let hole = ElevationTileId::containing(48.0, 13.0);
    store
        .put_elevation_tile(&uniform_elevation(hole, ELEVATION_NO_DATA))
        .unwrap();
    assert_eq!(store.max_elevation_at(48.0, 13.0).unwrap(), None);
}

#[test]
fn elevation_tile_set_loads_from_store() {
    let (_dir, mut store) = open_store();
    let id = ElevationTileId::containing(50.5, 10.5);
    store
        .put_elevation_tile(&uniform_elevation(id, 421))
        .unwrap();

    let set = ElevationTileSet::from_store(&store, germany()).unwrap();
    assert_eq!(set.tile_count(), 1);
    assert_eq!(set.max_elevation_at(50.5, 10.5), Some(MetersAmsl(421.0)));
    assert_eq!(set.max_elevation_at(53.0, 10.5), None);
}

/// Existing v1 stores (hillshade-era installs) gain the elevation table on
/// open and serve the new API.
#[test]
fn v1_store_migrates_to_v2_on_open() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("store.sqlite");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        schema::apply_v1(&conn).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }

    let mut store = Store::open(&path).unwrap();
    let id = ElevationTileId::containing(50.5, 10.5);
    assert_eq!(store.elevation_tile(id).unwrap(), None);
    store
        .put_elevation_tile(&uniform_elevation(id, 99))
        .unwrap();
    assert_eq!(
        store.max_elevation_at(50.5, 10.5).unwrap(),
        Some(MetersAmsl(99.0))
    );
}

// --- dataset meta -----------------------------------------------------------

#[test]
fn dataset_meta_round_trip() {
    let (_dir, mut store) = open_store();
    assert_eq!(
        store.dataset_meta(Dataset::Airports, Country::DE).unwrap(),
        None
    );

    let meta = DatasetMeta {
        dataset: Dataset::Airports,
        country: Country::DE,
        source: "openaip".to_owned(),
        airac: Some(AiracCycle::new(
            "2506",
            NaiveDate::from_ymd_opt(2025, 6, 12).unwrap(),
        )),
        ingested_at: Utc.with_ymd_and_hms(2026, 6, 10, 12, 30, 0).unwrap(),
    };
    store.put_dataset_meta(&meta).unwrap();
    assert_eq!(
        store.dataset_meta(Dataset::Airports, Country::DE).unwrap(),
        Some(meta.clone())
    );
    assert_eq!(
        store.dataset_meta(Dataset::Airports, Country::AT).unwrap(),
        None,
        "meta is keyed per country"
    );
    assert_eq!(
        store.dataset_meta(Dataset::Navaids, Country::DE).unwrap(),
        None
    );

    // Upsert: a re-ingest overwrites, including dropping the AIRAC cycle.
    let newer = DatasetMeta {
        dataset: Dataset::Airports,
        country: Country::DE,
        source: "openaip".to_owned(),
        airac: None,
        ingested_at: Utc.with_ymd_and_hms(2026, 7, 1, 8, 0, 0).unwrap(),
    };
    store.put_dataset_meta(&newer).unwrap();
    assert_eq!(
        store.dataset_meta(Dataset::Airports, Country::DE).unwrap(),
        Some(newer.clone())
    );

    // A second country's row coexists; per-dataset listing sees both.
    let at = DatasetMeta {
        country: Country::AT,
        ..meta
    };
    store.put_dataset_meta(&at).unwrap();
    let all = store.dataset_metas(Dataset::Airports).unwrap();
    assert_eq!(all, vec![at.clone(), newer]);
    // Summary: the AT row is the only one carrying an AIRAC cycle.
    assert_eq!(
        store.dataset_meta_summary(Dataset::Airports).unwrap(),
        Some(at)
    );
}

// --- country scoping ---------------------------------------------------------

/// An Austrian fixture for the multi-country tests.
fn lowi() -> Airport {
    Airport {
        ident: Some(IcaoCode::new("LOWI").unwrap()),
        name: "Innsbruck".to_owned(),
        kind: AirportKind::International,
        position: ll(47.2602, 11.3439),
        elevation: MetersAmsl(581.0),
        runways: vec![],
        frequencies: vec![],
    }
}

fn europe() -> BoundingBox {
    BoundingBox::new(-25.0, 34.0, 32.0, 72.0).unwrap()
}

/// The binding rule of schema v3: replace-all is scoped per (dataset,
/// country) — re-ingesting DE must never touch AT rows (and vice versa).
#[test]
fn replace_all_is_isolated_per_country() {
    let (_dir, mut store) = open_store();
    store
        .insert_airports(Country::DE, &[eddf(), eddb()])
        .unwrap();
    store.insert_airports(Country::AT, &[lowi()]).unwrap();
    assert_eq!(store.airports_in_bbox(europe()).unwrap().len(), 3);

    // Replace DE with a single airport: AT survives untouched.
    store.insert_airports(Country::DE, &[eddf()]).unwrap();
    let got = store.airports_in_bbox(europe()).unwrap();
    assert_eq!(got.len(), 2);
    assert!(got.contains(&eddf()));
    assert!(got.contains(&lowi()), "AT rows must survive a DE replace");

    // Replace DE with nothing: still only AT data left.
    store.insert_airports(Country::DE, &[]).unwrap();
    assert_eq!(store.airports_in_bbox(europe()).unwrap(), vec![lowi()]);

    // The R*Tree stayed in lockstep: search still works and finds exactly
    // the surviving feature.
    let hits = store.search("LOWI", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].feature, Feature::Airport(lowi()));
    let de_hits = store.search("EDDF", 10).unwrap();
    assert!(de_hits.is_empty(), "replaced DE rows are gone from search");
}

#[test]
fn airspace_replace_is_isolated_per_country() {
    let (_dir, mut store) = open_store();
    let de = airspace(
        "FRANKFURT CTR",
        AirspaceClass::D,
        AirspaceKind::Ctr,
        square(49.9, 8.4, 50.15, 8.75),
    );
    let at = airspace(
        "INNSBRUCK CTR",
        AirspaceClass::D,
        AirspaceKind::Ctr,
        square(47.2, 11.2, 47.4, 11.5),
    );
    store
        .insert_airspaces(Country::DE, std::slice::from_ref(&de))
        .unwrap();
    store
        .insert_airspaces(Country::AT, std::slice::from_ref(&at))
        .unwrap();

    // Deleting/replacing DE airspaces must not touch AT rows.
    store.insert_airspaces(Country::DE, &[]).unwrap();
    assert_eq!(store.airspaces_in_bbox(europe()).unwrap(), vec![at]);
}

// --- v2 → v3 migration --------------------------------------------------------

/// Builds a v2-era store by hand: v1+v2 DDL, one airport row (old column
/// shape, no `country`), one old-shape meta row. Mirrors what a real
/// pre-multi-country install holds.
fn build_synthetic_v2_store(path: &std::path::Path) {
    let conn = rusqlite::Connection::open(path).unwrap();
    schema::apply_v1(&conn).unwrap();
    schema::apply_v2(&conn).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2);

    let airport = eddf();
    let blob = postcard::to_stdvec(&airport).unwrap();
    conn.execute(
        "INSERT INTO airports (ident, name, data) VALUES (?1, ?2, ?3)",
        rusqlite::params!["EDDF", "FRANKFURT/MAIN", blob],
    )
    .unwrap();
    let id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO airports_rtree (id, min_lon, max_lon, min_lat, max_lat)
         VALUES (?1, 8.5622, 8.5622, 50.0379, 50.0379)",
        rusqlite::params![id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO meta (dataset, source, airac_id, airac_effective, ingested_at)
         VALUES ('airports', 'openAIP', '2506', '2025-06-12', '2026-06-10T12:30:00+00:00')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO terrain_tiles (z, x, y, data) VALUES (5, 16, 10, x'01')",
        [],
    )
    .unwrap();
}

/// The lossless v2 → v3 migration: existing data survives, meta and
/// feature rows are backfilled as DE, and the new per-country API works.
#[test]
fn v2_store_migrates_to_v3_with_de_backfill() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("store.sqlite");
    build_synthetic_v2_store(&path);

    let mut store = Store::open(&path).unwrap();

    // Pre-existing data is intact and reachable.
    assert_eq!(store.airports_in_bbox(germany()).unwrap(), vec![eddf()]);
    assert_eq!(store.terrain_tile(5, 16, 10).unwrap(), Some(vec![0x01]));

    // The old meta row now reads as (airports, DE), losslessly.
    let meta = store
        .dataset_meta(Dataset::Airports, Country::DE)
        .unwrap()
        .expect("meta row backfilled as DE");
    assert_eq!(meta.country, Country::DE);
    assert_eq!(meta.source, "openAIP");
    assert_eq!(
        meta.airac,
        Some(AiracCycle::new(
            "2506",
            NaiveDate::from_ymd_opt(2025, 6, 12).unwrap()
        ))
    );
    assert_eq!(
        meta.ingested_at,
        Utc.with_ymd_and_hms(2026, 6, 10, 12, 30, 0).unwrap()
    );
    assert_eq!(
        store.dataset_meta(Dataset::Airports, Country::AT).unwrap(),
        None
    );

    // Feature rows were backfilled as DE: an AT ingest must not disturb
    // them, while a DE replace targets exactly them.
    store.insert_airports(Country::AT, &[lowi()]).unwrap();
    assert_eq!(store.airports_in_bbox(europe()).unwrap().len(), 2);
    store.insert_airports(Country::DE, &[eddb()]).unwrap();
    let got = store.airports_in_bbox(europe()).unwrap();
    assert_eq!(got.len(), 2);
    assert!(got.contains(&eddb()), "DE replace replaced the v2 row");
    assert!(got.contains(&lowi()), "AT survived the DE replace");

    // Version landed at the current schema.
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3);
}

// --- schema guard -----------------------------------------------------------

#[test]
fn open_rejects_newer_schema_version() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("store.sqlite");
    drop(Store::open(&path).unwrap());

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA user_version = 99;").unwrap();
    drop(conn);

    match Store::open(&path).map(|_| ()) {
        Err(StoreError::Schema(msg)) => assert!(msg.contains("99"), "{msg}"),
        other => panic!("expected schema error, got {other:?}"),
    }
}

/// The store must be shareable across reader threads (`Send + Sync`).
#[test]
fn store_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Store>();
}
