//! Warn-not-block profile validation (design §3.5): odd values render as
//! an amber list at the top of the editor, but every state remains
//! saveable — the user may legitimately be mid-entry. The only *blocked*
//! mutation is deleting envelope vertices below three (enforced in the
//! envelope editor itself).

use strata_plan::AircraftProfile;
use strata_plan::aircraft::StationKind;

/// Human-readable warnings about `profile`, in section order.
pub(super) fn warnings(profile: &AircraftProfile) -> Vec<String> {
    let mut out = Vec::new();

    if profile
        .name
        .as_deref()
        .is_some_and(|n| n.contains("EXAMPLE DATA"))
    {
        out.push(
            "This profile carries example data — replace every value with your POH's before \
             planning with it."
                .to_owned(),
        );
    }

    // Identity (FPL needs these eventually).
    if profile.registration.trim().is_empty() && profile.callsign.trim().is_empty() {
        out.push("No registration or callsign — the ICAO FPL (item 7) needs one.".to_owned());
    }
    if profile.type_designator.trim().is_empty() {
        out.push("No ICAO type designator — the ICAO FPL (item 9) needs one.".to_owned());
    }

    // Performance.
    if profile.performance.cruise_settings.is_empty() {
        out.push("No cruise power setting — flights with this aircraft cannot compute.".to_owned());
    }
    for setting in &profile.performance.cruise_settings {
        if setting.tas.0 <= 0.0 || setting.fuel_flow.0 <= 0.0 {
            out.push(format!(
                "Cruise setting \"{}\" has a zero TAS or fuel flow.",
                setting.name
            ));
        }
    }
    let climb = profile.performance.climb;
    if climb.ias.0 <= 0.0 || climb.rate.0 <= 0.0 || climb.fuel_flow.0 <= 0.0 {
        out.push("Climb IAS, rate and fuel flow must all be set for the climb phase.".to_owned());
    }
    let descent = profile.performance.descent;
    if descent.ias.0 <= 0.0 || descent.rate.0 <= 0.0 {
        out.push("Descent IAS and rate must be set for the descent phase.".to_owned());
    }

    // Fuel.
    if profile.fuel.usable.0 <= 0.0 {
        out.push("Usable fuel is zero — fuel planning cannot work.".to_owned());
    }
    if let Some(tabs) = profile.fuel.tabs
        && tabs.0 > profile.fuel.usable.0
    {
        out.push("The tab level exceeds the usable fuel capacity.".to_owned());
    }
    if !(0.5..=1.1).contains(&profile.fuel.density.0) {
        out.push(format!(
            "Fuel density {} kg/L looks wrong (avgas ≈ 0.72, Jet A-1 ≈ 0.80).",
            super::fields::format_num(profile.fuel.density.0, 3)
        ));
    }

    // Weight & balance.
    let wb = &profile.weight_balance;
    if wb.empty_mass.0 <= 0.0 {
        out.push("Empty mass is zero.".to_owned());
    }
    if wb.max_takeoff.0 > 0.0 && wb.empty_mass.0 > wb.max_takeoff.0 {
        out.push("Empty mass exceeds MTOW.".to_owned());
    }
    if let Some(mlw) = wb.max_landing
        && mlw.0 > wb.max_takeoff.0
        && wb.max_takeoff.0 > 0.0
    {
        out.push("MLW exceeds MTOW — usually it is at or below it.".to_owned());
    }
    if let Some(mzfw) = wb.max_zero_fuel
        && mzfw.0 > wb.max_takeoff.0
        && wb.max_takeoff.0 > 0.0
    {
        out.push("MZFW exceeds MTOW.".to_owned());
    }
    if wb.envelope.len() < 3 {
        out.push("The CG envelope needs at least three points.".to_owned());
    }
    if !wb.stations.is_empty() && !wb.stations.iter().any(|s| s.kind == StationKind::Fuel) {
        out.push("No fuel station — W&B cannot place the fuel mass.".to_owned());
    }

    // Distances.
    let distances = &profile.distances;
    if distances.takeoff_roll.0 <= 0.0 || distances.landing_roll.0 <= 0.0 {
        out.push(
            "Base takeoff/landing rolls are zero — runway distance checks stay silent.".to_owned(),
        );
    }
    if distances.takeoff_safety_factor < 1.0 || distances.landing_safety_factor < 1.0 {
        out.push(
            "A safety factor below 1.0 shortens the distance — that is not a margin.".to_owned(),
        );
    }

    out
}

#[cfg(test)]
mod tests {
    use strata_data::domain::Meters;
    use strata_plan::aircraft::{AircraftId, EnvelopePoint, WbStation};
    use strata_plan::units::{Kilograms, Liters};

    use crate::flight_io::aircraft::example_c172;

    use super::*;

    /// The example profile minus its example marker: a healthy profile
    /// warns about nothing.
    fn healthy() -> AircraftProfile {
        let mut profile = example_c172();
        profile.name = Some("Skyhawk".to_owned());
        profile
    }

    #[test]
    fn a_healthy_profile_has_no_warnings() {
        assert_eq!(warnings(&healthy()), Vec::<String>::new());
    }

    #[test]
    fn example_data_is_called_out() {
        let listed = warnings(&example_c172());
        assert!(
            listed.iter().any(|w| w.contains("example data")),
            "{listed:?}"
        );
    }

    #[test]
    fn an_empty_profile_warns_but_never_blocks() {
        let empty = AircraftProfile::new(AircraftId::new("x").unwrap());
        let listed = warnings(&empty);
        assert!(listed.iter().any(|w| w.contains("item 7")));
        assert!(listed.iter().any(|w| w.contains("item 9")));
        assert!(listed.iter().any(|w| w.contains("cruise power setting")));
        assert!(listed.iter().any(|w| w.contains("Usable fuel")));
        assert!(listed.iter().any(|w| w.contains("three points")));
        // Empty stations list: the missing-fuel-station warning would be
        // noise before any station exists.
        assert!(
            !listed.iter().any(|w| w.contains("fuel station")),
            "{listed:?}"
        );
    }

    #[test]
    fn odd_values_warn_specifically() {
        let mut profile = healthy();
        profile.fuel.tabs = Some(Liters(500.0));
        profile.fuel.density = strata_plan::units::KilogramsPerLiter(1.4);
        profile.weight_balance.max_landing = Some(Kilograms(2000.0));
        profile.weight_balance.envelope = vec![
            EnvelopePoint {
                arm: Meters(1.0),
                mass: Kilograms(700.0),
            },
            EnvelopePoint {
                arm: Meters(1.2),
                mass: Kilograms(700.0),
            },
        ];
        profile
            .weight_balance
            .stations
            .retain(|s| s.kind != StationKind::Fuel);
        profile.weight_balance.stations.push(WbStation {
            name: "Ballast".to_owned(),
            arm: Meters(3.0),
            kind: StationKind::Other,
            max_load: None,
        });
        profile.distances.takeoff_safety_factor = 0.9;

        let listed = warnings(&profile);
        assert!(listed.iter().any(|w| w.contains("tab level")));
        assert!(listed.iter().any(|w| w.contains("density")));
        assert!(listed.iter().any(|w| w.contains("MLW")));
        assert!(listed.iter().any(|w| w.contains("three points")));
        assert!(listed.iter().any(|w| w.contains("fuel station")));
        assert!(listed.iter().any(|w| w.contains("safety factor")));
    }
}
