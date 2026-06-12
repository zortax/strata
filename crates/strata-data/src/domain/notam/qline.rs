//! Q-line decoder. The qualifier line compresses a NOTAM into eight
//! slash-separated fields:
//!
//! ```text
//! Q) EDGG/QMRLC/IV/NBO/A/000/999/5002N00834E005
//!    FIR  code  tfc purp scope lo  up  centre+radius
//! ```
//!
//! Lower/upper are flight levels (`000` = surface, `999` = unlimited),
//! mapped to the domain [`VerticalLimit`]. The centre is `ddmmN dddmmE`
//! (degrees + minutes), the radius the trailing three digits in NM.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::domain::{IcaoCode, LatLon, VerticalLimit};

use super::NotamParseError;
use super::qcode::{QCondition, QSubject};

/// The decoded five-letter Q-code (`Q` + subject + condition).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QCode {
    /// Letters 2–3: what the NOTAM is about.
    pub subject: QSubject,
    /// Letters 4–5: its condition.
    pub condition: QCondition,
}

impl QCode {
    /// Parses a five-letter code like `QMRLC`.
    pub fn parse(code: &str) -> Result<Self, NotamParseError> {
        let letters = code
            .strip_prefix('Q')
            .filter(|rest| rest.len() == 4 && rest.bytes().all(|b| b.is_ascii_alphabetic()))
            .ok_or_else(|| NotamParseError::MalformedQLine {
                field: "Q-code",
                value: code.to_owned(),
            })?;
        Ok(Self {
            subject: QSubject::from_code(&letters[0..2]),
            condition: QCondition::from_code(&letters[2..4]),
        })
    }
}

impl fmt::Display for QCode {
    /// The transmitted form, e.g. `QMRLC`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Q{}{}", self.subject.code(), self.condition.code())
    }
}

macro_rules! letter_flags {
    (
        $(#[$meta:meta])*
        $name:ident { $($flag:ident => $letter:literal, $desc:literal;)+ }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name {
            $(#[doc = $desc] pub $flag: bool,)+
        }

        impl $name {
            /// Parses the letter set; an empty body yields no flags,
            /// letters outside the alphabet are an error.
            pub fn parse(s: &str) -> Result<Self, NotamParseError> {
                let mut value = Self::default();
                for c in s.chars() {
                    match c {
                        $($letter => value.$flag = true,)+
                        _ => {
                            return Err(NotamParseError::MalformedQLine {
                                field: stringify!($name),
                                value: s.to_owned(),
                            });
                        }
                    }
                }
                Ok(value)
            }
        }

        impl fmt::Display for $name {
            /// Canonical letter order as transmitted.
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                $(if self.$flag {
                    write!(f, "{}", $letter)?;
                })+
                Ok(())
            }
        }
    };
}

letter_flags! {
    /// Q-line traffic qualifier (`I`, `V`, `IV`, `K`).
    Traffic {
        ifr => 'I', "affects IFR traffic";
        vfr => 'V', "affects VFR traffic";
        checklist => 'K', "checklist NOTAM";
    }
}

letter_flags! {
    /// Q-line purpose qualifier (combinations of `N`, `B`, `O`, `M`, `K`).
    Purpose {
        immediate_attention => 'N', "for the immediate attention of operators";
        briefing => 'B', "of operational significance: PIB entry";
        flight_operations => 'O', "concerning flight operations";
        miscellaneous => 'M', "miscellaneous: not for briefing";
        checklist => 'K', "checklist NOTAM";
    }
}

letter_flags! {
    /// Q-line scope qualifier (combinations of `A`, `E`, `W`, `K`).
    Scope {
        aerodrome => 'A', "aerodrome";
        enroute => 'E', "en-route";
        nav_warning => 'W', "navigation warning";
        checklist => 'K', "checklist NOTAM";
    }
}

/// The decoded Q-line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QLine {
    /// FIR the NOTAM lies in (e.g. `EDGG`).
    pub fir: IcaoCode,
    /// Subject + condition.
    pub code: QCode,
    pub traffic: Traffic,
    pub purpose: Purpose,
    pub scope: Scope,
    /// Lower limit (`000` decodes to GND).
    pub lower: VerticalLimit,
    /// Upper limit (`999` decodes to UNL).
    pub upper: VerticalLimit,
    /// Centre of the affected circle.
    pub centre: LatLon,
    /// Radius around `centre` in nautical miles.
    pub radius_nm: u32,
}

