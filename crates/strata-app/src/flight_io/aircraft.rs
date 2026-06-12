//! Aircraft profile files: `<data_dir>/aircraft/<id>.strata-aircraft`,
//! plus the two bundled example profiles written on first run.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use strata_data::domain::Meters;
use strata_plan::AircraftProfile;
use strata_plan::aircraft::{
    AircraftId, ClimbPerformance, DescentPerformance, Distances, EnvelopePoint, FuelSystem,
    FuelType, Performance, PowerSetting, StationKind, WbStation, WeightBalance,
};
use strata_plan::units::{
    FeetPerMinute, Kilograms, KilogramsPerLiter, Knots, Liters, LitersPerHour,
};

use crate::fsutil::{WriteTicket, write_atomic_ordered};

/// File extension of aircraft profiles (no leading dot).
pub const AIRCRAFT_EXTENSION: &str = "strata-aircraft";

/// The aircraft directory under the app data dir (next to `flights/`).
pub fn aircraft_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("aircraft")
}

/// The canonical file path of a profile: `<dir>/<id>.strata-aircraft`
/// (the id is the file stem — see [`AircraftId`]'s slug alphabet).
pub fn aircraft_path(dir: &Path, id: &AircraftId) -> PathBuf {
    dir.join(format!("{id}.{AIRCRAFT_EXTENSION}"))
}

/// Scans `dir` for `*.strata-aircraft` files, sorted by id. A missing
/// directory is an empty list; unreadable files are skipped with a warning.
pub fn list_aircraft(dir: &Path) -> Vec<AircraftProfile> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            tracing::warn!(dir = %dir.display(), %err, "reading aircraft directory failed");
            return Vec::new();
        }
    };
    let mut profiles = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(AIRCRAFT_EXTENSION) {
            continue;
        }
        match load_aircraft(&path) {
            Ok(profile) => profiles.push(profile),
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "skipping unreadable aircraft file");
            }
        }
    }
    profiles.sort_by(|a, b| a.id.cmp(&b.id));
    profiles
}

/// Loads one aircraft profile (versioned + tolerant).
pub fn load_aircraft(path: &Path) -> anyhow::Result<AircraftProfile> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read aircraft file {}", path.display()))?;
    AircraftProfile::from_json_str(&text)
        .with_context(|| format!("parse aircraft file {}", path.display()))
}

/// Saves `profile` to its canonical path under `dir`, atomically
/// (ordering ticket captured at call time — for synchronous callers).
pub fn save_aircraft(dir: &Path, profile: &AircraftProfile) -> anyhow::Result<()> {
    save_aircraft_ordered(dir, profile, WriteTicket::next())
}

/// [`save_aircraft`] with a caller-captured [`WriteTicket`] — detached
/// background savers capture the ticket together with the profile
/// snapshot on the UI thread, so an older snapshot can never land over a
/// newer one regardless of write completion order.
pub fn save_aircraft_ordered(
    dir: &Path,
    profile: &AircraftProfile,
    ticket: WriteTicket,
) -> anyhow::Result<()> {
    let text = profile
        .to_json_string()
        .context("serialize aircraft profile")?;
    write_atomic_ordered(&aircraft_path(dir, &profile.id), &text, ticket).map(|_| ())
}

/// Removes the profile file of `id` under `dir`. A missing file is fine —
/// the goal state (no such profile) is already true.
pub fn delete_aircraft(dir: &Path, id: &AircraftId) -> anyhow::Result<()> {
    let path = aircraft_path(dir, id);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("delete aircraft file {}", path.display())),
    }
}

