//! Transmission-format parser: header + ordered item markers.
//!
//! Items are located by scanning for `X)` markers (X in Q,A,…,G) at
//! whitespace boundaries in ascending item order — ascending order is what
//! keeps markers-as-content in item E (e.g. a `B)` inside the text) from
//! being mistaken for items. The heuristic limit: an `F)`/`G)` *inside* E
//! text at a word boundary would split early; real NOTAM E texts do not do
//! this, and the fixture corpus pins the behaviour.

use crate::domain::IcaoCode;

use super::{Notam, NotamItems, NotamKind, NotamParseError, NotamValidity, QLine, validity};

/// Item letters in transmission order.
const ITEM_ORDER: [char; 8] = ['Q', 'A', 'B', 'C', 'D', 'E', 'F', 'G'];

pub(super) fn parse(input: &str) -> Result<Notam, NotamParseError> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(NotamParseError::Empty);
    }
    let body = strip_outer_parens(raw);
    let (header, items) = split_items(body)?;
    let (id, kind) = parse_header(header)?;

    let q_body = require_item(&items, 'Q')?;
    let q = QLine::parse(q_body)?;

    let a_body = require_item(&items, 'A')?;
    let locations = a_body
        .split_whitespace()
        .map(|token| {
            IcaoCode::new(token).map_err(|_| NotamParseError::InvalidLocation(token.to_owned()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if locations.is_empty() {
        return Err(NotamParseError::MissingItem('A'));
    }

    let b_body = require_item(&items, 'B')?;
    let from = validity::parse_compact_datetime(b_body.trim())?;

    let c_body = find_item(&items, 'C');
    let until = match c_body {
        Some(c) => validity::parse_item_c(c)?,
        // A NOTAMC carries no item C; the cancellation never expires.
        None if matches!(kind, NotamKind::Cancellation { .. }) => super::NotamEnd::Permanent,
        None => return Err(NotamParseError::MissingItem('C')),
    };

    let e_body = require_item(&items, 'E')?;

    Ok(Notam {
        id,
        kind,
        q,
        locations,
        validity: NotamValidity { from, until },
        schedule: find_item(&items, 'D').map(str::to_owned),
        text: e_body.to_owned(),
        items: NotamItems {
            q: q_body.to_owned(),
            a: a_body.to_owned(),
            b: b_body.to_owned(),
            c: c_body.map(str::to_owned),
            d: find_item(&items, 'D').map(str::to_owned),
            e: e_body.to_owned(),
            f: find_item(&items, 'F').map(str::to_owned),
            g: find_item(&items, 'G').map(str::to_owned),
        },
        raw: raw.to_owned(),
    })
}

/// Strips one matching pair of outer parentheses (the ICAO message form
/// wraps the whole NOTAM in `(...)`).
fn strip_outer_parens(s: &str) -> &str {
    s.strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
        .map_or(s, str::trim)
}

/// Item letter + verbatim body, in transmission order.
type ItemBodies<'a> = Vec<(char, &'a str)>;

/// Splits the body into the header (before item Q) and the item bodies.
fn split_items(body: &str) -> Result<(&str, ItemBodies<'_>), NotamParseError> {
    let bytes = body.as_bytes();
    // (marker start, content start, letter), ascending item order enforced.
    let mut markers: Vec<(usize, usize, char)> = Vec::new();
    let mut last_rank: Option<usize> = None;

    let mut i = 0;
    while i + 1 < bytes.len() {
        let letter = bytes[i] as char;
        let at_boundary = i == 0 || bytes[i - 1].is_ascii_whitespace();
        let closed = bytes[i + 1] == b')';
        let followed_ok = i + 2 >= bytes.len() || bytes[i + 2].is_ascii_whitespace();
        if at_boundary
            && closed
            && followed_ok
            && let Some(rank) = ITEM_ORDER.iter().position(|&m| m == letter)
            && last_rank.is_none_or(|last| rank > last)
        {
            markers.push((i, i + 2, letter));
            last_rank = Some(rank);
            i += 2;
            continue;
        }
        i += 1;
    }

    let Some(&(first_start, _, first_letter)) = markers.first() else {
        return Err(NotamParseError::MissingItem('Q'));
    };
    if first_letter != 'Q' {
        return Err(NotamParseError::MissingItem('Q'));
    }

    let header = body[..first_start].trim();
    let mut items = Vec::with_capacity(markers.len());
    for (index, &(_, content_start, letter)) in markers.iter().enumerate() {
        let content_end = markers
            .get(index + 1)
            .map_or(body.len(), |&(next_start, _, _)| next_start);
        items.push((letter, body[content_start..content_end].trim()));
    }
    Ok((header, items))
}

fn find_item<'a>(items: &[(char, &'a str)], letter: char) -> Option<&'a str> {
    items
        .iter()
        .find(|&&(l, _)| l == letter)
        .map(|&(_, body)| body)
        .filter(|body| !body.is_empty())
}