impl QLine {
    /// Parses the body of item Q (without the `Q)` marker).
    pub fn parse(s: &str) -> Result<Self, NotamParseError> {
        let fields: Vec<&str> = s.trim().split('/').map(str::trim).collect();
        let [fir, code, traffic, purpose, scope, lower, upper, geo] = fields[..] else {
            return Err(NotamParseError::MalformedQLine {
                field: "field count",
                value: s.trim().to_owned(),
            });
        };
        let fir = IcaoCode::new(fir).map_err(|_| NotamParseError::MalformedQLine {
            field: "FIR",
            value: fir.to_owned(),
        })?;
        let (centre, radius_nm) = parse_centre_radius(geo)?;
        Ok(Self {
            fir,
            code: QCode::parse(code)?,
            traffic: Traffic::parse(traffic)?,
            purpose: Purpose::parse(purpose)?,
            scope: Scope::parse(scope)?,
            lower: parse_limit(lower, "lower limit")?,
            upper: parse_limit(upper, "upper limit")?,
            centre,
            radius_nm,
        })
    }
}

/// Three-digit flight-level limit: `000` = GND, `999` = UNL, else FL.
fn parse_limit(s: &str, field: &'static str) -> Result<VerticalLimit, NotamParseError> {
    let err = || NotamParseError::MalformedQLine {
        field,
        value: s.to_owned(),
    };
    if s.len() != 3 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(err());
    }
    let level: u16 = s.parse().map_err(|_| err())?;
    Ok(match level {
        0 => VerticalLimit::gnd(),
        999 => VerticalLimit::unl(),
        fl => VerticalLimit::fl(fl),
    })
}

/// `ddmmNdddmmE` + 3-digit radius, e.g. `5002N00834E005`.
fn parse_centre_radius(s: &str) -> Result<(LatLon, u32), NotamParseError> {
    let err = |field: &'static str| NotamParseError::MalformedQLine {
        field,
        value: s.to_owned(),
    };
    if s.len() != 14 || !s.is_ascii() {
        return Err(err("centre/radius"));
    }
    let (lat_part, rest) = s.split_at(5);
    let (lon_part, radius_part) = rest.split_at(6);

    let lat = parse_arc(&lat_part[..4], lat_part.as_bytes()[4], b'N', b'S', 2)
        .ok_or_else(|| err("centre latitude"))?;
    let lon = parse_arc(&lon_part[..5], lon_part.as_bytes()[5], b'E', b'W', 3)
        .ok_or_else(|| err("centre longitude"))?;
    let centre = LatLon::new(lat, lon).map_err(|_| err("centre"))?;

    if !radius_part.bytes().all(|b| b.is_ascii_digit()) {
        return Err(err("radius"));
    }
    let radius_nm: u32 = radius_part.parse().map_err(|_| err("radius"))?;
    Ok((centre, radius_nm))
}

/// Degrees+minutes digits with a hemisphere letter → signed degrees.
fn parse_arc(digits: &str, hemi: u8, pos: u8, neg: u8, deg_digits: usize) -> Option<f64> {
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let degrees: f64 = digits[..deg_digits].parse().ok()?;
    let minutes: f64 = digits[deg_digits..].parse().ok()?;
    if minutes >= 60.0 {
        return None;
    }
    let magnitude = degrees + minutes / 60.0;
    if hemi == pos {
        Some(magnitude)
    } else if hemi == neg {
        Some(-magnitude)
    } else {
        None
    }
}