/// A free aircraft id derived from `name`, deduplicated against the files
/// in `dir` (the id is the file stem): `skyhawk`, `skyhawk-2`, … — the
/// aircraft twin of [`allocate_flight_path`](super::allocate_flight_path).
pub fn allocate_aircraft_id(dir: &Path, name: &str) -> AircraftId {
    // `slug` yields the id alphabet already; truncating to 56 chars leaves
    // room for a `-NNNNNNN` dedup suffix inside the 64-char id limit.
    // Degenerate names get "aircraft", not `slug`'s flight-flavoured
    // fallback.
    let stem: String = if name.chars().any(|c| c.is_ascii_alphanumeric()) {
        super::slug(name).chars().take(56).collect()
    } else {
        "aircraft".to_owned()
    };
    let id = |s: &str| AircraftId::new(s).expect("slug stays a valid aircraft id");
    let candidate = id(&stem);
    if !aircraft_path(dir, &candidate).exists() {
        return candidate;
    }
    for n in 2.. {
        let candidate = id(&format!("{stem}-{n}"));
        if !aircraft_path(dir, &candidate).exists() {
            return candidate;
        }
    }
    unreachable!("the counter loop returns")
}

/// First-run seeding: when `dir` holds no aircraft profile yet, the two
/// bundled examples (a C172- and a DA40-class profile, both clearly marked
/// as example data) are written. Returns whether seeding happened.
///
/// "No profile yet" means no `*.strata-aircraft` file exists — a user who
/// deleted the examples but keeps their own profiles is never re-seeded.
pub fn ensure_example_aircraft(dir: &Path) -> anyhow::Result<bool> {
    fs::create_dir_all(dir).with_context(|| format!("create aircraft dir {}", dir.display()))?;
    let has_profiles = fs::read_dir(dir)
        .with_context(|| format!("read aircraft dir {}", dir.display()))?
        .flatten()
        .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some(AIRCRAFT_EXTENSION));
    if has_profiles {
        return Ok(false);
    }
    for profile in [example_c172(), example_da40()] {
        save_aircraft(dir, &profile)?;
    }
    tracing::info!(dir = %dir.display(), "seeded example aircraft profiles");
    Ok(true)
}

/// Marker appended to every bundled profile name. The values are typical
/// for the class but **not** any specific airframe's POH data.
const EXAMPLE_MARKER: &str = "EXAMPLE DATA — replace with your POH values";

/// The aircraft manager's "New aircraft" seed (design §3.5): the C172-class
/// example values under a fresh id, clearly marked as template data;
/// registration and callsign cleared for the user's own.
pub fn new_aircraft_template(id: AircraftId) -> AircraftProfile {
    AircraftProfile {
        id,
        name: Some(format!("New aircraft ({EXAMPLE_MARKER})")),
        registration: String::new(),
        callsign: String::new(),
        ..example_c172()
    }
}

