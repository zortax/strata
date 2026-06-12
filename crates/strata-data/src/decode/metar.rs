//! METAR body decoder: tolerant ordered-group parser. Unattributable tokens
//! land in [`MetarDecode::unparsed_tokens`]; a real-world METAR never fails
//! to decode — only fundamentally unusable input (empty, or a TAF passed in)
//! errors.

use crate::domain::{MetarDecode, Trend, Visibility};

use super::DecodeError;
use super::elements::{
    CloudGroup, QnhGroup, VisGroup, WindGroup, is_slash_only, parse_cloud, parse_qnh,
    parse_statute_miles, parse_temp_dew, parse_variable_range, parse_visibility, parse_wind,
    parse_wx, statute_miles_to_meters,
};
use super::time::parse_day_time_z;

/// Decodes a raw METAR body (with or without the leading `METAR`/`SPECI`
/// keyword and station/time groups). Unattributable tokens land in
/// [`MetarDecode::unparsed_tokens`] instead of failing the decode.
pub fn decode_metar(raw: &str) -> Result<MetarDecode, DecodeError> {
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    let Some(&first) = tokens.first() else {
        return Err(DecodeError::Empty);
    };

    let mut i = 0;
    match first {
        "METAR" | "SPECI" => i = 1,
        "TAF" => {
            return Err(DecodeError::WrongReportType {
                expected: "METAR",
                found: first.to_owned(),
            });
        }
        _ => {}
    }
    while matches!(tokens.get(i), Some(&"COR" | &"AMD")) {
        i += 1;
    }
    // Station and observation time carry no MetarDecode fields (the provider
    // takes them from API metadata); skip them when present.
    if tokens.get(i).is_some_and(|t| looks_like_station(t)) {
        i += 1;
    }
    if tokens.get(i).is_some_and(|t| parse_day_time_z(t).is_some()) {
        i += 1;
    }

    let mut decode = MetarDecode {
        wind: None,
        visibility: None,
        weather: Vec::new(),
        clouds: Vec::new(),
        vertical_visibility_ft: None,
        temperature_c: None,
        dewpoint_c: None,
        qnh: None,
        trend: None,
        auto: false,
        remarks: None,
        unparsed_tokens: Vec::new(),
    };
    let mut unparsed: Vec<&str> = Vec::new();

    while i < tokens.len() {
        let token = tokens[i];
        i += 1;

        match token {
            "AUTO" => {
                decode.auto = true;
                continue;
            }
            "COR" | "AMD" | "NSW" => continue,
            "RMK" => {
                let remarks = tokens[i..].join(" ");
                decode.remarks = (!remarks.is_empty()).then_some(remarks);
                break;
            }
            "NOSIG" => {
                decode.trend = Some(Trend::Nosig);
                continue;
            }
            "BECMG" | "TEMPO" => {
                decode.trend = Some(if token == "BECMG" {
                    Trend::Becmg
                } else {
                    Trend::Tempo
                });
                // Simplified trend handling: the trend group's own elements
                // are consumed but not modeled (only the trend kind is kept).
                while i < tokens.len() && tokens[i] != "RMK" {
                    i += 1;
                }
                continue;
            }
            _ => {}
        }

        if is_slash_only(token) {
            continue;
        }

        if let Some(group) = parse_wind(token) {
            match group {
                WindGroup::Wind(mut wind) if decode.wind.is_none() => {
                    if let Some(&next) = tokens.get(i)
                        && let Some(range) = parse_variable_range(next)
                    {
                        wind.variable_range = Some(range);
                        i += 1;
                    }
                    decode.wind = Some(wind);
                }
                WindGroup::Wind(_) => unparsed.push(token),
                WindGroup::Unavailable => {}
            }
            continue;
        }
        if let Some(range) = parse_variable_range(token) {
            match decode.wind.as_mut() {
                Some(wind) if wind.variable_range.is_none() => {
                    wind.variable_range = Some(range);
                }
                _ => unparsed.push(token),
            }
            continue;
        }

        if let Some(group) = parse_visibility(token) {
            match group {
                VisGroup::Prevailing(vis) if decode.visibility.is_none() => {
                    decode.visibility = Some(vis);
                }
                VisGroup::Prevailing(_) => unparsed.push(token),
                // Directional minimum visibility: only used as a fallback
                // when no prevailing visibility was reported.
                VisGroup::Directional(meters) if decode.visibility.is_none() => {
                    decode.visibility = Some(Visibility::Meters(meters));
                }
                VisGroup::Directional(_) | VisGroup::Unavailable => {}
            }
            continue;
        }
        if let Some(sm) = parse_statute_miles(token) {
            if decode.visibility.is_none() {
                decode.visibility = Some(Visibility::Meters(statute_miles_to_meters(sm)));
            } else {
                unparsed.push(token);
            }
            continue;
        }
        // Two-token statute-mile form: "2 1/2SM".
        if token.len() <= 2
            && token.bytes().all(|b| b.is_ascii_digit())
            && let Some(&next) = tokens.get(i)
            && next.contains('/')
            && let Some(fraction) = parse_statute_miles(next)
            && let Ok(whole) = token.parse::<f64>()
        {
            i += 1;
            if decode.visibility.is_none() {
                decode.visibility = Some(Visibility::Meters(statute_miles_to_meters(
                    whole + fraction,
                )));
            }
            continue;
        }

        if let Some(phenomena) = parse_wx(token) {
            decode.weather.extend(phenomena);
            continue;
        }

        if let Some(group) = parse_cloud(token) {
            match group {
                CloudGroup::Layer(layer) => decode.clouds.push(layer),
                CloudGroup::VerticalVisibility(vv) => {
                    decode.vertical_visibility_ft = decode.vertical_visibility_ft.or(vv);
                }
            }
            continue;
        }

        if let Some((temperature, dewpoint)) = parse_temp_dew(token) {
            if decode.temperature_c.is_none() && decode.dewpoint_c.is_none() {
                decode.temperature_c = temperature;
                decode.dewpoint_c = dewpoint;
            } else {
                unparsed.push(token);
            }
            continue;
        }

        if let Some(group) = parse_qnh(token) {
            match group {
                QnhGroup::Value(qnh) if decode.qnh.is_none() => decode.qnh = Some(qnh),
                QnhGroup::Value(_) => unparsed.push(token),
                QnhGroup::Unavailable => {}
            }
            continue;
        }

        // RVR (R27/1200N), runway state, wind shear, colour states, recent
        // weather, … — recognized real-world groups without domain fields all
        // deliberately land here.
        unparsed.push(token);
    }

    if !unparsed.is_empty() {
        tracing::debug!(tokens = ?unparsed, raw, "METAR tokens not attributed");
    }
    decode.unparsed_tokens = unparsed.into_iter().map(str::to_owned).collect();
    Ok(decode)
}