/// Renders a centre + radius back to the Q-line form (nearest minute).
/// Used to reconstruct canonical NOTAM text from structured API rows.
pub(crate) fn format_centre_radius(centre: LatLon, radius_nm: u32) -> String {
    fn deg_min(value: f64) -> (u32, u32) {
        let total_minutes = (value.abs() * 60.0).round() as u32;
        (total_minutes / 60, total_minutes % 60)
    }
    let (lat_d, lat_m) = deg_min(centre.lat());
    let (lon_d, lon_m) = deg_min(centre.lon());
    format!(
        "{lat_d:02}{lat_m:02}{}{lon_d:03}{lon_m:02}{}{:03}",
        if centre.lat() >= 0.0 { 'N' } else { 'S' },
        if centre.lon() >= 0.0 { 'E' } else { 'W' },
        radius_nm.min(999),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VerticalReference;

    #[test]
    fn parses_a_typical_aerodrome_q_line() {
        let q = QLine::parse("EDGG/QMRLC/IV/NBO/A/000/999/5002N00834E005").expect("parses");
        assert_eq!(q.fir.as_str(), "EDGG");
        assert_eq!(q.code.subject, QSubject::Runway);
        assert_eq!(q.code.condition, QCondition::Closed);
        assert!(q.traffic.ifr && q.traffic.vfr && !q.traffic.checklist);
        assert!(q.purpose.immediate_attention && q.purpose.briefing && q.purpose.flight_operations);
        assert!(!q.purpose.miscellaneous);
        assert!(q.scope.aerodrome && !q.scope.enroute && !q.scope.nav_warning);
        assert_eq!(q.lower.reference, VerticalReference::Gnd);
        assert_eq!(q.upper.reference, VerticalReference::Unl);
        assert!((q.centre.lat() - (50.0 + 2.0 / 60.0)).abs() < 1e-9);
        assert!((q.centre.lon() - (8.0 + 34.0 / 60.0)).abs() < 1e-9);
        assert_eq!(q.radius_nm, 5);
    }

    #[test]
    fn parses_flight_level_limits() {
        let q = QLine::parse("EDMM/QRRCA/IV/BO/W/000/100/4942N01156E010").expect("parses");
        assert_eq!(q.lower.reference, VerticalReference::Gnd);
        assert_eq!(q.upper.reference, VerticalReference::Fl(100));
        assert_eq!(q.radius_nm, 10);
    }

    #[test]
    fn parses_southern_and_western_hemispheres() {
        let q = QLine::parse("SBBS/QWMLW/IV/M/W/000/050/2335S04638W020").expect("parses");
        assert!((q.centre.lat() - -(23.0 + 35.0 / 60.0)).abs() < 1e-9);
        assert!((q.centre.lon() - -(46.0 + 38.0 / 60.0)).abs() < 1e-9);
    }

    #[test]
    fn unknown_q_code_letters_decode_to_other() {
        let q = QLine::parse("EDGG/QFBHA/IV/BO/A/000/999/5002N00834E005").expect("parses");
        assert_eq!(q.code.subject, QSubject::Other("FB".to_owned()));
        assert_eq!(q.code.condition, QCondition::Other("HA".to_owned()));
        assert_eq!(q.code.to_string(), "QFBHA");
    }

    #[test]
    fn ifr_only_traffic() {
        let q = QLine::parse("EDGG/QICAS/I/NBO/A/000/999/5002N00834E005").expect("parses");
        assert!(q.traffic.ifr && !q.traffic.vfr);
        assert_eq!(q.traffic.to_string(), "I");
    }

    #[test]
    fn rejects_malformed_q_lines() {
        // Too few fields.
        assert!(QLine::parse("EDGG/QMRLC/IV/NBO/A/000/999").is_err());
        // Bad code (no Q prefix).
        assert!(QLine::parse("EDGG/MRLCX/IV/NBO/A/000/999/5002N00834E005").is_err());
        // Bad traffic letter.
        assert!(QLine::parse("EDGG/QMRLC/IX/NBO/A/000/999/5002N00834E005").is_err());
        // Bad scope letter.
        assert!(QLine::parse("EDGG/QMRLC/IV/NBO/Z/000/999/5002N00834E005").is_err());
        // Bad limits.
        assert!(QLine::parse("EDGG/QMRLC/IV/NBO/A/0A0/999/5002N00834E005").is_err());
        // Bad hemisphere letter.
        assert!(QLine::parse("EDGG/QMRLC/IV/NBO/A/000/999/5002X00834E005").is_err());
        // Minutes out of range.
        assert!(QLine::parse("EDGG/QMRLC/IV/NBO/A/000/999/5099N00834E005").is_err());
        // Radius not numeric.
        assert!(QLine::parse("EDGG/QMRLC/IV/NBO/A/000/999/5002N00834E0A5").is_err());
    }

    #[test]
    fn flag_displays_render_canonical_letter_order() {
        let purpose = Purpose::parse("BON").expect("parses");
        assert_eq!(purpose.to_string(), "NBO");
        let scope = Scope::parse("AE").expect("parses");
        assert_eq!(scope.to_string(), "AE");
    }

    #[test]
    fn centre_radius_formatting_round_trips() {
        let (centre, radius) = parse_centre_radius("5002N00834E005").expect("parses");
        assert_eq!(format_centre_radius(centre, radius), "5002N00834E005");
        let (centre, radius) = parse_centre_radius("2335S04638W020").expect("parses");
        assert_eq!(format_centre_radius(centre, radius), "2335S04638W020");
    }

    #[test]
    fn centre_formatting_carries_rounded_minutes() {
        // 47.9999° ≈ 48°00' — the carry must roll into degrees.
        let centre = LatLon::new(47.99999, 11.99999).expect("valid");
        assert_eq!(format_centre_radius(centre, 5), "4800N01200E005");
    }
}
