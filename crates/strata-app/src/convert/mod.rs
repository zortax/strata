//! Pure conversions from `strata-data` domain types to `strata-render`
//! value types. The app is the only place the two crates meet.

use std::hash::{Hash as _, Hasher as _};

use strata_data::domain::{
    Airport, AirportKind, Airspace, AirspaceClass, AirspaceKind, FlightCategory, LatLon, Navaid,
    NavaidKind, Obstacle, ReportingPoint, Runway, Sigmet, SigmetHazard,
};
use strata_render::{
    AirspaceStyleKey, FlightCategoryColor, IcaoClass, PointKind, RenderAirspace,
    RenderPointFeature, RenderSigmet,
};

/// Stable per-feature id for render-side label collision ordering.
///
/// Renderer label ids reserve the top bits (`1 << 61..63`), so keep ids in
/// the low 48 bits. `DefaultHasher` uses fixed keys — deterministic across
/// runs, so labels don't reshuffle between feature feeds.
fn stable_id(parts: &[&str]) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    for part in parts {
        part.hash(&mut hasher);
    }
    hasher.finish() & 0xFFFF_FFFF_FFFF
}

fn render_lat_lon(p: LatLon) -> strata_render::LatLon {
    strata_render::LatLon::new(p.lat(), p.lon())
}

fn ring(points: &[LatLon]) -> Vec<[f64; 2]> {
    points.iter().map(|p| [p.lon(), p.lat()]).collect()
}

/// Chart styling key for an airspace: special zone kinds win over the ICAO
/// class; class-defined airspace (TMA, CTA, plain areas, …) falls back to its
/// class color, matching German chart conventions.
pub fn airspace_style_key(class: AirspaceClass, kind: &AirspaceKind) -> AirspaceStyleKey {
    match kind {
        // A military CTR is still a control zone.
        AirspaceKind::Ctr | AirspaceKind::Mctr => AirspaceStyleKey::Ctr,
        AirspaceKind::Tmz => AirspaceStyleKey::Tmz,
        AirspaceKind::Rmz => AirspaceStyleKey::Rmz,
        // Alert/warning areas are charted like danger areas (ICAO Annex 4).
        AirspaceKind::Danger | AirspaceKind::AlertArea | AirspaceKind::WarningArea => {
            AirspaceStyleKey::Danger
        }
        // TRA/TSA and overflight restrictions are charted like ED-R.
        AirspaceKind::Restricted
        | AirspaceKind::Tra
        | AirspaceKind::Tsa
        | AirspaceKind::OverflightRestriction => AirspaceStyleKey::Restricted,
        AirspaceKind::Prohibited => AirspaceStyleKey::Prohibited,
        AirspaceKind::GliderSector => AirspaceStyleKey::GliderSector,
        // openAIP publishes para-jump areas as type 28 "Aerial Sporting Or
        // Recreational Activity"; the rest of that type shares the styling.
        AirspaceKind::ParachuteJumpArea | AirspaceKind::RecreationalActivity => {
            AirspaceStyleKey::ParaJump
        }
        // Everything class-defined or informational styles by ICAO class.
        AirspaceKind::Area
        | AirspaceKind::Tma
        | AirspaceKind::Cta
        | AirspaceKind::Atz
        | AirspaceKind::Matz
        | AirspaceKind::Htz
        | AirspaceKind::Tiz
        | AirspaceKind::Tia
        | AirspaceKind::Fir
        | AirspaceKind::Uir
        | AirspaceKind::FisSector
        | AirspaceKind::VfrSector
        | AirspaceKind::AccSector
        | AirspaceKind::Adiz
        | AirspaceKind::Airway
        | AirspaceKind::MilitaryTrainingRoute
        | AirspaceKind::MilitaryRoute
        | AirspaceKind::MilitaryTrainingArea
        | AirspaceKind::TsaTraFeedingRoute
        | AirspaceKind::ProtectedArea
        | AirspaceKind::TransponderSetting
        | AirspaceKind::LowerTrafficArea
        | AirspaceKind::UpperTrafficArea
        | AirspaceKind::Other(_) => match class {
            AirspaceClass::A => AirspaceStyleKey::IcaoClass(IcaoClass::A),
            AirspaceClass::B => AirspaceStyleKey::IcaoClass(IcaoClass::B),
            AirspaceClass::C => AirspaceStyleKey::IcaoClass(IcaoClass::C),
            AirspaceClass::D => AirspaceStyleKey::IcaoClass(IcaoClass::D),
            AirspaceClass::E => AirspaceStyleKey::IcaoClass(IcaoClass::E),
            AirspaceClass::F => AirspaceStyleKey::IcaoClass(IcaoClass::F),
            AirspaceClass::G => AirspaceStyleKey::IcaoClass(IcaoClass::G),
            AirspaceClass::Unclassified => AirspaceStyleKey::Other,
        },
    }
}

