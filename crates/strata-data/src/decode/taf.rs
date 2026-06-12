//! TAF decoder: header (station, issue time, validity), base forecast group,
//! and FM/BECMG/TEMPO/PROBnn change groups. Forecast elements reuse the METAR
//! element parsers; unattributable group tokens are skipped (logged at debug)
//! since [`TafGroup`] carries no unparsed-token field.

use chrono::{DateTime, Months, Utc};

use crate::domain::{IcaoCode, Taf, TafChange, TafChangeKind, TafGroup, Visibility};

use super::DecodeError;
use super::elements::{
    CloudGroup, VisGroup, WindGroup, is_slash_only, parse_cloud, parse_statute_miles,
    parse_variable_range, parse_visibility, parse_wind, parse_wx, statute_miles_to_meters,
};
use super::time::{parse_day_time_z, resolve_day_hour, resolve_day_hour_minute};

/// Decodes a raw TAF. `anchor` resolves day/hour tokens (`1000/1024`,
/// `FM101200`) to absolute UTC datetimes — pass the bulletin/issue time
/// from the API, or "now" for live data.
pub fn decode_taf(raw: &str, anchor: DateTime<Utc>) -> Result<Taf, DecodeError> {
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    let Some(&first) = tokens.first() else {
        return Err(DecodeError::Empty);
    };

    let mut i = 0;
    match first {
        "TAF" => i = 1,
        "METAR" | "SPECI" => {
            return Err(DecodeError::WrongReportType {
                expected: "TAF",
                found: first.to_owned(),
            });
        }
        _ => {}
    }
    while matches!(tokens.get(i), Some(&"AMD" | &"COR")) {
        i += 1;
    }

    let &station_token = tokens.get(i).ok_or(DecodeError::Missing("station"))?;
    let station = IcaoCode::new(station_token)
        .map_err(|_| DecodeError::InvalidStation(station_token.to_owned()))?;
    i += 1;

    let issued_at = match tokens.get(i).and_then(|t| {
        let (day, hour, minute) = parse_day_time_z(t)?;
        resolve_day_hour_minute(day, hour, minute, anchor)
    }) {
        Some(at) => {
            i += 1;
            at
        }
        None => anchor,
    };

    let &validity_token = tokens
        .get(i)
        .ok_or(DecodeError::Missing("validity period"))?;
    let (valid_from, valid_to) =
        parse_validity(validity_token, issued_at).ok_or(DecodeError::Missing("validity period"))?;
    i += 1;

    // Split the remainder into the base group and change groups at the
    // FM/BECMG/TEMPO/PROBnn markers.
    struct RawChange<'a> {
        kind: TafChangeKind,
        from: DateTime<Utc>,
        to: Option<DateTime<Utc>>,
        body: Vec<&'a str>,
    }
    let mut base_tokens: Vec<&str> = Vec::new();
    let mut changes: Vec<RawChange<'_>> = Vec::new();
    while i < tokens.len() {
        let token = tokens[i];
        if token == "BECMG" || token == "TEMPO" {
            i += 1;
            let kind = if token == "BECMG" {
                TafChangeKind::Becmg
            } else {
                TafChangeKind::Tempo
            };
            let (from, to) =
                take_window(&tokens, &mut i, issued_at).unwrap_or((valid_from, valid_to));
            changes.push(RawChange {
                kind,
                from,
                to: Some(to),
                body: Vec::new(),
            });
        } else if let Some(probability) = parse_prob(token) {
            i += 1;
            let kind = if tokens.get(i) == Some(&"TEMPO") {
                i += 1;
                TafChangeKind::ProbTempo(probability)
            } else {
                TafChangeKind::Prob(probability)
            };
            let (from, to) =
                take_window(&tokens, &mut i, issued_at).unwrap_or((valid_from, valid_to));
            changes.push(RawChange {
                kind,
                from,
                to: Some(to),
                body: Vec::new(),
            });
        } else if let Some(from) = parse_fm(token, issued_at) {
            i += 1;
            changes.push(RawChange {
                kind: TafChangeKind::Fm,
                from,
                to: None, // filled below: until the next FM group or TAF end
                body: Vec::new(),
            });
        } else {
            match changes.last_mut() {
                Some(change) => change.body.push(token),
                None => base_tokens.push(token),
            }
            i += 1;
        }
    }

    for j in 0..changes.len() {
        if changes[j].to.is_none() {
            let next_fm = changes[j + 1..]
                .iter()
                .find(|c| matches!(c.kind, TafChangeKind::Fm))
                .map(|c| c.from);
            changes[j].to = Some(next_fm.unwrap_or(valid_to));
        }
    }

    Ok(Taf {
        raw: raw.to_owned(),
        station,
        issued_at,
        valid_from,
        valid_to,
        base: parse_group(&base_tokens),
        changes: changes
            .into_iter()
            .map(|c| TafChange {
                kind: c.kind,
                valid_from: c.from,
                valid_to: c.to.unwrap_or(valid_to),
                group: parse_group(&c.body),
            })
            .collect(),
    })
}

