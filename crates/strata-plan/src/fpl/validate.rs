//! Per-item format validation (ICAO Doc 4444 Appendix 2, format level
//! only — no semantic checks against AIP data).
//!
//! Hand-rolled character checks ("format regexes" without the dependency);
//! every produced item passes through here before assembly, and the
//! validators double as a pre-flight check for user-edited fields.

use super::FplError;

/// Validates one item's field value. Supported items: 7, 8, 9, 10, 13,
/// 15, 16, 18, 19 — anything else is an [`FplError::InvalidItem`].
pub fn validate_item(item: u8, value: &str) -> Result<(), FplError> {
    match item {
        7 => item7(value),
        8 => item8(value),
        9 => item9(value),
        10 => item10(value),
        13 => item13(value),
        15 => item15(value),
        16 => item16(value),
        18 => item18(value),
        19 => item19(value),
        _ => Err(invalid(item, "unsupported item number".to_owned())),
    }
}

fn invalid(item: u8, reason: String) -> FplError {
    FplError::InvalidItem { item, reason }
}

fn is_upper_alnum(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// 24-hour clock `HHMM`.
fn is_time_of_day(s: &str) -> bool {
    s.len() == 4 && is_digits(s) && &s[..2] < "24" && &s[2..] < "60"
}

/// Duration `HHMM` (hours unconstrained to two digits, minutes < 60).
fn is_duration(s: &str) -> bool {
    s.len() == 4 && is_digits(s) && &s[2..] < "60"
}

/// Item 7 — aircraft identification: 1–7 uppercase alphanumerics with at
/// least one letter, no hyphen.
fn item7(value: &str) -> Result<(), FplError> {
    if value.len() <= 7
        && is_upper_alnum(value)
        && value.chars().any(|c| c.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(invalid(
            7,
            format!("{value:?} is not 1-7 alphanumerics with a letter"),
        ))
    }
}

/// Item 8 — flight rules (`I V Y Z`) + optional type of flight
/// (`S N G M X`).
fn item8(value: &str) -> Result<(), FplError> {
    let mut chars = value.chars();
    let rules_ok = chars.next().is_some_and(|c| "IVYZ".contains(c));
    let type_ok = match chars.next() {
        None => true,
        Some(c) => "SNGMX".contains(c) && chars.next().is_none(),
    };
    if rules_ok && type_ok {
        Ok(())
    } else {
        Err(invalid(
            8,
            format!("{value:?} is not rules [IVYZ] + optional type [SNGMX]"),
        ))
    }
}

/// Item 9 — `TYPE/WAKE`: 2–4 character ICAO type designator with at least
/// one letter, wake category `L M H J`.
fn item9(value: &str) -> Result<(), FplError> {
    let err = || invalid(9, format!("{value:?} is not TYPE(2-4 alnum)/[LMHJ]"));
    let (designator, wake) = value.split_once('/').ok_or_else(err)?;
    let designator_ok = (2..=4).contains(&designator.len())
        && is_upper_alnum(designator)
        && designator.chars().any(|c| c.is_ascii_uppercase());
    let wake_ok = wake.len() == 1 && "LMHJ".contains(wake);
    if designator_ok && wake_ok { Ok(()) } else { Err(err()) }
}

/// Item 10 — `COM-NAV/SUR`: both sides non-empty uppercase alphanumerics
/// (`N` = none).
fn item10(value: &str) -> Result<(), FplError> {
    let err = || invalid(10, format!("{value:?} is not EQUIPMENT/SURVEILLANCE"));
    let (com, sur) = value.split_once('/').ok_or_else(err)?;
    if is_upper_alnum(com) && is_upper_alnum(sur) {
        Ok(())
    } else {
        Err(err())
    }
}

/// Item 13 — departure aerodrome + time: 4 letters (`ZZZZ` allowed) + a
/// valid `HHMM` time of day.
fn item13(value: &str) -> Result<(), FplError> {
    let ok = value.len() == 8
        && value.split_at_checked(4).is_some_and(|(ad, time)| {
            ad.chars().all(|c| c.is_ascii_uppercase()) && is_time_of_day(time)
        });
    if ok {
        Ok(())
    } else {
        Err(invalid(13, format!("{value:?} is not AAAA + HHMM")))
    }
}

/// One item 15 route element: `DCT`, a 2–5 character ident, or an ICAO
/// coordinate group (`ddmmN dddmmE`, with or without minutes).
fn is_route_element(token: &str) -> bool {
    if token == "DCT" {
        return true;
    }
    if is_coords(token) {
        return true;
    }
    (2..=5).contains(&token.len())
        && is_upper_alnum(token)
        && token.chars().any(|c| c.is_ascii_uppercase())
}

fn is_coords(token: &str) -> bool {
    let split = |s: &str, ns: usize| -> Option<(u32, u32, char)> {
        if s.is_empty() {
            return None;
        }
        // Checked split: a multibyte char straddling the boundary is not a
        // valid hemisphere letter anyway.
        let (digits, hemisphere) = s.split_at_checked(s.len() - 1)?;
        let h = hemisphere.chars().next()?;
        if !is_digits(digits) {
            return None;
        }
        match digits.len() {
            n if n == ns => Some((digits.parse().ok()?, 0, h)),
            n if n == ns + 2 => Some((
                digits[..ns].parse().ok()?,
                digits[ns..].parse().ok()?,
                h,
            )),
            _ => None,
        }
    };
    let Some(ns_pos) = token.find(['N', 'S']) else {
        return false;
    };
    let (lat, lon) = token.split_at(ns_pos + 1);
    let Some((lat_deg, lat_min, lat_h)) = split(lat, 2) else {
        return false;
    };
    let Some((lon_deg, lon_min, lon_h)) = split(lon, 3) else {
        return false;
    };
    matches!(lat_h, 'N' | 'S')
        && matches!(lon_h, 'E' | 'W')
        && lat_deg <= 90
        && lat_min < 60
        && lon_deg <= 180
        && lon_min < 60
}

/// Item 15 — cruising speed + level, then route elements.
fn item15(value: &str) -> Result<(), FplError> {
    let mut tokens = value.split_whitespace();
    let Some(first) = tokens.next() else {
        return Err(invalid(15, "empty item".to_owned()));
    };
    let speed_len = match first.chars().next() {
        Some('N') | Some('K') => 5, // N/K + 4 digits
        Some('M') => 4,             // M + 3 digits
        _ => 0,
    };
    // Checked split: multibyte input must yield Err, never panic.
    let split = (speed_len > 0 && first.len() > speed_len)
        .then(|| first.split_at_checked(speed_len))
        .flatten();
    let (speed_ok, level) = match split {
        Some((speed, level)) => (speed.get(1..).is_some_and(is_digits), level),
        None => (false, ""),
    };
    let level_ok = match level.split_at_checked(1) {
        Some(("F", digits)) | Some(("A", digits)) => digits.len() == 3 && is_digits(digits),
        Some(("S", digits)) | Some(("M", digits)) => digits.len() == 4 && is_digits(digits),
        _ => level == "VFR",
    };
    if !(speed_ok && level_ok) {
        return Err(invalid(
            15,
            format!("{first:?} is not speed (N####/K####/M###) + level (F###/A###/S####/M####/VFR)"),
        ));
    }
    for token in tokens {
        if !is_route_element(token) {
            return Err(invalid(15, format!("{token:?} is not a route element")));
        }
    }
    Ok(())
}

/// Item 16 — destination + total EET + up to two alternate aerodromes.
fn item16(value: &str) -> Result<(), FplError> {
    let err = |what: &str| invalid(16, format!("{value:?}: {what}"));
    let mut tokens = value.split_whitespace();
    let Some(first) = tokens.next() else {
        return Err(err("empty item"));
    };
    let dest_ok = first.len() == 8
        && first.split_at_checked(4).is_some_and(|(ad, eet)| {
            ad.chars().all(|c| c.is_ascii_uppercase()) && is_duration(eet)
        });
    if !dest_ok {
        return Err(err("destination must be AAAA + HHMM"));
    }
    let alternates: Vec<&str> = tokens.collect();
    if alternates.len() > 2 {
        return Err(err("at most two alternates"));
    }
    for alternate in alternates {
        if !(alternate.len() == 4 && alternate.chars().all(|c| c.is_ascii_uppercase())) {
            return Err(err("alternates must be 4-letter indicators"));
        }
    }
    Ok(())
}

/// Item 18 — `0` or `IND/value` groups (free-text values may continue
/// over following tokens).
fn item18(value: &str) -> Result<(), FplError> {
    if value == "0" {
        return Ok(());
    }
    let mut tokens = value.split_whitespace();
    let Some(first) = tokens.next() else {
        return Err(invalid(18, "empty item (use \"0\")".to_owned()));
    };
    if !is_group_start(first) {
        return Err(invalid(
            18,
            format!("{first:?} does not start an IND/value group"),
        ));
    }
    Ok(())
}

fn is_group_start(token: &str) -> bool {
    token.split_once('/').is_some_and(|(indicator, rest)| {
        (1..=5).contains(&indicator.len())
            && indicator.chars().all(|c| c.is_ascii_uppercase())
            && !rest.is_empty()
    })
}

/// Item 19 — supplementary groups; validates the format of every present
/// group (`E/HHMM`, `P/n|TBN`, `R/[UVE]+`, `S/[PDMJ]+`, `J/[LFUV]+`,
/// `D A N C` free text).
fn item19(value: &str) -> Result<(), FplError> {
    let tokens: Vec<&str> = value.split_whitespace().collect();
    if tokens.is_empty() {
        return Err(invalid(19, "empty item".to_owned()));
    }
    if !is_group_start(tokens[0]) {
        return Err(invalid(
            19,
            format!("{:?} does not start an IND/value group", tokens[0]),
        ));
    }
    let mut groups: Vec<(char, String)> = Vec::new();
    for token in tokens {
        if let Some((indicator, rest)) = token.split_once('/')
            && indicator.len() == 1
            && indicator.chars().all(|c| c.is_ascii_uppercase())
        {
            let c = indicator.chars().next().expect("len checked");
            groups.push((c, rest.to_owned()));
        } else if let Some(last) = groups.last_mut() {
            last.1.push(' ');
            last.1.push_str(token);
        } else {
            return Err(invalid(19, format!("{token:?} outside any group")));
        }
    }
    for (indicator, group_value) in &groups {
        let ok = match indicator {
            'E' => is_duration(group_value),
            'P' => {
                group_value == "TBN"
                    || ((1..=3).contains(&group_value.len()) && is_digits(group_value))
            }
            'R' => !group_value.is_empty() && group_value.chars().all(|c| "UVE".contains(c)),
            'S' => !group_value.is_empty() && group_value.chars().all(|c| "PDMJ".contains(c)),
            'J' => !group_value.is_empty() && group_value.chars().all(|c| "LFUV".contains(c)),
            'D' | 'A' | 'N' | 'C' => !group_value.is_empty(),
            _ => false,
        };
        if !ok {
            return Err(invalid(
                19,
                format!("group {indicator}/{group_value} is malformed"),
            ));
        }
    }
    Ok(())
}