pub fn airspace(a: &Airspace) -> RenderAirspace {
    let lower = a.lower.to_string();
    let upper = a.upper.to_string();
    RenderAirspace {
        id: stable_id(&["airspace", &a.name, &lower, &upper]),
        style: airspace_style_key(a.class, &a.kind),
        polygon: ring(a.geometry.exterior()),
        holes: a.geometry.holes().iter().map(|h| ring(h)).collect(),
        lower_label: lower,
        upper_label: upper,
        name: a.name.clone(),
    }
}

/// Symbol kind for an airport; `None` = not drawn (closed fields).
pub fn airport_point_kind(kind: AirportKind) -> Option<PointKind> {
    match kind {
        AirportKind::International => Some(PointKind::AirportIntl),
        AirportKind::Regional | AirportKind::MilitaryAerodrome => Some(PointKind::AirportRegional),
        AirportKind::Airfield
        | AirportKind::WaterAirfield
        | AirportKind::LandingStrip
        | AirportKind::Other(_) => Some(PointKind::Airfield),
        AirportKind::GliderSite => Some(PointKind::GliderSite),
        AirportKind::Heliport => Some(PointKind::Heliport),
        AirportKind::UltraLightSite => Some(PointKind::UltraLight),
        AirportKind::Closed => None,
    }
}

/// Orientation of the airport's primary runway for the symbol's runway tick:
/// true heading in degrees, normalized to `[0, 180)` — a runway is a
/// bidirectional line, so reciprocal designators (07/25) give the same
/// orientation. Primary = the longest runway carrying heading data, falling
/// back to the first with a heading; `None` when no runway has one.
pub fn primary_runway_heading_deg(runways: &[Runway]) -> Option<f32> {
    let mut primary: Option<(&Runway, u16)> = None;
    for runway in runways {
        let Some(heading) = runway.true_heading_deg else {
            continue;
        };
        let longer = match primary {
            None => true,
            Some((best, _)) => match (runway.length, best.length) {
                // Strictly longer wins; ties keep the earlier runway.
                (Some(len), Some(best_len)) => len.0 > best_len.0,
                (Some(_), None) => true,
                // Without a length this runway never displaces an earlier pick.
                (None, _) => false,
            },
        };
        if longer {
            primary = Some((runway, heading));
        }
    }
    primary.map(|(_, heading)| f32::from(heading % 180))
}

pub fn airport(a: &Airport) -> Option<RenderPointFeature> {
    let kind = airport_point_kind(a.kind)?;
    let label = a.ident.as_ref().map(|i| i.as_str().to_owned());
    Some(RenderPointFeature {
        id: stable_id(&["airport", label.as_deref().unwrap_or(&a.name)]),
        kind,
        position: render_lat_lon(a.position),
        label,
        rotation_deg: primary_runway_heading_deg(&a.runways),
    })
}

