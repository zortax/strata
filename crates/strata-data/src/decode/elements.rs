//! Token-level parsers for the groups shared between METAR bodies and TAF
//! forecast groups (wind, visibility, present weather, clouds) plus the
//! METAR-only groups (temperature/dew point, QNH).
//!
//! Every parser returns `None` when the token is not that kind of group —
//! never an error — so callers try parsers in order and route leftovers to
//! `unparsed_tokens`. "Recognized but unavailable" AUTO-station groups
//! (`/////KT`, `////`, `Q////`) are distinct from `None`: the token is
//! consumed without producing a value.

use crate::domain::{
    CloudAmount, CloudKind, CloudLayer, Qnh, Visibility, Wind, WindDirection, WxDescriptor,
    WxIntensity, WxKind, WxPhenomenon,
};

const KT_PER_MPS: f64 = 1.943_844;
const KT_PER_KMH: f64 = 0.539_957;
const METERS_PER_STATUTE_MILE: f64 = 1_609.344;

/// All-slash tokens (`//`, `///`, `////`, `//////`) are unavailable-data
/// placeholders from AUTO stations (weather, colour state, visibility,
/// cloud); they are consumed without setting any field.
pub(crate) fn is_slash_only(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|b| b == b'/')
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum WindGroup {
    Wind(Wind),
    Unavailable,
}

/// `dddffKT`, `dddffGffKT`, `VRBffKT`, MPS/KMH units (converted to knots),
/// `/////KT` unavailable.
pub(crate) fn parse_wind(token: &str) -> Option<WindGroup> {
    if !token.is_ascii() {
        return None;
    }
    let body = token
        .strip_suffix("KT")
        .or_else(|| token.strip_suffix("MPS"))
        .or_else(|| token.strip_suffix("KMH"))?;
    let unit = &token[body.len()..];
    if body.len() < 5 {
        return None;
    }
    if body.bytes().all(|b| b == b'/') {
        return Some(WindGroup::Unavailable);
    }

    let (dir_part, rest) = body.split_at(3);
    let direction = if dir_part == "VRB" {
        Some(WindDirection::Variable)
    } else if dir_part == "///" {
        None
    } else if dir_part.bytes().all(|b| b.is_ascii_digit()) {
        let degrees: u16 = dir_part.parse().ok()?;
        if degrees > 360 {
            return None;
        }
        Some(WindDirection::Degrees(degrees))
    } else {
        return None;
    };

    let (speed_part, gust_part) = match rest.split_once('G') {
        Some((speed, gust)) => (speed, Some(gust)),
        None => (rest, None),
    };
    if speed_part.bytes().all(|b| b == b'/') {
        return Some(WindGroup::Unavailable);
    }
    if !(2..=3).contains(&speed_part.len()) || !speed_part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let speed: u16 = speed_part.parse().ok()?;
    let gust: Option<u16> = match gust_part {
        Some(g) if (2..=3).contains(&g.len()) && g.bytes().all(|b| b.is_ascii_digit()) => {
            Some(g.parse().ok()?)
        }
        Some(_) => return None,
        None => None,
    };

    // Direction unavailable (`///ffKT`) leaves no representable wind value.
    let Some(direction) = direction else {
        return Some(WindGroup::Unavailable);
    };
    let to_kt = |v: u16| -> u16 {
        match unit {
            "MPS" => (f64::from(v) * KT_PER_MPS).round() as u16,
            "KMH" => (f64::from(v) * KT_PER_KMH).round() as u16,
            _ => v,
        }
    };
    Some(WindGroup::Wind(Wind {
        direction,
        speed_kt: to_kt(speed),
        gust_kt: gust.map(to_kt),
        variable_range: None,
    }))
}

