//! Pure label/formatting helpers shared by the Briefing tab's NOTAM cards
//! and the PDF [`input`](super::input) conversion — validity windows,
//! relevance chips, decoded Q-line summaries, snapshot provenance.

use chrono::{DateTime, Utc};
use strata_data::domain::{Notam, NotamEnd, NotamValidity, QLine, VerticalReference};
use strata_plan::flight::PlannedAltitude;
use strata_plan::notam_relevance::NotamRelevance;

use crate::state::briefing::NotamSource;

/// `"16 Jun 07:00Z → 18 Jun 15:00Z"`, with the EST/PERM conventions
/// spelled out (`(est)` working end, `permanent`).
pub(crate) fn validity_label(validity: &NotamValidity) -> String {
    let from = fmt_utc(validity.from);
    match validity.until {
        NotamEnd::At(to) => format!("{from} → {}", fmt_utc(to)),
        NotamEnd::Estimated(to) => format!("{from} → {} (est)", fmt_utc(to)),
        NotamEnd::Permanent => format!("{from} → permanent"),
    }
}

fn fmt_utc(t: DateTime<Utc>) -> String {
    t.format("%d %b %H:%MZ").to_string()
}

/// Why the NOTAM briefs — the relevance chip text.
pub(crate) fn relevance_label(relevance: &NotamRelevance) -> String {
    match relevance {
        NotamRelevance::Aerodrome(icao) => format!("Aerodrome {icao}"),
        NotamRelevance::RouteCorridor { distance_nm } if distance_nm.0 < 0.05 => {
            "Corridor — on track".to_owned()
        }
        NotamRelevance::RouteCorridor { distance_nm } => {
            format!("Corridor · {:.1} NM off track", distance_nm.0)
        }
        NotamRelevance::Fir => "FIR-wide".to_owned(),
    }
}

/// The decoded Q-line on one line: subject — condition, the vertical band
/// (when it says anything), the affected radius (when geometric).
/// Unknown Q-codes fall back to their raw two-letter codes — never empty.
pub(crate) fn q_summary(q: &QLine) -> String {
    let mut parts = vec![capitalized(&format!(
        "{} — {}",
        q.code.subject, q.code.condition
    ))];
    if let Some(limits) = q_limits(q) {
        parts.push(limits);
    }
    // 999 conventionally means "the whole FIR" — not a geometric radius.
    if q.radius_nm < 999 {
        parts.push(format!("{} NM radius", q.radius_nm));
    }
    parts.join(" · ")
}

/// First letter uppercased (the Q-code table is lowercase prose).
fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `"GND → FL 100"`; `None` for the say-nothing full GND → UNL band.
pub(crate) fn q_limits(q: &QLine) -> Option<String> {
    let full_band = q.lower.reference == VerticalReference::Gnd
        && q.upper.reference == VerticalReference::Unl;
    (!full_band).then(|| format!("{} → {}", q.lower, q.upper))
}