pub fn navaid_point_kind(kind: NavaidKind) -> PointKind {
    match kind {
        NavaidKind::Dme => PointKind::Dme,
        NavaidKind::Tacan => PointKind::Tacan,
        NavaidKind::Ndb => PointKind::Ndb,
        NavaidKind::Vor | NavaidKind::Dvor => PointKind::Vor,
        // VORTACs read as VOR-DME for VFR purposes; no dedicated symbol.
        NavaidKind::VorDme | NavaidKind::Vortac | NavaidKind::DvorDme | NavaidKind::Dvortac => {
            PointKind::VorDme
        }
    }
}

pub fn navaid(n: &Navaid) -> RenderPointFeature {
    RenderPointFeature {
        id: stable_id(&["navaid", &n.ident, &n.name]),
        kind: navaid_point_kind(n.kind),
        position: render_lat_lon(n.position),
        label: Some(n.ident.clone()),
        rotation_deg: None,
    }
}

pub fn reporting_point(p: &ReportingPoint) -> RenderPointFeature {
    RenderPointFeature {
        id: stable_id(&["reporting-point", &p.name]),
        kind: if p.mandatory {
            PointKind::ReportingPointMandatory
        } else {
            PointKind::ReportingPointVoluntary
        },
        position: render_lat_lon(p.position),
        label: Some(p.name.clone()),
        rotation_deg: None,
    }
}

pub fn obstacle(o: &Obstacle) -> RenderPointFeature {
    RenderPointFeature {
        id: stable_id(&[
            "obstacle",
            o.name.as_deref().unwrap_or(""),
            &format!("{:.5},{:.5}", o.position.lat(), o.position.lon()),
        ]),
        kind: PointKind::Obstacle,
        position: render_lat_lon(o.position),
        label: None,
        rotation_deg: None,
    }
}

pub fn flight_category_color(category: FlightCategory) -> FlightCategoryColor {
    match category {
        FlightCategory::Vfr => FlightCategoryColor::Vfr,
        FlightCategory::Mvfr => FlightCategoryColor::Mvfr,
        FlightCategory::Ifr => FlightCategoryColor::Ifr,
        FlightCategory::Lifr => FlightCategoryColor::Lifr,
    }
}

/// METAR station dot at the reporting airport's position.
pub fn weather_station(
    station: &str,
    position: LatLon,
    category: FlightCategory,
) -> RenderPointFeature {
    RenderPointFeature {
        id: stable_id(&["wx", station]),
        kind: PointKind::WeatherStation(flight_category_color(category)),
        position: render_lat_lon(position),
        label: None,
        rotation_deg: None,
    }
}

/// Short AWC-style hazard code for map labels.
pub fn sigmet_hazard_code(hazard: &SigmetHazard) -> String {
    match hazard {
        SigmetHazard::Thunderstorm => "TS".into(),
        SigmetHazard::Turbulence => "TURB".into(),
        SigmetHazard::Icing => "ICE".into(),
        SigmetHazard::MountainWave => "MTW".into(),
        SigmetHazard::VolcanicAsh => "VA".into(),
        SigmetHazard::TropicalCyclone => "TC".into(),
        SigmetHazard::DustStorm => "DS".into(),
        SigmetHazard::Sandstorm => "SS".into(),
        SigmetHazard::RadioactiveCloud => "RDOACT".into(),
        SigmetHazard::Other(code) => code.clone(),
    }
}