/// Parses the forecast elements of a base or change group with the shared
/// METAR element parsers. Tokens without a domain field (NSW, TX/TN
/// temperature forecasts, wind shear, …) are skipped.
fn parse_group(tokens: &[&str]) -> TafGroup {
    let mut group = TafGroup {
        wind: None,
        visibility: None,
        weather: Vec::new(),
        clouds: Vec::new(),
    };
    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i];
        i += 1;

        // NSW = "no significant weather": ends the previous weather, which an
        // empty weather list already expresses. CNL/NIL carry no elements.
        if matches!(token, "NSW" | "CNL" | "NIL") || is_slash_only(token) {
            continue;
        }
        if let Some(wind) = parse_wind(token) {
            match wind {
                WindGroup::Wind(mut wind) if group.wind.is_none() => {
                    if let Some(&next) = tokens.get(i)
                        && let Some(range) = parse_variable_range(next)
                    {
                        wind.variable_range = Some(range);
                        i += 1;
                    }
                    group.wind = Some(wind);
                }
                WindGroup::Wind(_) | WindGroup::Unavailable => {}
            }
            continue;
        }
        if let Some(vis) = parse_visibility(token) {
            if group.visibility.is_none() {
                match vis {
                    VisGroup::Prevailing(v) => group.visibility = Some(v),
                    VisGroup::Directional(meters) => {
                        group.visibility = Some(Visibility::Meters(meters));
                    }
                    VisGroup::Unavailable => {}
                }
            }
            continue;
        }
        if let Some(sm) = parse_statute_miles(token) {
            if group.visibility.is_none() {
                group.visibility = Some(Visibility::Meters(statute_miles_to_meters(sm)));
            }
            continue;
        }
        if let Some(phenomena) = parse_wx(token) {
            group.weather.extend(phenomena);
            continue;
        }
        if let Some(cloud) = parse_cloud(token) {
            match cloud {
                CloudGroup::Layer(layer) => group.clouds.push(layer),
                // TafGroup has no vertical-visibility field.
                CloudGroup::VerticalVisibility(_) => {}
            }
            continue;
        }
        tracing::debug!(token, "TAF group token not attributed");
    }
    group
}

/// Consumes a `ddhh/ddhh` window token at the cursor, if present.
fn take_window(
    tokens: &[&str],
    i: &mut usize,
    anchor: DateTime<Utc>,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let window = parse_validity(tokens.get(*i)?, anchor)?;
    *i += 1;
    Some(window)
}

/// `ddhh/ddhh` validity period.
fn parse_validity(token: &str, anchor: DateTime<Utc>) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let (from_part, to_part) = token.split_once('/')?;
    let from = parse_day_hour(from_part, anchor)?;
    let mut to = parse_day_hour(to_part, anchor)?;
    if to <= from {
        // Both ends resolve independently to the instant nearest the anchor;
        // a wrapped end means the period crosses into the next month.
        to = to.checked_add_months(Months::new(1))?;
    }
    Some((from, to))
}

fn parse_day_hour(part: &str, anchor: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if part.len() != 4 || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    resolve_day_hour(part[..2].parse().ok()?, part[2..].parse().ok()?, anchor)
}