/// Variable wind direction range, e.g. `180V240`.
pub(crate) fn parse_variable_range(token: &str) -> Option<(u16, u16)> {
    if token.len() != 7 || !token.is_ascii() {
        return None;
    }
    let (from, rest) = token.split_at(3);
    let to = rest.strip_prefix('V')?;
    if !from.bytes().all(|b| b.is_ascii_digit()) || !to.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let from: u16 = from.parse().ok()?;
    let to: u16 = to.parse().ok()?;
    if from > 360 || to > 360 {
        return None;
    }
    Some((from, to))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum VisGroup {
    Prevailing(Visibility),
    /// Directional minimum visibility (`4000NE`); only used when no
    /// prevailing visibility was reported.
    Directional(u32),
    Unavailable,
}

/// `CAVOK`, 4-digit meters (`9999` = 10 km or more), optional `NDV` or
/// compass-direction suffix, `////` unavailable.
pub(crate) fn parse_visibility(token: &str) -> Option<VisGroup> {
    if token == "CAVOK" {
        return Some(VisGroup::Prevailing(Visibility::Cavok));
    }
    if token == "////" {
        return Some(VisGroup::Unavailable);
    }
    if !token.is_ascii() {
        return None;
    }
    let digit_len = token.bytes().take_while(u8::is_ascii_digit).count();
    if digit_len != 4 {
        return None;
    }
    let (digits, suffix) = token.split_at(digit_len);
    let meters: u32 = digits.parse().ok()?;
    match suffix {
        "" | "NDV" => Some(VisGroup::Prevailing(Visibility::Meters(meters))),
        "N" | "S" | "E" | "W" | "NE" | "NW" | "SE" | "SW" => Some(VisGroup::Directional(meters)),
        _ => None,
    }
}

/// US statute-mile visibility: `6SM`, `P6SM`, `1/2SM`, `M1/4SM`. Returns
/// statute miles; the two-token form (`2 1/2SM`) is combined by the caller.
pub(crate) fn parse_statute_miles(token: &str) -> Option<f64> {
    let body = token.strip_suffix("SM")?;
    let body = body.strip_prefix(['M', 'P']).unwrap_or(body);
    if body.is_empty() || !body.is_ascii() {
        return None;
    }
    if let Some((num, den)) = body.split_once('/') {
        if num.is_empty()
            || den.is_empty()
            || !num.bytes().all(|b| b.is_ascii_digit())
            || !den.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let num: f64 = num.parse().ok()?;
        let den: f64 = den.parse().ok()?;
        if den == 0.0 {
            return None;
        }
        Some(num / den)
    } else {
        if !body.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        body.parse().ok()
    }
}

pub(crate) fn statute_miles_to_meters(sm: f64) -> u32 {
    (sm * METERS_PER_STATUTE_MILE).round() as u32
}

const DESCRIPTORS: [(&str, WxDescriptor); 8] = [
    ("MI", WxDescriptor::Shallow),
    ("BC", WxDescriptor::Patches),
    ("PR", WxDescriptor::Partial),
    ("DR", WxDescriptor::LowDrifting),
    ("BL", WxDescriptor::Blowing),
    ("SH", WxDescriptor::Showers),
    ("TS", WxDescriptor::Thunderstorm),
    ("FZ", WxDescriptor::Freezing),
];

const KINDS: [(&str, WxKind); 21] = [
    ("DZ", WxKind::Drizzle),
    ("RA", WxKind::Rain),
    ("SN", WxKind::Snow),
    ("SG", WxKind::SnowGrains),
    ("IC", WxKind::IceCrystals),
    ("PL", WxKind::IcePellets),
    ("GR", WxKind::Hail),
    ("GS", WxKind::SmallHail),
    ("UP", WxKind::UnknownPrecipitation),
    ("BR", WxKind::Mist),
    ("FG", WxKind::Fog),
    ("FU", WxKind::Smoke),
    ("VA", WxKind::VolcanicAsh),
    ("DU", WxKind::WidespreadDust),
    ("SA", WxKind::Sand),
    ("HZ", WxKind::Haze),
    ("PO", WxKind::DustWhirls),
    ("SQ", WxKind::Squalls),
    ("FC", WxKind::FunnelCloud),
    ("SS", WxKind::Sandstorm),
    ("DS", WxKind::DustStorm),
];

/// A present-weather group: optional intensity (`-`/`+`/`VC`), optional
/// descriptor, then zero or more phenomenon codes. A group with several
/// precipitation codes (`-RASN`) yields one phenomenon per code; a bare
/// descriptor (`TS`, `VCSH`) yields a descriptor-only phenomenon.
pub(crate) fn parse_wx(token: &str) -> Option<Vec<WxPhenomenon>> {
    if !token.is_ascii() {
        return None;
    }
    let (intensity, rest) = if let Some(rest) = token.strip_prefix('-') {
        (WxIntensity::Light, rest)
    } else if let Some(rest) = token.strip_prefix('+') {
        (WxIntensity::Heavy, rest)
    } else if let Some(rest) = token.strip_prefix("VC") {
        (WxIntensity::Vicinity, rest)
    } else {
        (WxIntensity::Moderate, token)
    };
    if rest.is_empty() || rest.len() % 2 != 0 {
        return None;
    }

    let mut codes = rest;
    let mut descriptor = None;
    if let Some(&(_, d)) = DESCRIPTORS.iter().find(|(c, _)| *c == &codes[..2]) {
        descriptor = Some(d);
        codes = &codes[2..];
    }
    let mut kinds = Vec::new();
    while !codes.is_empty() {
        let (_, kind) = KINDS.iter().find(|(c, _)| *c == &codes[..2])?;
        kinds.push(*kind);
        codes = &codes[2..];
    }

    if kinds.is_empty() {
        // Descriptor-only group (`TS`, `VCSH`); a bare intensity is not weather.
        descriptor?;
        return Some(vec![WxPhenomenon {
            intensity,
            descriptor,
            kind: None,
        }]);
    }
    Some(
        kinds
            .into_iter()
            .map(|kind| WxPhenomenon {
                intensity,
                descriptor,
                kind: Some(kind),
            })
            .collect(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CloudGroup {
    Layer(CloudLayer),
    /// `VVnnn` vertical visibility in feet; `VV///` reports `None`.
    VerticalVisibility(Option<u32>),
}

/// `FEW|SCT|BKN|OVC` + base (3 digits in hundreds of feet, or `///`) +
/// optional `CB`/`TCU`/`///` type, plus the no-cloud codes.
pub(crate) fn parse_cloud(token: &str) -> Option<CloudGroup> {
    if !token.is_ascii() {
        return None;
    }
    let no_cloud = |amount| {
        Some(CloudGroup::Layer(CloudLayer {
            amount,
            base_ft_agl: None,
            kind: None,
        }))
    };
    match token {
        "NCD" => return no_cloud(CloudAmount::NoCloudDetected),
        "NSC" => return no_cloud(CloudAmount::NoSignificantCloud),
        // US codes without dedicated domain variants: CLR is the automated
        // no-cloud report, SKC the manual "sky clear".
        "CLR" => return no_cloud(CloudAmount::NoCloudDetected),
        "SKC" => return no_cloud(CloudAmount::NoSignificantCloud),
        _ => {}
    }
    if let Some(rest) = token.strip_prefix("VV") {
        if rest == "///" {
            return Some(CloudGroup::VerticalVisibility(None));
        }
        if rest.len() == 3 && rest.bytes().all(|b| b.is_ascii_digit()) {
            let hundreds: u32 = rest.parse().ok()?;
            return Some(CloudGroup::VerticalVisibility(Some(hundreds * 100)));
        }
        return None;
    }

    if token.len() < 6 {
        return None;
    }
    let (amount_part, rest) = token.split_at(3);
    let amount = match amount_part {
        "FEW" => CloudAmount::Few,
        "SCT" => CloudAmount::Scattered,
        "BKN" => CloudAmount::Broken,
        "OVC" => CloudAmount::Overcast,
        _ => return None,
    };
    let (base_part, kind_part) = rest.split_at(3);
    let base_ft_agl = if base_part == "///" {
        None
    } else if base_part.bytes().all(|b| b.is_ascii_digit()) {
        let hundreds: u32 = base_part.parse().ok()?;
        Some(hundreds * 100)
    } else {
        return None;
    };
    let kind = match kind_part {
        "" | "///" => None,
        "CB" => Some(CloudKind::Cumulonimbus),
        "TCU" => Some(CloudKind::ToweringCumulus),
        _ => return None,
    };
    Some(CloudGroup::Layer(CloudLayer {
        amount,
        base_ft_agl,
        kind,
    }))
}

/// `tt/dd` with `M` for negative values; either side may be missing or
/// slashed out (`10/`, `M05/M07`, `///08`). Both sides missing is not a
/// temperature group.
pub(crate) fn parse_temp_dew(token: &str) -> Option<(Option<i16>, Option<i16>)> {
    if !token.is_ascii() {
        return None;
    }
    let (temp_part, dew_part) = token.split_once('/')?;
    let temperature = parse_temp_side(temp_part)?;
    let dewpoint = parse_temp_side(dew_part)?;
    if temperature.is_none() && dewpoint.is_none() {
        return None;
    }
    Some((temperature, dewpoint))
}

/// `Some(None)` = side present but unavailable; outer `None` = not a
/// temperature side at all.
fn parse_temp_side(side: &str) -> Option<Option<i16>> {
    if side.is_empty() || side.bytes().all(|b| b == b'/') {
        return Some(None);
    }
    let (negative, digits) = match side.strip_prefix('M') {
        Some(rest) => (true, rest),
        None => (false, side),
    };
    if !(1..=2).contains(&digits.len()) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value: i16 = digits.parse().ok()?;
    Some(Some(if negative { -value } else { value }))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum QnhGroup {
    Value(Qnh),
    Unavailable,
}

/// `Qhhhh` (hectopascal) or `Aiiii` (hundredths of inHg); `Q////`/`A////`
/// unavailable.
pub(crate) fn parse_qnh(token: &str) -> Option<QnhGroup> {
    if !token.is_ascii() || token.len() != 5 {
        return None;
    }
    if let Some(rest) = token.strip_prefix('Q') {
        if rest == "////" {
            return Some(QnhGroup::Unavailable);
        }
        if rest.bytes().all(|b| b.is_ascii_digit()) {
            return Some(QnhGroup::Value(Qnh::Hpa(rest.parse().ok()?)));
        }
        return None;
    }
    if let Some(rest) = token.strip_prefix('A') {
        if rest == "////" {
            return Some(QnhGroup::Unavailable);
        }
        if rest.bytes().all(|b| b.is_ascii_digit()) {
            let hundredths: u32 = rest.parse().ok()?;
            return Some(QnhGroup::Value(Qnh::InHg(hundredths as f32 / 100.0)));
        }
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wind_simple() {
        assert_eq!(
            parse_wind("21008KT"),
            Some(WindGroup::Wind(Wind {
                direction: WindDirection::Degrees(210),
                speed_kt: 8,
                gust_kt: None,
                variable_range: None,
            }))
        );
    }

    #[test]
    fn wind_gusts() {
        assert_eq!(
            parse_wind("22010G25KT"),
            Some(WindGroup::Wind(Wind {
                direction: WindDirection::Degrees(220),
                speed_kt: 10,
                gust_kt: Some(25),
                variable_range: None,
            }))
        );
    }

    #[test]
    fn wind_variable_direction() {
        assert_eq!(
            parse_wind("VRB02KT"),
            Some(WindGroup::Wind(Wind {
                direction: WindDirection::Variable,
                speed_kt: 2,
                gust_kt: None,
                variable_range: None,
            }))
        );
    }

    #[test]
    fn wind_mps_converts_to_knots() {
        let WindGroup::Wind(wind) = parse_wind("14004MPS").expect("wind") else {
            panic!("expected available wind");
        };
        assert_eq!(wind.speed_kt, 8); // 4 m/s = 7.78 kt
    }

    #[test]
    fn wind_unavailable_and_rejects() {
        assert_eq!(parse_wind("/////KT"), Some(WindGroup::Unavailable));
        assert_eq!(parse_wind("///05KT"), Some(WindGroup::Unavailable));
        assert_eq!(parse_wind("21008"), None);
        assert_eq!(parse_wind("99908KT"), None); // direction > 360
        assert_eq!(parse_wind("CAVOK"), None);
    }

    #[test]
    fn variable_range() {
        assert_eq!(parse_variable_range("180V240"), Some((180, 240)));
        assert_eq!(parse_variable_range("180V400"), None);
        assert_eq!(parse_variable_range("1800240"), None);
    }

    #[test]
    fn visibility_forms() {
        assert_eq!(
            parse_visibility("CAVOK"),
            Some(VisGroup::Prevailing(Visibility::Cavok))
        );
        assert_eq!(
            parse_visibility("9999"),
            Some(VisGroup::Prevailing(Visibility::Meters(9999)))
        );
        assert_eq!(
            parse_visibility("0500"),
            Some(VisGroup::Prevailing(Visibility::Meters(500)))
        );
        assert_eq!(
            parse_visibility("9999NDV"),
            Some(VisGroup::Prevailing(Visibility::Meters(9999)))
        );
        assert_eq!(parse_visibility("4000NE"), Some(VisGroup::Directional(4000)));
        assert_eq!(parse_visibility("////"), Some(VisGroup::Unavailable));
        assert_eq!(parse_visibility("10/09"), None);
        assert_eq!(parse_visibility("092320Z"), None);
    }

    #[test]
    fn statute_miles() {
        assert_eq!(parse_statute_miles("6SM"), Some(6.0));
        assert_eq!(parse_statute_miles("P6SM"), Some(6.0));
        assert_eq!(parse_statute_miles("1/2SM"), Some(0.5));
        assert_eq!(parse_statute_miles("M1/4SM"), Some(0.25));
        assert_eq!(parse_statute_miles("9999"), None);
        assert_eq!(statute_miles_to_meters(1.0), 1609);
    }

    #[test]
    fn weather_multi_precipitation_shares_prefix() {
        let phenomena = parse_wx("-RASN").expect("weather");
        assert_eq!(phenomena.len(), 2);
        assert!(
            phenomena
                .iter()
                .all(|p| p.intensity == WxIntensity::Light && p.descriptor.is_none())
        );
        assert_eq!(phenomena[0].kind, Some(WxKind::Rain));
        assert_eq!(phenomena[1].kind, Some(WxKind::Snow));
    }

    #[test]
    fn weather_descriptor_forms() {
        let tsra = parse_wx("+TSRA").expect("weather");
        assert_eq!(tsra.len(), 1);
        assert_eq!(tsra[0].intensity, WxIntensity::Heavy);
        assert_eq!(tsra[0].descriptor, Some(WxDescriptor::Thunderstorm));
        assert_eq!(tsra[0].kind, Some(WxKind::Rain));

        let vcsh = parse_wx("VCSH").expect("weather");
        assert_eq!(vcsh[0].intensity, WxIntensity::Vicinity);
        assert_eq!(vcsh[0].descriptor, Some(WxDescriptor::Showers));
        assert_eq!(vcsh[0].kind, None);

        let ts = parse_wx("TS").expect("weather");
        assert_eq!(ts[0].descriptor, Some(WxDescriptor::Thunderstorm));
        assert_eq!(ts[0].kind, None);

        let bcfg = parse_wx("BCFG").expect("weather");
        assert_eq!(bcfg[0].descriptor, Some(WxDescriptor::Patches));
        assert_eq!(bcfg[0].kind, Some(WxKind::Fog));
    }

    #[test]
    fn weather_rejects_non_weather() {
        assert_eq!(parse_wx("RERA"), None); // recent weather is not modeled
        assert_eq!(parse_wx("BLU"), None); // NATO colour state
        assert_eq!(parse_wx("BLU+"), None);
        assert_eq!(parse_wx("NSW"), None);
        assert_eq!(parse_wx("-"), None);
        assert_eq!(parse_wx("EDDB"), None);
    }

    #[test]
    fn cloud_layers() {
        assert_eq!(
            parse_cloud("FEW016"),
            Some(CloudGroup::Layer(CloudLayer {
                amount: CloudAmount::Few,
                base_ft_agl: Some(1600),
                kind: None,
            }))
        );
        assert_eq!(
            parse_cloud("BKN015CB"),
            Some(CloudGroup::Layer(CloudLayer {
                amount: CloudAmount::Broken,
                base_ft_agl: Some(1500),
                kind: Some(CloudKind::Cumulonimbus),
            }))
        );
        assert_eq!(
            parse_cloud("SCT030TCU"),
            Some(CloudGroup::Layer(CloudLayer {
                amount: CloudAmount::Scattered,
                base_ft_agl: Some(3000),
                kind: Some(CloudKind::ToweringCumulus),
            }))
        );
        // AUTO stations slash out the unmeasured cloud type / base.
        assert_eq!(
            parse_cloud("OVC051///"),
            Some(CloudGroup::Layer(CloudLayer {
                amount: CloudAmount::Overcast,
                base_ft_agl: Some(5100),
                kind: None,
            }))
        );
        assert_eq!(
            parse_cloud("BKN///"),
            Some(CloudGroup::Layer(CloudLayer {
                amount: CloudAmount::Broken,
                base_ft_agl: None,
                kind: None,
            }))
        );
    }

    #[test]
    fn cloud_no_cloud_codes() {
        let amount = |token| match parse_cloud(token) {
            Some(CloudGroup::Layer(layer)) => layer.amount,
            other => panic!("{token}: {other:?}"),
        };
        assert_eq!(amount("NCD"), CloudAmount::NoCloudDetected);
        assert_eq!(amount("NSC"), CloudAmount::NoSignificantCloud);
        assert_eq!(amount("CLR"), CloudAmount::NoCloudDetected);
        assert_eq!(amount("SKC"), CloudAmount::NoSignificantCloud);
    }

    #[test]
    fn vertical_visibility() {
        assert_eq!(
            parse_cloud("VV003"),
            Some(CloudGroup::VerticalVisibility(Some(300)))
        );
        assert_eq!(parse_cloud("VV///"), Some(CloudGroup::VerticalVisibility(None)));
        assert_eq!(parse_cloud("VVABC"), None);
    }

    #[test]
    fn temp_dew_forms() {
        assert_eq!(parse_temp_dew("10/09"), Some((Some(10), Some(9))));
        assert_eq!(parse_temp_dew("M05/M07"), Some((Some(-5), Some(-7))));
        assert_eq!(parse_temp_dew("09/M01"), Some((Some(9), Some(-1))));
        assert_eq!(parse_temp_dew("10/"), Some((Some(10), None)));
        assert_eq!(parse_temp_dew("00/00"), Some((Some(0), Some(0))));
        assert_eq!(parse_temp_dew("////"), None);
        assert_eq!(parse_temp_dew("1000/1024"), None); // TAF validity
        assert_eq!(parse_temp_dew("1/2SM"), None);
    }

    #[test]
    fn qnh_forms() {
        assert_eq!(parse_qnh("Q1013"), Some(QnhGroup::Value(Qnh::Hpa(1013))));
        assert_eq!(parse_qnh("Q0996"), Some(QnhGroup::Value(Qnh::Hpa(996))));
        assert_eq!(parse_qnh("A2992"), Some(QnhGroup::Value(Qnh::InHg(29.92))));
        assert_eq!(parse_qnh("Q////"), Some(QnhGroup::Unavailable));
        assert_eq!(parse_qnh("AUTO"), None);
        assert_eq!(parse_qnh("A2992X"), None);
    }

    #[test]
    fn slash_only_detection() {
        assert!(is_slash_only("//"));
        assert!(is_slash_only("//////"));
        assert!(!is_slash_only("10/09"));
        assert!(!is_slash_only(""));
    }
}