pub fn sigmet(s: &Sigmet) -> RenderSigmet {
    RenderSigmet {
        polygon: ring(s.geometry.exterior()),
        hazard_label: format!("SIGMET {}", sigmet_hazard_code(&s.hazard)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_data::domain::{
        Meters, MetersAgl, MetersAmsl, Polygon, RunwaySurface, VerticalLimit,
    };

    fn p(lat: f64, lon: f64) -> LatLon {
        LatLon::new(lat, lon).expect("valid test coordinate")
    }

    #[test]
    fn special_kinds_win_over_class() {
        let cases = [
            (AirspaceKind::Ctr, AirspaceStyleKey::Ctr),
            (AirspaceKind::Mctr, AirspaceStyleKey::Ctr),
            (AirspaceKind::Tmz, AirspaceStyleKey::Tmz),
            (AirspaceKind::Rmz, AirspaceStyleKey::Rmz),
            (AirspaceKind::Danger, AirspaceStyleKey::Danger),
            (AirspaceKind::AlertArea, AirspaceStyleKey::Danger),
            (AirspaceKind::WarningArea, AirspaceStyleKey::Danger),
            (AirspaceKind::Restricted, AirspaceStyleKey::Restricted),
            (AirspaceKind::Tra, AirspaceStyleKey::Restricted),
            (AirspaceKind::Tsa, AirspaceStyleKey::Restricted),
            (
                AirspaceKind::OverflightRestriction,
                AirspaceStyleKey::Restricted,
            ),
            (AirspaceKind::Prohibited, AirspaceStyleKey::Prohibited),
            (AirspaceKind::GliderSector, AirspaceStyleKey::GliderSector),
            (AirspaceKind::ParachuteJumpArea, AirspaceStyleKey::ParaJump),
            (
                AirspaceKind::RecreationalActivity,
                AirspaceStyleKey::ParaJump,
            ),
        ];
        for (kind, expected) in cases {
            // Class D must not override the zone styling.
            assert_eq!(airspace_style_key(AirspaceClass::D, &kind), expected);
        }
    }

    #[test]
    fn plain_airspace_styles_by_icao_class() {
        let classes = [
            (AirspaceClass::A, IcaoClass::A),
            (AirspaceClass::B, IcaoClass::B),
            (AirspaceClass::C, IcaoClass::C),
            (AirspaceClass::D, IcaoClass::D),
            (AirspaceClass::E, IcaoClass::E),
            (AirspaceClass::F, IcaoClass::F),
            (AirspaceClass::G, IcaoClass::G),
        ];
        for (class, expected) in classes {
            assert_eq!(
                airspace_style_key(class, &AirspaceKind::Area),
                AirspaceStyleKey::IcaoClass(expected)
            );
        }
        assert_eq!(
            airspace_style_key(AirspaceClass::Unclassified, &AirspaceKind::Fir),
            AirspaceStyleKey::Other
        );
    }

    #[test]
    fn class_defined_kinds_style_like_their_class() {
        // TMA/CTA carry their controlling class color, German chart style.
        for kind in [AirspaceKind::Tma, AirspaceKind::Cta] {
            assert_eq!(
                airspace_style_key(AirspaceClass::C, &kind),
                AirspaceStyleKey::IcaoClass(IcaoClass::C)
            );
            assert_eq!(
                airspace_style_key(AirspaceClass::D, &kind),
                AirspaceStyleKey::IcaoClass(IcaoClass::D)
            );
        }
        // Unknown codes fall back to class, then to neutral.
        assert_eq!(
            airspace_style_key(AirspaceClass::E, &AirspaceKind::Other(99)),
            AirspaceStyleKey::IcaoClass(IcaoClass::E)
        );
        assert_eq!(
            airspace_style_key(AirspaceClass::Unclassified, &AirspaceKind::Other(99)),
            AirspaceStyleKey::Other
        );
        // Unclassified informational sectors stay neutral.
        assert_eq!(
            airspace_style_key(AirspaceClass::Unclassified, &AirspaceKind::FisSector),
            AirspaceStyleKey::Other
        );
        assert_eq!(
            airspace_style_key(AirspaceClass::Unclassified, &AirspaceKind::Atz),
            AirspaceStyleKey::Other
        );
    }

    #[test]
    fn airport_kinds_map_to_symbols() {
        assert_eq!(
            airport_point_kind(AirportKind::International),
            Some(PointKind::AirportIntl)
        );
        assert_eq!(
            airport_point_kind(AirportKind::Regional),
            Some(PointKind::AirportRegional)
        );
        assert_eq!(
            airport_point_kind(AirportKind::MilitaryAerodrome),
            Some(PointKind::AirportRegional)
        );
        assert_eq!(
            airport_point_kind(AirportKind::Airfield),
            Some(PointKind::Airfield)
        );
        assert_eq!(
            airport_point_kind(AirportKind::GliderSite),
            Some(PointKind::GliderSite)
        );
        assert_eq!(
            airport_point_kind(AirportKind::Heliport),
            Some(PointKind::Heliport)
        );
        assert_eq!(
            airport_point_kind(AirportKind::UltraLightSite),
            Some(PointKind::UltraLight)
        );
        assert_eq!(airport_point_kind(AirportKind::Closed), None);
    }

    fn runway(designator: &str, heading: Option<u16>, length_m: Option<f64>) -> Runway {
        Runway {
            designator: designator.into(),
            true_heading_deg: heading,
            length: length_m.map(Meters),
            width: None,
            surface: RunwaySurface::Asphalt,
            main: false,
        }
    }

    #[test]
    fn longest_runway_with_heading_wins() {
        let runways = [
            runway("07", Some(70), Some(1200.0)),
            runway("18", Some(180), Some(2800.0)), // longest → primary
            runway("13", Some(130), Some(900.0)),
        ];
        assert_eq!(primary_runway_heading_deg(&runways), Some(0.0)); // 180 → 0
        // A longer runway *without* a heading cannot become primary.
        let runways = [
            runway("07", Some(70), Some(1200.0)),
            runway("18", None, Some(4000.0)),
        ];
        assert_eq!(primary_runway_heading_deg(&runways), Some(70.0));
    }

    #[test]
    fn runways_without_length_fall_back_to_first_with_heading() {
        let runways = [
            runway("36", None, None),
            runway("09", Some(92), None), // first with a heading
            runway("14", Some(140), None),
        ];
        assert_eq!(primary_runway_heading_deg(&runways), Some(92.0));
    }

    #[test]
    fn missing_headings_give_no_rotation() {
        assert_eq!(primary_runway_heading_deg(&[]), None);
        let runways = [runway("07", None, Some(1500.0)), runway("25", None, None)];
        assert_eq!(primary_runway_heading_deg(&runways), None);
    }

    #[test]
    fn heading_normalizes_to_half_turn_and_reciprocals_agree() {
        // A runway is a bidirectional line: reciprocal designators give the
        // same orientation, always in [0, 180).
        for (a, b) in [(70u16, 250u16), (130, 310), (0, 180), (179, 359)] {
            let ha = primary_runway_heading_deg(&[runway("A", Some(a), None)]);
            let hb = primary_runway_heading_deg(&[runway("B", Some(b), None)]);
            assert_eq!(ha, hb, "{a}/{b} are the same runway line");
            let h = ha.expect("heading present");
            assert!((0.0..180.0).contains(&h), "{a}: {h} outside [0, 180)");
        }
        assert_eq!(
            primary_runway_heading_deg(&[runway("36", Some(360), None)]),
            Some(0.0)
        );
    }

    #[test]
    fn airport_rotation_comes_from_primary_runway() {
        let mut a = Airport {
            ident: None,
            name: "Testfeld".into(),
            kind: AirportKind::Airfield,
            position: p(50.0, 8.0),
            elevation: MetersAmsl(110.0),
            runways: vec![
                runway("07", Some(67), Some(4000.0)),
                runway("18", Some(180), Some(1000.0)),
            ],
            frequencies: vec![],
        };
        assert_eq!(
            airport(&a).expect("drawn").rotation_deg,
            Some(67.0),
            "longest runway's heading rotates the symbol"
        );
        a.runways.clear();
        assert_eq!(
            airport(&a).expect("drawn").rotation_deg,
            None,
            "no runway data → un-rotated symbol"
        );
    }

    #[test]
    fn navaid_kinds_map_to_symbols() {
        assert_eq!(navaid_point_kind(NavaidKind::Vor), PointKind::Vor);
        assert_eq!(navaid_point_kind(NavaidKind::Dvor), PointKind::Vor);
        assert_eq!(navaid_point_kind(NavaidKind::VorDme), PointKind::VorDme);
        assert_eq!(navaid_point_kind(NavaidKind::DvorDme), PointKind::VorDme);
        assert_eq!(navaid_point_kind(NavaidKind::Vortac), PointKind::VorDme);
        assert_eq!(navaid_point_kind(NavaidKind::Dvortac), PointKind::VorDme);
        assert_eq!(navaid_point_kind(NavaidKind::Dme), PointKind::Dme);
        assert_eq!(navaid_point_kind(NavaidKind::Tacan), PointKind::Tacan);
        assert_eq!(navaid_point_kind(NavaidKind::Ndb), PointKind::Ndb);
    }

    #[test]
    fn flight_categories_map_one_to_one() {
        assert_eq!(
            flight_category_color(FlightCategory::Vfr),
            FlightCategoryColor::Vfr
        );
        assert_eq!(
            flight_category_color(FlightCategory::Mvfr),
            FlightCategoryColor::Mvfr
        );
        assert_eq!(
            flight_category_color(FlightCategory::Ifr),
            FlightCategoryColor::Ifr
        );
        assert_eq!(
            flight_category_color(FlightCategory::Lifr),
            FlightCategoryColor::Lifr
        );
    }

    #[test]
    fn airspace_geometry_converts_to_lon_lat_rings() {
        let exterior = vec![p(50.0, 8.0), p(50.0, 9.0), p(51.0, 9.0), p(51.0, 8.0)];
        let geometry = Polygon::new(exterior, vec![]).expect("valid polygon");
        let a = Airspace {
            name: "TMA TEST".into(),
            class: AirspaceClass::C,
            kind: AirspaceKind::Tma,
            lower: VerticalLimit::amsl(MetersAmsl::from_feet(2500.0)),
            upper: VerticalLimit::fl(100),
            geometry,
            airac: None,
        };
        let r = airspace(&a);
        assert_eq!(r.polygon[0], [8.0, 50.0], "ring must be [lon, lat]");
        assert_eq!(r.style, AirspaceStyleKey::IcaoClass(IcaoClass::C));
        assert_eq!(r.upper_label, "FL 100");
        assert!(r.lower_label.contains("MSL"), "got {}", r.lower_label);
        assert!(r.id < (1 << 48), "id must stay clear of label flag bits");
    }

    #[test]
    fn reporting_point_mandatory_flag_selects_kind() {
        let rp = ReportingPoint {
            name: "ECHO".into(),
            mandatory: true,
            position: p(50.0, 8.0),
            airports: vec![],
        };
        assert_eq!(
            reporting_point(&rp).kind,
            PointKind::ReportingPointMandatory
        );
        let rp = ReportingPoint {
            mandatory: false,
            ..rp
        };
        assert_eq!(
            reporting_point(&rp).kind,
            PointKind::ReportingPointVoluntary
        );
    }

    #[test]
    fn obstacle_converts_with_stable_id() {
        let o = Obstacle {
            name: Some("Windpark".into()),
            kind: strata_data::domain::ObstacleKind::WindTurbine,
            position: p(52.5, 13.4),
            height: MetersAgl::from_feet(650.0),
            elevation_top: MetersAmsl::from_feet(900.0),
            lighted: true,
        };
        let a = obstacle(&o);
        let b = obstacle(&o);
        assert_eq!(a.id, b.id);
        assert_eq!(a.kind, PointKind::Obstacle);
    }

    #[test]
    fn sigmet_hazard_codes() {
        assert_eq!(sigmet_hazard_code(&SigmetHazard::Thunderstorm), "TS");
        assert_eq!(sigmet_hazard_code(&SigmetHazard::Turbulence), "TURB");
        assert_eq!(sigmet_hazard_code(&SigmetHazard::Other("XX".into())), "XX");
    }
}
