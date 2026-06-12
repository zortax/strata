//! Field formatting helpers for FPL items.

use strata_data::domain::LatLon;

use crate::flight::PlannedAltitude;
use crate::units::Knots;

/// `HHMM` from a duration in minutes (floored to whole minutes) — item 16
/// EET and item 19 `E/` endurance.
pub(crate) fn hhmm_duration(minutes: f64) -> String {
    let total = minutes.max(0.0).floor() as u64;
    format!("{:02}{:02}", total / 60, total % 60)
}

/// ICAO degrees-and-minutes position, e.g. `5030N00940E` (lat `ddmm` +
/// `N|S`, lon `dddmm` + `E|W`), minutes rounded with degree carry.
pub(crate) fn icao_coords(p: LatLon) -> String {
    let (lat_deg, lat_min) = degrees_minutes(p.lat().abs());
    let (lon_deg, lon_min) = degrees_minutes(p.lon().abs());
    format!(
        "{lat_deg:02}{lat_min:02}{}{lon_deg:03}{lon_min:02}{}",
        if p.lat() < 0.0 { 'S' } else { 'N' },
        if p.lon() < 0.0 { 'W' } else { 'E' },
    )
}

fn degrees_minutes(degrees_abs: f64) -> (u32, u32) {
    let total_minutes = (degrees_abs * 60.0).round() as u32;
    (total_minutes / 60, total_minutes % 60)
}

/// Item 15 cruising speed: `N` + 4-digit TAS in knots.
pub(crate) fn speed_block(tas: Knots) -> String {
    format!("N{:04}", tas.0.round() as u32)
}

/// Item 15 level: `A` + hundreds of feet for AMSL altitudes, `F` + flight
/// level, or literal `VFR` when no cruise altitude is planned.
pub(crate) fn level_block(altitude: Option<PlannedAltitude>) -> String {
    match altitude {
        Some(PlannedAltitude::Amsl(m)) => {
            format!("A{:03}", (m.as_feet() / 100.0).round() as u32)
        }
        Some(PlannedAltitude::FlightLevel(fl)) => format!("F{fl:03}"),
        None => "VFR".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use strata_data::domain::MetersAmsl;

    use super::*;

    #[test]
    fn durations_floor_to_whole_minutes() {
        assert_eq!(hhmm_duration(0.0), "0000");
        assert_eq!(hhmm_duration(79.0), "0119");
        assert_eq!(hhmm_duration(240.9), "0400");
        assert_eq!(hhmm_duration(-5.0), "0000");
    }

    #[test]
    fn coordinates_render_icao_degrees_minutes() {
        let p = |lat, lon| LatLon::new(lat, lon).expect("valid");
        // 49.75° = 49°45'; 9.5° = 009°30'.
        assert_eq!(icao_coords(p(49.75, 9.5)), "4945N00930E");
        // Southern/western hemispheres.
        assert_eq!(icao_coords(p(-33.5, -70.25)), "3330S07015W");
        // Minute rounding carries into the degrees: 49.9999° ⇒ 50°00'.
        assert_eq!(icao_coords(p(49.9999, 8.0)), "5000N00800E");
    }

    #[test]
    fn speed_and_level_blocks() {
        assert_eq!(speed_block(Knots(107.0)), "N0107");
        assert_eq!(speed_block(Knots(110.4)), "N0110");
        // 4500 ft ⇒ A045; FL 85 ⇒ F085; unplanned ⇒ VFR.
        assert_eq!(
            level_block(Some(PlannedAltitude::Amsl(MetersAmsl::from_feet(4500.0)))),
            "A045"
        );
        assert_eq!(level_block(Some(PlannedAltitude::FlightLevel(85))), "F085");
        assert_eq!(level_block(None), "VFR");
    }
}