/// A C172-class example profile (avgas four-seater, ~120 kt).
pub fn example_c172() -> AircraftProfile {
    AircraftProfile {
        name: Some(format!("Cessna 172S class ({EXAMPLE_MARKER})")),
        registration: "D-EXAA".to_owned(),
        type_designator: "C172".to_owned(),
        performance: Performance {
            cruise_settings: vec![
                PowerSetting {
                    name: "75 %".to_owned(),
                    tas: Knots(122.0),
                    fuel_flow: LitersPerHour(36.0),
                },
                PowerSetting {
                    name: "65 %".to_owned(),
                    tas: Knots(115.0),
                    fuel_flow: LitersPerHour(32.0),
                },
                PowerSetting {
                    name: "55 %".to_owned(),
                    tas: Knots(105.0),
                    fuel_flow: LitersPerHour(28.0),
                },
            ],
            climb: ClimbPerformance {
                ias: Knots(74.0),
                rate: FeetPerMinute(700.0),
                fuel_flow: LitersPerHour(42.0),
            },
            descent: DescentPerformance {
                ias: Knots(110.0),
                rate: FeetPerMinute(500.0),
                fuel_flow: LitersPerHour(24.0),
            },
            taxi_fuel_flow: LitersPerHour(7.0),
        },
        fuel: FuelSystem {
            usable: Liters(201.0),
            tabs: Some(Liters(132.0)),
            fuel_type: FuelType::Avgas100Ll,
            density: KilogramsPerLiter(0.72),
        },
        weight_balance: WeightBalance {
            empty_mass: Kilograms(767.0),
            empty_arm: Meters(1.00),
            stations: vec![
                WbStation {
                    name: "Front seats".to_owned(),
                    arm: Meters(0.94),
                    kind: StationKind::Seat,
                    max_load: None,
                },
                WbStation {
                    name: "Rear seats".to_owned(),
                    arm: Meters(1.85),
                    kind: StationKind::Seat,
                    max_load: None,
                },
                WbStation {
                    name: "Baggage".to_owned(),
                    arm: Meters(2.41),
                    kind: StationKind::Baggage,
                    max_load: Some(Kilograms(54.0)),
                },
                WbStation {
                    name: "Fuel".to_owned(),
                    arm: Meters(1.17),
                    kind: StationKind::Fuel,
                    max_load: None,
                },
            ],
            max_takeoff: Kilograms(1157.0),
            max_landing: None,
            max_zero_fuel: None,
            max_ramp: Some(Kilograms(1160.0)),
            envelope: vec![
                EnvelopePoint {
                    arm: Meters(0.89),
                    mass: Kilograms(680.0),
                },
                EnvelopePoint {
                    arm: Meters(0.89),
                    mass: Kilograms(885.0),
                },
                EnvelopePoint {
                    arm: Meters(1.04),
                    mass: Kilograms(1157.0),
                },
                EnvelopePoint {
                    arm: Meters(1.20),
                    mass: Kilograms(1157.0),
                },
                EnvelopePoint {
                    arm: Meters(1.20),
                    mass: Kilograms(680.0),
                },
            ],
        },
        distances: Distances {
            takeoff_roll: Meters(293.0),
            takeoff_over_50ft: Some(Meters(497.0)),
            landing_roll: Meters(175.0),
            landing_over_50ft: Some(Meters(407.0)),
            ..Distances::default()
        },
        ..AircraftProfile::new(AircraftId::new("example-c172").expect("static id is a valid slug"))
    }
}

