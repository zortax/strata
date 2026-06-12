//! Frequency suggestion for nav-log rows.
//!
//! Documented heuristic ("nearest relevant", design §3.3):
//!
//! 1. **Airport rows** — a waypoint that *is* a named airport present in
//!    the prefetched airport list — suggest that airport's own station,
//!    preferring the kinds a VFR arrival/departure actually calls:
//!    `Tower > AFIS > Radio > Information > CTAF > Unicom > Multicom >
//!    Approach`. Among equal kinds the `primary` frequency wins.
//! 2. **En-route rows** (free points, navaids, reporting points, TOC/TOD)
//!    — walk the airports by great-circle distance from the row's
//!    position and take the first that offers an en-route service,
//!    preferring `FIS > Information > Tower > AFIS > Radio > CTAF` (FIS
//!    sector frequencies ride on airport records in the openAIP model,
//!    e.g. "LANGEN INFORMATION").
//!
//! Returns `None` when the airport list offers nothing relevant — the
//! field is a suggestion, never load-bearing.

use strata_data::domain::{Airport, Frequency, FrequencyKind, LatLon};

use crate::route::great_circle_distance;

const AIRPORT_PRIORITY: &[FrequencyKind] = &[
    FrequencyKind::Tower,
    FrequencyKind::Afis,
    FrequencyKind::Radio,
    FrequencyKind::Information,
    FrequencyKind::Ctaf,
    FrequencyKind::Unicom,
    FrequencyKind::Multicom,
    FrequencyKind::Approach,
];

const ENROUTE_PRIORITY: &[FrequencyKind] = &[
    FrequencyKind::Fis,
    FrequencyKind::Information,
    FrequencyKind::Tower,
    FrequencyKind::Afis,
    FrequencyKind::Radio,
    FrequencyKind::Ctaf,
];

/// Suggests a frequency for a row at `position`; `airport_ident` is the
/// row's waypoint ICAO id when it is a named airport.
pub(crate) fn suggest(
    airports: &[Airport],
    position: LatLon,
    airport_ident: Option<&str>,
) -> Option<Frequency> {
    if let Some(ident) = airport_ident
        && let Some(airport) = airports
            .iter()
            .find(|a| a.ident.as_ref().is_some_and(|i| i.as_str() == ident))
        && let Some(frequency) = pick(airport, AIRPORT_PRIORITY)
    {
        return Some(frequency);
    }

    let mut by_distance: Vec<&Airport> = airports.iter().collect();
    by_distance.sort_by(|a, b| {
        great_circle_distance(a.position, position)
            .0
            .total_cmp(&great_circle_distance(b.position, position).0)
    });
    by_distance
        .iter()
        .find_map(|airport| pick(airport, ENROUTE_PRIORITY))
}

/// Best frequency of `airport` by kind priority; `primary` breaks ties
/// within a kind.
fn pick(airport: &Airport, priority: &[FrequencyKind]) -> Option<Frequency> {
    for &kind in priority {
        let mut of_kind = airport.frequencies.iter().filter(|f| f.kind == kind);
        let Some(first) = of_kind.next() else {
            continue;
        };
        let best = if first.primary {
            first
        } else {
            of_kind.find(|f| f.primary).unwrap_or(first)
        };
        return Some(best.clone());
    }
    None
}