/// `FMddhhmm`.
fn parse_fm(token: &str, anchor: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let rest = token.strip_prefix("FM")?;
    if rest.len() != 6 || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    resolve_day_hour_minute(
        rest[..2].parse().ok()?,
        rest[2..4].parse().ok()?,
        rest[4..6].parse().ok()?,
        anchor,
    )
}

/// `PROBnn`.
fn parse_prob(token: &str) -> Option<u8> {
    let rest = token.strip_prefix("PROB")?;
    if rest.len() != 2 || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::domain::{CloudAmount, CloudKind, WindDirection, WxDescriptor, WxKind};

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(y, mo, d)
            .expect("date")
            .and_hms_opt(h, mi, 0)
            .expect("time")
            .and_utc()
    }

    fn anchor() -> DateTime<Utc> {
        utc(2026, 6, 9, 23, 0)
    }

    fn decode(raw: &str) -> Taf {
        decode_taf(raw, anchor()).unwrap_or_else(|e| panic!("{raw}: {e}"))
    }

    #[test]
    fn empty_and_wrong_type_error() {
        assert!(matches!(decode_taf("", anchor()), Err(DecodeError::Empty)));
        assert!(matches!(
            decode_taf("METAR EDDB 092320Z CAVOK", anchor()),
            Err(DecodeError::WrongReportType {
                expected: "TAF",
                ..
            })
        ));
    }

    #[test]
    fn invalid_station_errors() {
        assert!(matches!(
            decode_taf("TAF X 092300Z 1000/1024 CAVOK", anchor()),
            Err(DecodeError::InvalidStation(_))
        ));
    }

    #[test]
    fn missing_validity_errors() {
        assert!(matches!(
            decode_taf("TAF EDDF 091200Z NIL", anchor()),
            Err(DecodeError::Missing("validity period"))
        ));
    }

    #[test]
    fn header_base_and_change_groups() {
        let taf = decode(
            "TAF EDDB 092300Z 1000/1024 22005KT CAVOK \
             TEMPO 1011/1019 SHRA SCT045CB \
             PROB30 TEMPO 1012/1017 TSRA",
        );
        assert_eq!(taf.station.as_str(), "EDDB");
        assert_eq!(taf.issued_at, utc(2026, 6, 9, 23, 0));
        assert_eq!(taf.valid_from, utc(2026, 6, 10, 0, 0));
        assert_eq!(taf.valid_to, utc(2026, 6, 11, 0, 0)); // hour 24

        let base = &taf.base;
        assert_eq!(
            base.wind.map(|w| (w.direction, w.speed_kt)),
            Some((WindDirection::Degrees(220), 5))
        );
        assert_eq!(base.visibility, Some(Visibility::Cavok));

        assert_eq!(taf.changes.len(), 2);
        let tempo = &taf.changes[0];
        assert_eq!(tempo.kind, TafChangeKind::Tempo);
        assert_eq!(tempo.valid_from, utc(2026, 6, 10, 11, 0));
        assert_eq!(tempo.valid_to, utc(2026, 6, 10, 19, 0));
        assert_eq!(
            tempo.group.weather[0].descriptor,
            Some(WxDescriptor::Showers)
        );
        assert_eq!(tempo.group.weather[0].kind, Some(WxKind::Rain));
        assert_eq!(tempo.group.clouds[0].amount, CloudAmount::Scattered);
        assert_eq!(tempo.group.clouds[0].kind, Some(CloudKind::Cumulonimbus));

        let prob_tempo = &taf.changes[1];
        assert_eq!(prob_tempo.kind, TafChangeKind::ProbTempo(30));
        assert_eq!(prob_tempo.valid_from, utc(2026, 6, 10, 12, 0));
        assert_eq!(prob_tempo.valid_to, utc(2026, 6, 10, 17, 0));
        assert_eq!(
            prob_tempo.group.weather[0].descriptor,
            Some(WxDescriptor::Thunderstorm)
        );
    }

    #[test]
    fn becmg_with_gusting_wind() {
        let taf = decode(
            "TAF EDDV 092300Z 1000/1024 VRB03KT 9999 SCT035 \
             BECMG 1005/1007 24008KT \
             PROB30 TEMPO 1011/1020 27015G30KT 4500 TSRA",
        );
        assert_eq!(taf.changes.len(), 2);
        assert_eq!(taf.changes[0].kind, TafChangeKind::Becmg);
        let wind = taf.changes[1].group.wind.expect("wind");
        assert_eq!(wind.gust_kt, Some(30));
        assert_eq!(
            taf.changes[1].group.visibility,
            Some(Visibility::Meters(4500))
        );
    }

    #[test]
    fn prob_without_tempo() {
        let taf = decode(
            "TAF EDDH 092300Z 1000/1024 20005KT 9999 SCT030 PROB30 1002/1005 0500 FG BKN001",
        );
        assert_eq!(taf.changes.len(), 1);
        assert_eq!(taf.changes[0].kind, TafChangeKind::Prob(30));
        assert_eq!(
            taf.changes[0].group.visibility,
            Some(Visibility::Meters(500))
        );
        assert_eq!(taf.changes[0].group.weather[0].kind, Some(WxKind::Fog));
    }

    #[test]
    fn fm_group_runs_until_next_fm_or_taf_end() {
        let taf = decode(
            "TAF KJFK 092300Z 1000/1024 24010KT P6SM SCT030 \
             FM101200 27015G25KT 5SM -SHRA BKN020 \
             FM102000 30008KT P6SM SKC",
        );
        assert_eq!(taf.changes.len(), 2);
        assert_eq!(taf.changes[0].kind, TafChangeKind::Fm);
        assert_eq!(taf.changes[0].valid_from, utc(2026, 6, 10, 12, 0));
        assert_eq!(taf.changes[0].valid_to, utc(2026, 6, 10, 20, 0));
        assert_eq!(taf.changes[1].valid_from, utc(2026, 6, 10, 20, 0));
        assert_eq!(taf.changes[1].valid_to, taf.valid_to);
        assert_eq!(
            taf.changes[0].group.visibility,
            Some(Visibility::Meters(8047))
        );
    }

    #[test]
    fn nsw_clears_nothing_but_is_consumed() {
        let taf = decode("TAF EDDK 092300Z 1000/1024 14004KT CAVOK BECMG 1006/1008 NSW 26007KT");
        assert!(taf.changes[0].group.weather.is_empty());
        assert!(taf.changes[0].group.wind.is_some());
    }

    #[test]
    fn amd_header_is_accepted() {
        let taf = decode("TAF AMD EDDM 092300Z 1000/1024 VRB03KT 9999 SCT020");
        assert_eq!(taf.station.as_str(), "EDDM");
        assert_eq!(taf.base.clouds.len(), 1);
    }

    /// Every rawTAF in the aviationweather.gov fixture must decode with the
    /// station and validity period matching the API's own decode.
    #[test]
    fn fixture_corpus_decodes() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/aviationweather/tafs-de.json"
        );
        let json = std::fs::read_to_string(path).expect("read fixture");
        let reports: serde_json::Value = serde_json::from_str(&json).expect("parse fixture");
        let reports = reports.as_array().expect("fixture is an array");
        assert!(!reports.is_empty());

        for report in reports {
            let raw = report["rawTAF"].as_str().expect("rawTAF");
            let station = report["icaoId"].as_str().expect("icaoId");
            let issue = report["issueTime"].as_str().expect("issueTime");
            let anchor = DateTime::parse_from_rfc3339(issue)
                .expect("issueTime rfc3339")
                .with_timezone(&Utc);

            let taf = decode_taf(raw, anchor).unwrap_or_else(|e| panic!("{raw}: {e}"));
            assert_eq!(taf.station.as_str(), station, "{raw}");
            assert_eq!(taf.issued_at, anchor, "{raw}");
            assert_eq!(
                taf.valid_from.timestamp(),
                report["validTimeFrom"].as_i64().expect("validTimeFrom"),
                "{raw}"
            );
            assert_eq!(
                taf.valid_to.timestamp(),
                report["validTimeTo"].as_i64().expect("validTimeTo"),
                "{raw}"
            );
            // The API's own decode lists the base group plus one entry per
            // change group.
            let fcsts = report["fcsts"].as_array().expect("fcsts");
            assert_eq!(taf.changes.len(), fcsts.len() - 1, "{raw}");
        }
    }
}