fn require_item<'a>(items: &[(char, &'a str)], letter: char) -> Result<&'a str, NotamParseError> {
    find_item(items, letter).ok_or(NotamParseError::MissingItem(letter))
}

/// `A1234/26 NOTAMN` — id, kind, and the referenced id for `R`/`C`.
fn parse_header(header: &str) -> Result<(super::NotamId, NotamKind), NotamParseError> {
    let err = || NotamParseError::MalformedHeader(header.to_owned());
    let mut tokens = header.split_whitespace();
    let id: super::NotamId = tokens.next().ok_or_else(err)?.parse()?;
    let kind = match tokens.next().ok_or_else(err)? {
        "NOTAMN" => NotamKind::New,
        "NOTAMR" => NotamKind::Replacement {
            replaces: parse_reference(tokens.next(), 'R')?,
        },
        "NOTAMC" => NotamKind::Cancellation {
            cancels: parse_reference(tokens.next(), 'C')?,
        },
        _ => return Err(err()),
    };
    Ok((id, kind))
}

fn parse_reference(token: Option<&str>, kind: char) -> Result<super::NotamId, NotamParseError> {
    token
        .ok_or(NotamParseError::MissingReference { kind })?
        .parse()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VerticalReference;
    use crate::domain::notam::{NotamEnd, QCondition, QSubject};

    const RWY_CLOSURE: &str = "A1234/26 NOTAMN\n\
        Q) EDGG/QMRLC/IV/NBO/A/000/999/5002N00834E005\n\
        A) EDDF B) 2606150600 C) 2606171800\n\
        E) RWY 07C/25C CLSD DUE TO RWY MAINT";

    #[test]
    fn parses_a_complete_notam() {
        let notam = Notam::parse(RWY_CLOSURE).expect("parses");
        assert_eq!(notam.id.to_string(), "A1234/26");
        assert_eq!(notam.kind, NotamKind::New);
        assert_eq!(notam.fir().as_str(), "EDGG");
        assert_eq!(notam.q.code.subject, QSubject::Runway);
        assert_eq!(notam.q.code.condition, QCondition::Closed);
        assert_eq!(notam.locations.len(), 1);
        assert_eq!(notam.locations[0].as_str(), "EDDF");
        assert_eq!(notam.text, "RWY 07C/25C CLSD DUE TO RWY MAINT");
        assert_eq!(notam.schedule, None);
        assert_eq!(notam.items.b, "2606150600");
        assert_eq!(notam.items.c.as_deref(), Some("2606171800"));
        assert_eq!(notam.raw, RWY_CLOSURE);
    }

    #[test]
    fn parses_parenthesized_message_form() {
        let wrapped = format!("({RWY_CLOSURE})");
        let notam = Notam::parse(&wrapped).expect("parses");
        assert_eq!(notam.id.to_string(), "A1234/26");
        // Raw keeps what was received, including the parentheses.
        assert_eq!(notam.raw, wrapped);
    }

    #[test]
    fn parses_replacement_reference() {
        let text = "A1300/26 NOTAMR A1198/26\n\
            Q) EDGG/QMNLC/IV/BO/A/000/999/5002N00834E005\n\
            A) EDDF B) 2606121000 C) 2606302359\n\
            E) APRON 3 EAST PART CLSD";
        let notam = Notam::parse(text).expect("parses");
        let NotamKind::Replacement { replaces } = notam.kind else {
            panic!("expected replacement, got {:?}", notam.kind);
        };
        assert_eq!(replaces.to_string(), "A1198/26");
    }

    #[test]
    fn parses_cancellation_without_item_c() {
        let text = "A1310/26 NOTAMC A1241/26\n\
            Q) EDGG/QMXXX/IV/M/A/000/999/5002N00834E005\n\
            A) EDDF B) 2606161200\n\
            E) TWY N WORK COMPLETED";
        let notam = Notam::parse(text).expect("parses");
        let NotamKind::Cancellation { cancels } = notam.kind else {
            panic!("expected cancellation, got {:?}", notam.kind);
        };
        assert_eq!(cancels.to_string(), "A1241/26");
        assert_eq!(notam.validity.until, NotamEnd::Permanent);
        assert_eq!(notam.items.c, None);
    }

    #[test]
    fn missing_item_c_on_a_notamn_is_an_error() {
        let text = "A1234/26 NOTAMN\n\
            Q) EDGG/QMRLC/IV/NBO/A/000/999/5002N00834E005\n\
            A) EDDF B) 2606150600\n\
            E) RWY CLSD";
        assert_eq!(Notam::parse(text), Err(NotamParseError::MissingItem('C')));
    }

    #[test]
    fn parses_schedule_and_f_g_limits() {
        let text = "B0815/26 NOTAMN\n\
            Q) EDMM/QOBCE/IV/M/AE/000/021/4822N01145E001\n\
            A) EDDM B) 2606120000 C) 2609301200EST\n\
            D) DLY 0500-2000\n\
            E) CRANE ERECTED 1.2KM NE THR RWY 26L\n\
            F) GND\n\
            G) 1916FT AMSL";
        let notam = Notam::parse(text).expect("parses");
        assert_eq!(notam.schedule.as_deref(), Some("DLY 0500-2000"));
        assert_eq!(notam.items.f.as_deref(), Some("GND"));
        assert_eq!(notam.items.g.as_deref(), Some("1916FT AMSL"));
        assert!(matches!(notam.validity.until, NotamEnd::Estimated(_)));
        assert_eq!(notam.q.upper.reference, VerticalReference::Fl(21));
        assert!(notam.q.scope.aerodrome && notam.q.scope.enroute);
    }

    #[test]
    fn multiline_item_e_with_marker_lookalikes_stays_one_item() {
        let text = "E0231/26 NOTAMN\n\
            Q) EDGG/QGWAU/IV/NBO/E/000/999/5007N00840E150\n\
            A) EDGG B) 2606140000 C) 2606212359EST\n\
            E) GPS SIGNAL UNRELIABLE WI 150NM RADIUS OF 5007N00840E.\n\
            POSSIBLE DEGRADATION B) OF GNSS BASED NAV AND SURVEILLANCE";
        let notam = Notam::parse(text).expect("parses");
        // The stray `B)` inside item E must not split the item (markers
        // are only accepted in ascending order).
        assert!(notam.text.contains("DEGRADATION B) OF GNSS"));
        assert_eq!(notam.items.b, "2606140000");
    }

    #[test]
    fn multiple_locations_in_item_a() {
        let text = "A1400/26 NOTAMN\n\
            Q) EDGG/QSELT/IV/BO/AE/000/999/5002N00834E100\n\
            A) EDDF EDFE B) 2606150600 C) 2606171800\n\
            E) FIS LANGEN FREQ 119.150 LIMITED";
        let notam = Notam::parse(text).expect("parses");
        let codes: Vec<&str> = notam.locations.iter().map(IcaoCode::as_str).collect();
        assert_eq!(codes, ["EDDF", "EDFE"]);
    }

    #[test]
    fn header_errors() {
        assert_eq!(Notam::parse(""), Err(NotamParseError::Empty));
        assert_eq!(Notam::parse("   \n "), Err(NotamParseError::Empty));
        assert!(matches!(
            Notam::parse(
                "NOTAMN\nQ) EDGG/QMRLC/IV/NBO/A/000/999/5002N00834E005\nA) EDDF B) 2606150600 C) 2606171800\nE) X"
            ),
            Err(NotamParseError::MalformedId(_))
        ));
        assert!(matches!(
            Notam::parse(
                "A1234/26 NOTAMX\nQ) EDGG/QMRLC/IV/NBO/A/000/999/5002N00834E005\nA) EDDF B) 2606150600 C) 2606171800\nE) X"
            ),
            Err(NotamParseError::MalformedHeader(_))
        ));
        assert_eq!(
            Notam::parse(
                "A1300/26 NOTAMR\nQ) EDGG/QMRLC/IV/NBO/A/000/999/5002N00834E005\nA) EDDF B) 2606150600 C) 2606171800\nE) X"
            ),
            Err(NotamParseError::MissingReference { kind: 'R' })
        );
    }

    #[test]
    fn missing_items_are_reported() {
        assert_eq!(
            Notam::parse("A1234/26 NOTAMN\nE) NO QUALIFIER LINE"),
            Err(NotamParseError::MissingItem('Q'))
        );
        assert_eq!(
            Notam::parse(
                "A1234/26 NOTAMN\nQ) EDGG/QMRLC/IV/NBO/A/000/999/5002N00834E005\nB) 2606150600 C) 2606171800\nE) X"
            ),
            Err(NotamParseError::MissingItem('A'))
        );
        assert_eq!(
            Notam::parse(
                "A1234/26 NOTAMN\nQ) EDGG/QMRLC/IV/NBO/A/000/999/5002N00834E005\nA) EDDF B) 2606150600 C) 2606171800"
            ),
            Err(NotamParseError::MissingItem('E'))
        );
    }

    #[test]
    fn invalid_location_is_reported() {
        let text = "A1234/26 NOTAMN\n\
            Q) EDGG/QMRLC/IV/NBO/A/000/999/5002N00834E005\n\
            A) EDD B) 2606150600 C) 2606171800\n\
            E) X";
        assert_eq!(
            Notam::parse(text),
            Err(NotamParseError::InvalidLocation("EDD".to_owned()))
        );
    }
}