/// Item A locations as the card's location chip (`"EDDF"`, FIR-wide
/// NOTAMs list the FIR itself).
pub(crate) fn location_label(notam: &Notam) -> String {
    if notam.locations.is_empty() {
        notam.fir().to_string()
    } else {
        notam
            .locations
            .iter()
            .map(|l| l.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The snapshot-status header line. The provenance is always visible:
/// a snapshot rendered while credentials are missing is labelled a
/// stored snapshot, never claimed live.
pub(crate) fn snapshot_label(
    taken_at: Option<DateTime<Utc>>,
    fetching: bool,
    source: NotamSource,
) -> String {
    if fetching {
        return "Fetching NOTAMs…".to_owned();
    }
    match taken_at {
        Some(taken_at) => format!(
            "NOTAMs fetched {} · {}",
            taken_at.format("%H:%MZ"),
            source_label(source)
        ),
        None => "No NOTAM data yet".to_owned(),
    }
}

/// Provenance word of the configured NOTAM source. `NotConfigured` only
/// renders when the document carries a snapshot from an earlier session —
/// honest: it is whatever was stored, and cannot be refreshed.
pub(crate) fn source_label(source: NotamSource) -> &'static str {
    match source {
        NotamSource::NotConfigured => "stored snapshot",
        NotamSource::Autorouter => "autorouter.aero",
    }
}

/// Datum-carrying altitude string for the PDF (`"5500 ft AMSL"`, `"FL95"`).
pub(crate) fn fmt_planned_altitude(altitude: PlannedAltitude) -> String {
    match altitude {
        PlannedAltitude::Amsl(meters) => format!("{:.0} ft AMSL", meters.as_feet()),
        PlannedAltitude::FlightLevel(n) => format!("FL{n}"),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;
    use strata_data::domain::MetersAmsl;
    use strata_plan::units::NauticalMiles;

    use super::*;

    fn utc(d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, d, h, mi, 0).single().expect("valid")
    }

    #[test]
    fn validity_labels_cover_definite_estimated_and_permanent_ends() {
        let definite = NotamValidity {
            from: utc(16, 7, 0),
            until: NotamEnd::At(utc(18, 15, 0)),
        };
        assert_eq!(validity_label(&definite), "16 Jun 07:00Z → 18 Jun 15:00Z");

        let estimated = NotamValidity {
            from: utc(16, 7, 0),
            until: NotamEnd::Estimated(utc(18, 15, 0)),
        };
        assert_eq!(
            validity_label(&estimated),
            "16 Jun 07:00Z → 18 Jun 15:00Z (est)"
        );

        let permanent = NotamValidity {
            from: utc(16, 7, 0),
            until: NotamEnd::Permanent,
        };
        assert_eq!(validity_label(&permanent), "16 Jun 07:00Z → permanent");
    }

    #[test]
    fn relevance_chips_name_their_class() {
        let icao = strata_data::domain::IcaoCode::new("EDDF").expect("valid");
        assert_eq!(
            relevance_label(&NotamRelevance::Aerodrome(icao)),
            "Aerodrome EDDF"
        );
        assert_eq!(
            relevance_label(&NotamRelevance::RouteCorridor {
                distance_nm: NauticalMiles(0.0)
            }),
            "Corridor — on track"
        );
        assert_eq!(
            relevance_label(&NotamRelevance::RouteCorridor {
                distance_nm: NauticalMiles(2.31)
            }),
            "Corridor · 2.3 NM off track"
        );
        assert_eq!(relevance_label(&NotamRelevance::Fir), "FIR-wide");
    }

    #[test]
    fn q_summary_decodes_subject_condition_band_and_radius() {
        let q = QLine::parse("EDMM/QRRCA/IV/BO/W/000/100/4942N01156E010").expect("parses");
        assert_eq!(
            q_summary(&q),
            "Restricted area — activated · GND → FL 100 · 10 NM radius"
        );

        // Full GND→UNL band says nothing; FIR-wide radius is non-geometric.
        let q = QLine::parse("EDGG/QMRLC/IV/NBO/A/000/999/5002N00834E999").expect("parses");
        assert_eq!(q_limits(&q), None);
        assert_eq!(q_summary(&q), "Runway — closed");

        // Unknown codes fall back to the raw letters, never empty.
        let q = QLine::parse("EDGG/QZZYY/IV/BO/A/000/050/5002N00834E005").expect("parses");
        assert_eq!(q_summary(&q), "ZZ — YY · GND → FL 50 · 5 NM radius");
    }

    #[test]
    fn snapshot_line_keeps_the_provenance_visible() {
        let taken = utc(16, 9, 12);
        assert_eq!(
            snapshot_label(Some(taken), false, NotamSource::NotConfigured),
            "NOTAMs fetched 09:12Z · stored snapshot"
        );
        assert_eq!(
            snapshot_label(Some(taken), false, NotamSource::Autorouter),
            "NOTAMs fetched 09:12Z · autorouter.aero"
        );
        assert_eq!(
            snapshot_label(None, false, NotamSource::NotConfigured),
            "No NOTAM data yet"
        );
        assert_eq!(
            snapshot_label(Some(taken), true, NotamSource::NotConfigured),
            "Fetching NOTAMs…"
        );
    }

    #[test]
    fn planned_altitudes_carry_their_datum() {
        assert_eq!(
            fmt_planned_altitude(PlannedAltitude::Amsl(MetersAmsl::from_feet(5500.0))),
            "5500 ft AMSL"
        );
        assert_eq!(fmt_planned_altitude(PlannedAltitude::FlightLevel(95)), "FL95");
    }
}