/// A station indicator: 4 alphanumerics starting with a letter that does not
/// read as a body group (so `TSRA` or `AUTO` at the start of a headerless
/// body is not mistaken for a station).
fn looks_like_station(token: &str) -> bool {
    token.len() == 4
        && token.bytes().all(|b| b.is_ascii_alphanumeric())
        && token.as_bytes()[0].is_ascii_alphabetic()
        && !matches!(token, "AUTO" | "GRID")
        && parse_wx(token).is_none()
        && parse_cloud(token).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        CloudAmount, CloudKind, FlightCategory, IcaoCode, Qnh, Wind, WindDirection, WxDescriptor,
        WxIntensity, WxKind,
    };

    fn decode(raw: &str) -> MetarDecode {
        decode_metar(raw).unwrap_or_else(|e| panic!("{raw}: {e}"))
    }

    #[test]
    fn empty_input_errors() {
        assert_eq!(decode_metar(""), Err(DecodeError::Empty));
        assert_eq!(decode_metar("   \t "), Err(DecodeError::Empty));
    }

    #[test]
    fn taf_input_is_wrong_report_type() {
        assert_eq!(
            decode_metar("TAF EDDB 092300Z 1000/1024 22005KT CAVOK"),
            Err(DecodeError::WrongReportType {
                expected: "METAR",
                found: "TAF".to_owned(),
            })
        );
    }

    #[test]
    fn full_auto_report() {
        let d = decode("METAR EDHK 092320Z AUTO 20006KT 160V240 9000 // OVC051/// 09/08 Q1013");
        assert!(d.auto);
        assert_eq!(
            d.wind,
            Some(Wind {
                direction: WindDirection::Degrees(200),
                speed_kt: 6,
                gust_kt: None,
                variable_range: Some((160, 240)),
            })
        );
        assert_eq!(d.visibility, Some(Visibility::Meters(9000)));
        assert_eq!(d.clouds.len(), 1);
        assert_eq!(d.clouds[0].amount, CloudAmount::Overcast);
        assert_eq!(d.clouds[0].base_ft_agl, Some(5100));
        assert_eq!(d.temperature_c, Some(9));
        assert_eq!(d.dewpoint_c, Some(8));
        assert_eq!(d.qnh, Some(Qnh::Hpa(1013)));
        assert!(d.unparsed_tokens.is_empty(), "{:?}", d.unparsed_tokens);
    }

    #[test]
    fn cavok_and_nosig() {
        let d = decode("METAR EDDP 092320Z AUTO 15006KT CAVOK 13/09 Q1017 NOSIG");
        assert_eq!(d.visibility, Some(Visibility::Cavok));
        assert_eq!(d.trend, Some(Trend::Nosig));
        assert_eq!(d.flight_category(), Some(FlightCategory::Vfr));
        assert!(d.unparsed_tokens.is_empty());
    }

    #[test]
    fn negative_temperatures() {
        let d = decode("EDDB 011200Z 27010KT 9999 NCD M05/M07 Q0996");
        assert_eq!(d.temperature_c, Some(-5));
        assert_eq!(d.dewpoint_c, Some(-7));
        assert_eq!(d.qnh, Some(Qnh::Hpa(996)));
        assert_eq!(d.clouds[0].amount, CloudAmount::NoCloudDetected);
    }

    #[test]
    fn variable_low_wind() {
        let d = decode("METAR EDDC 092320Z AUTO VRB02KT CAVOK 13/08 Q1017");
        assert_eq!(
            d.wind,
            Some(Wind {
                direction: WindDirection::Variable,
                speed_kt: 2,
                gust_kt: None,
                variable_range: None,
            })
        );
    }

    #[test]
    fn gusts_and_variable_range_together() {
        let d = decode("EDDF 011200Z 22010G25KT 180V240 9999 BKN015CB 17/12 Q1008");
        assert_eq!(
            d.wind,
            Some(Wind {
                direction: WindDirection::Degrees(220),
                speed_kt: 10,
                gust_kt: Some(25),
                variable_range: Some((180, 240)),
            })
        );
        assert_eq!(d.clouds[0].kind, Some(CloudKind::Cumulonimbus));
        assert_eq!(d.ceiling_ft_agl(), Some(1500));
    }

    #[test]
    fn inches_of_mercury_and_remarks() {
        let d = decode("METAR ETIH 092255Z AUTO 00000KT 9999 BKN140 09/07 A3009 RMK AO2 SLP193");
        assert_eq!(d.qnh, Some(Qnh::InHg(30.09)));
        assert_eq!(d.remarks.as_deref(), Some("AO2 SLP193"));
        assert!(d.unparsed_tokens.is_empty());
    }

    #[test]
    fn weather_groups_decode() {
        let d = decode("LOWI 092320Z AUTO VRB01KT 9999 -RA FEW009 SCT026 BKN046 13/11 Q1019");
        assert_eq!(d.weather.len(), 1);
        assert_eq!(d.weather[0].intensity, WxIntensity::Light);
        assert_eq!(d.weather[0].kind, Some(WxKind::Rain));
        assert_eq!(d.clouds.len(), 3);
        assert_eq!(d.ceiling_ft_agl(), Some(4600));
    }

    #[test]
    fn thunderstorm_descriptor() {
        let d = decode("EDDM 011200Z 24015G30KT 4500 +TSRA SCT020CB 20/16 Q1011");
        assert_eq!(d.weather[0].descriptor, Some(WxDescriptor::Thunderstorm));
        assert_eq!(d.weather[0].intensity, WxIntensity::Heavy);
        // 4500 m = 2.8 SM, below the 3 SM MVFR floor.
        assert_eq!(d.flight_category(), Some(FlightCategory::Ifr));
    }

    #[test]
    fn vertical_visibility_group() {
        let d = decode("EDDH 011200Z 00000KT 0200 FG VV002 08/08 Q1021");
        assert_eq!(d.vertical_visibility_ft, Some(200));
        assert_eq!(d.ceiling_ft_agl(), Some(200));
        assert_eq!(d.flight_category(), Some(FlightCategory::Lifr));
    }

    #[test]
    fn colour_state_lands_in_unparsed() {
        let d = decode("METAR EHVK 092325Z AUTO 24004KT 9999 OVC037 10/10 Q1016 BLU");
        assert_eq!(d.unparsed_tokens, vec!["BLU".to_owned()]);
        let d = decode("METAR ETMN 092320Z 20008KT 9999 FEW020 09/08 Q1013 BLU+");
        assert_eq!(d.unparsed_tokens, vec!["BLU+".to_owned()]);
    }

    #[test]
    fn unavailable_auto_groups_are_consumed_silently() {
        let d = decode("METAR ETWM 092320Z AUTO 21004KT //// // ////// 10/08 Q1014 ///");
        assert!(d.unparsed_tokens.is_empty(), "{:?}", d.unparsed_tokens);
        assert_eq!(d.visibility, None);
        assert!(d.clouds.is_empty());
        assert_eq!(d.flight_category(), None);
    }

    #[test]
    fn rvr_token_is_tolerated_as_unparsed() {
        let d = decode("EDDL 011200Z 27005KT 0400 R23L/1200N FG VV001 05/05 Q1030");
        assert_eq!(d.unparsed_tokens, vec!["R23L/1200N".to_owned()]);
        assert_eq!(d.visibility, Some(Visibility::Meters(400)));
    }

    #[test]
    fn headerless_body_decodes() {
        let d = decode("21008KT 9999 NCD 10/09 Q1013");
        assert_eq!(d.qnh, Some(Qnh::Hpa(1013)));
        assert_eq!(d.visibility, Some(Visibility::Meters(9999)));
        assert!(d.unparsed_tokens.is_empty());
    }

    #[test]
    fn headerless_body_starting_with_weather_keeps_the_weather() {
        let d = decode("TSRA 4000 BKN020CB 20/18 Q1009");
        assert_eq!(d.weather.len(), 1);
        assert_eq!(d.weather[0].descriptor, Some(WxDescriptor::Thunderstorm));
    }

    #[test]
    fn becmg_trend_is_simplified() {
        let d = decode("EDDK 011200Z 18005KT CAVOK 10/07 Q1017 BECMG 25015G25KT 4000 SHRA");
        assert_eq!(d.trend, Some(Trend::Becmg));
        assert!(d.unparsed_tokens.is_empty());
        // The trend group must not pollute the main observation.
        assert_eq!(d.visibility, Some(Visibility::Cavok));
        assert!(d.weather.is_empty());
    }

    #[test]
    fn statute_mile_visibility_forms() {
        let d = decode("KJFK 011251Z 28010KT 6SM FEW250 17/12 A3005");
        assert_eq!(d.visibility, Some(Visibility::Meters(9656)));
        let d = decode("KJFK 011251Z 28010KT 2 1/2SM BR OVC005 17/12 A3005");
        assert_eq!(d.visibility, Some(Visibility::Meters(4023)));
        assert!(d.unparsed_tokens.is_empty(), "{:?}", d.unparsed_tokens);
    }

    /// Every rawOb in the aviationweather.gov fixture must decode cleanly:
    /// no error, a parsed QNH, and the station token consumed by the header.
    #[test]
    fn fixture_corpus_decodes() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/aviationweather/metars-de.json"
        );
        let json = std::fs::read_to_string(path).expect("read fixture");
        let reports: serde_json::Value = serde_json::from_str(&json).expect("parse fixture");
        let reports = reports.as_array().expect("fixture is an array");
        assert!(!reports.is_empty());

        for report in reports {
            let raw = report["rawOb"].as_str().expect("rawOb");
            let station = report["icaoId"].as_str().expect("icaoId");
            let d = decode(raw);
            assert!(IcaoCode::new(station).is_ok(), "bad station in {raw}");
            assert!(d.qnh.is_some(), "QNH missing: {raw}");
            assert!(
                !d.unparsed_tokens.iter().any(|t| t == station),
                "station leaked into unparsed tokens: {raw}"
            );
            assert!(
                d.wind.is_some() || d.auto,
                "wind missing on a manned station: {raw}"
            );
        }
    }
}