/// A DA40-class example profile (Jet A-1 diesel four-seater, ~125 kt).
pub fn example_da40() -> AircraftProfile {
    AircraftProfile {
        name: Some(format!("Diamond DA40 NG class ({EXAMPLE_MARKER})")),
        registration: "D-EXAB".to_owned(),
        type_designator: "DA40".to_owned(),
        performance: Performance {
            cruise_settings: vec![
                PowerSetting {
                    name: "92 %".to_owned(),
                    tas: Knots(134.0),
                    fuel_flow: LitersPerHour(26.0),
                },
                PowerSetting {
                    name: "75 %".to_owned(),
                    tas: Knots(125.0),
                    fuel_flow: LitersPerHour(21.0),
                },
                PowerSetting {
                    name: "60 %".to_owned(),
                    tas: Knots(116.0),
                    fuel_flow: LitersPerHour(17.5),
                },
            ],
            climb: ClimbPerformance {
                ias: Knots(72.0),
                rate: FeetPerMinute(650.0),
                fuel_flow: LitersPerHour(28.0),
            },
            descent: DescentPerformance {
                ias: Knots(120.0),
                rate: FeetPerMinute(500.0),
                fuel_flow: LitersPerHour(12.0),
            },
            taxi_fuel_flow: LitersPerHour(5.0),
        },
        fuel: FuelSystem {
            usable: Liters(147.0),
            tabs: None,
            fuel_type: FuelType::JetA1,
            density: KilogramsPerLiter(0.80),
        },
        weight_balance: WeightBalance {
            empty_mass: Kilograms(900.0),
            empty_arm: Meters(2.40),
            stations: vec![
                WbStation {
                    name: "Front seats".to_owned(),
                    arm: Meters(2.30),
                    kind: StationKind::Seat,
                    max_load: None,
                },
                WbStation {
                    name: "Rear seats".to_owned(),
                    arm: Meters(3.25),
                    kind: StationKind::Seat,
                    max_load: None,
                },
                WbStation {
                    name: "Baggage".to_owned(),
                    arm: Meters(3.89),
                    kind: StationKind::Baggage,
                    max_load: Some(Kilograms(45.0)),
                },
                WbStation {
                    name: "Fuel".to_owned(),
                    arm: Meters(2.63),
                    kind: StationKind::Fuel,
                    max_load: None,
                },
            ],
            max_takeoff: Kilograms(1310.0),
            max_landing: None,
            max_zero_fuel: None,
            max_ramp: None,
            envelope: vec![
                EnvelopePoint {
                    arm: Meters(2.40),
                    mass: Kilograms(940.0),
                },
                EnvelopePoint {
                    arm: Meters(2.40),
                    mass: Kilograms(1080.0),
                },
                EnvelopePoint {
                    arm: Meters(2.46),
                    mass: Kilograms(1310.0),
                },
                EnvelopePoint {
                    arm: Meters(2.59),
                    mass: Kilograms(1310.0),
                },
                EnvelopePoint {
                    arm: Meters(2.59),
                    mass: Kilograms(940.0),
                },
            ],
        },
        distances: Distances {
            takeoff_roll: Meters(355.0),
            takeoff_over_50ft: Some(Meters(640.0)),
            landing_roll: Meters(290.0),
            landing_over_50ft: Some(Meters(610.0)),
            ..Distances::default()
        },
        ..AircraftProfile::new(AircraftId::new("example-da40").expect("static id is a valid slug"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn examples_are_marked_and_load_worthy() {
        for profile in [example_c172(), example_da40()] {
            let name = profile.name.clone().unwrap();
            assert!(name.contains("EXAMPLE DATA"), "{name}");
            assert!(
                !profile.performance.cruise_settings.is_empty(),
                "compute() needs at least one cruise setting"
            );
            assert!(profile.fuel.usable.0 > 0.0);
            assert!(profile.weight_balance.envelope.len() >= 3);
            // Fuel station present so W&B can place the fuel mass.
            assert!(
                profile
                    .weight_balance
                    .stations
                    .iter()
                    .any(|s| s.kind == StationKind::Fuel)
            );
            // Round-trips through the on-disk format.
            let text = profile.to_json_string().unwrap();
            assert_eq!(AircraftProfile::from_json_str(&text).unwrap(), profile);
        }
    }

    #[test]
    fn seeding_writes_examples_only_into_an_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let aircraft = aircraft_dir(dir.path());

        assert!(ensure_example_aircraft(&aircraft).unwrap(), "first run");
        let listed = list_aircraft(&aircraft);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id.as_str(), "example-c172");
        assert_eq!(listed[1].id.as_str(), "example-da40");

        assert!(!ensure_example_aircraft(&aircraft).unwrap(), "second run");

        // A user who removed the examples but keeps their own profile is
        // never re-seeded.
        for profile in &listed {
            fs::remove_file(aircraft_path(&aircraft, &profile.id)).unwrap();
        }
        let own = AircraftProfile::new(AircraftId::new("my-plane").unwrap());
        save_aircraft(&aircraft, &own).unwrap();
        assert!(!ensure_example_aircraft(&aircraft).unwrap());
        assert_eq!(list_aircraft(&aircraft).len(), 1);
    }

    /// The aircraft manager's save-on-change funnel: an edited profile
    /// written to its file loads back identical — including the envelope
    /// polygon a vertex drag rewrote and the new callsign field.
    #[test]
    fn edited_profiles_round_trip_through_their_file() {
        use strata_plan::units::Knots;

        let dir = tempfile::tempdir().unwrap();
        let aircraft = aircraft_dir(dir.path());

        let mut profile = example_c172();
        profile.callsign = "FLY23".to_owned();
        profile.name = Some("Skyhawk — club".to_owned());
        profile.performance.cruise_settings[0].tas = Knots(118.0);
        profile.fuel.tabs = None;
        profile.weight_balance.envelope[2] = EnvelopePoint {
            arm: Meters(1.05),
            mass: Kilograms(1150.0),
        };
        profile.distances.factors.per_10_kt_tailwind = 0.5;

        save_aircraft(&aircraft, &profile).unwrap();
        let loaded = load_aircraft(&aircraft_path(&aircraft, &profile.id)).unwrap();
        assert_eq!(loaded, profile);

        // Overwriting (the on-change save) replaces the content atomically.
        profile.weight_balance.envelope.push(EnvelopePoint {
            arm: Meters(1.10),
            mass: Kilograms(600.0),
        });
        save_aircraft(&aircraft, &profile).unwrap();
        let reloaded = load_aircraft(&aircraft_path(&aircraft, &profile.id)).unwrap();
        assert_eq!(reloaded.weight_balance.envelope.len(), 6);
        assert_eq!(reloaded, profile);
    }

    #[test]
    fn new_aircraft_template_is_marked_and_fresh() {
        let id = AircraftId::new("my-plane").unwrap();
        let template = new_aircraft_template(id.clone());
        assert_eq!(template.id, id);
        assert!(template.name.as_deref().unwrap().contains("EXAMPLE DATA"));
        assert!(template.registration.is_empty());
        assert!(template.callsign.is_empty());
        // The seed keeps the C172-class numbers (design §3.5: "seeded from
        // the C172-class example").
        let c172 = example_c172();
        assert_eq!(template.performance, c172.performance);
        assert_eq!(template.weight_balance, c172.weight_balance);
        assert_eq!(template.fuel, c172.fuel);
    }

    #[test]
    fn allocate_aircraft_id_slugs_and_deduplicates() {
        let dir = tempfile::tempdir().unwrap();
        let aircraft = aircraft_dir(dir.path());
        fs::create_dir_all(&aircraft).unwrap();

        let first = allocate_aircraft_id(&aircraft, "Skyhawk — club");
        assert_eq!(first.as_str(), "skyhawk-club");
        let mut profile = AircraftProfile::new(first);
        save_aircraft(&aircraft, &profile).unwrap();

        let second = allocate_aircraft_id(&aircraft, "Skyhawk — club");
        assert_eq!(second.as_str(), "skyhawk-club-2");
        profile.id = second;
        save_aircraft(&aircraft, &profile).unwrap();
        assert_eq!(
            allocate_aircraft_id(&aircraft, "Skyhawk — club").as_str(),
            "skyhawk-club-3"
        );

        // Degenerate and over-long names still produce valid ids.
        assert_eq!(allocate_aircraft_id(&aircraft, "→→→").as_str(), "aircraft");
        let long = allocate_aircraft_id(&aircraft, &"x".repeat(200));
        assert_eq!(long.as_str(), "x".repeat(56));
    }

    #[test]
    fn delete_aircraft_removes_the_file_and_tolerates_absence() {
        let dir = tempfile::tempdir().unwrap();
        let aircraft = aircraft_dir(dir.path());
        let profile = example_c172();
        save_aircraft(&aircraft, &profile).unwrap();
        assert!(aircraft_path(&aircraft, &profile.id).exists());

        delete_aircraft(&aircraft, &profile.id).unwrap();
        assert!(!aircraft_path(&aircraft, &profile.id).exists());
        // Idempotent: the goal state is already true.
        delete_aircraft(&aircraft, &profile.id).unwrap();
    }

    #[test]
    fn list_skips_unreadable_files() {
        let dir = tempfile::tempdir().unwrap();
        let aircraft = aircraft_dir(dir.path());
        assert!(list_aircraft(&aircraft).is_empty(), "missing dir is empty");
        fs::create_dir_all(&aircraft).unwrap();
        fs::write(aircraft.join("broken.strata-aircraft"), "nope").unwrap();
        save_aircraft(&aircraft, &example_c172()).unwrap();
        let listed = list_aircraft(&aircraft);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id.as_str(), "example-c172");
    }
}
