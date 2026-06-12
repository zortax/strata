//! Fixture-backed [`NotamProvider`]: a hand-built corpus of realistic
//! German NOTAMs, embedded at compile time from
//! `tests/fixtures/autorouter/*.txt`.
//!
//! **Unit tests only** — compiled under `cfg(test)` / the `test-support`
//! feature, never part of a runtime build (the app uses
//! [`super::AutorouterClient`] or, without credentials, no NOTAM provider
//! at all). Query semantics mirror the live API: matching is on item A,
//! plus the validity-window overlap the API applies server-side via
//! `startvalidity`/`endvalidity`.

use async_trait::async_trait;

use crate::Error;
use crate::domain::{IcaoCode, Notam, NotamParseError};
use crate::providers::{NotamProvider, TimeWindow};

/// The embedded corpus: aerodrome NOTAMs for EDDF/EDDM/EDDS/EDDN
/// (closures, works, unserviceable aids, fuel, a replacement and a
/// cancellation), an ED-R activation, a crane and a permanent mast,
/// FIR-wide GPS jamming, navigation warnings — with definite, `EST` and
/// `PERM` validity ends.
pub(super) const CORPUS: &[&str] = &[
    include_str!("../../../tests/fixtures/autorouter/eddf-rwy-closure.txt"),
    include_str!("../../../tests/fixtures/autorouter/eddf-twy-work.txt"),
    include_str!("../../../tests/fixtures/autorouter/eddf-apron-replacement.txt"),
    include_str!("../../../tests/fixtures/autorouter/eddf-twy-cancellation.txt"),
    include_str!("../../../tests/fixtures/autorouter/eddf-ils-unserviceable.txt"),
    include_str!("../../../tests/fixtures/autorouter/eddf-twr-freq-change.txt"),
    include_str!("../../../tests/fixtures/autorouter/eddm-papi-unserviceable.txt"),
    include_str!("../../../tests/fixtures/autorouter/eddm-crane.txt"),
    include_str!("../../../tests/fixtures/autorouter/eddm-mast-permanent.txt"),
    include_str!("../../../tests/fixtures/autorouter/eddm-vordme-unserviceable.txt"),
    include_str!("../../../tests/fixtures/autorouter/eddn-twy-closure.txt"),
    include_str!("../../../tests/fixtures/autorouter/edr-activation.txt"),
    include_str!("../../../tests/fixtures/autorouter/edgg-gps-jamming.txt"),
    include_str!("../../../tests/fixtures/autorouter/edmm-parachute-jumping.txt"),
    include_str!("../../../tests/fixtures/autorouter/edgg-glider-activity.txt"),
    include_str!("../../../tests/fixtures/autorouter/edds-avgas-unavailable.txt"),
    include_str!("../../../tests/fixtures/autorouter/edds-bird-hazard.txt"),
];

/// Serves NOTAMs from an in-memory corpus.
pub struct FixtureNotamProvider {
    notams: Vec<Notam>,
}

impl FixtureNotamProvider {
    /// The built-in German corpus, verbatim (authored validity dates).
    pub fn builtin() -> Self {
        // Infallible: the embedded corpus is compile-time constant and
        // `tests::every_fixture_parses` guards each file.
        Self::from_texts(CORPUS.iter().copied())
            .expect("embedded NOTAM corpus parses (guarded by tests)")
    }

    /// A provider over already-decoded NOTAMs (synthetic test corpora).
    pub fn new(notams: Vec<Notam>) -> Self {
        Self { notams }
    }

    /// Parses each text as one NOTAM in ICAO transmission format.
    pub fn from_texts<'a>(
        texts: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, NotamParseError> {
        let notams = texts
            .into_iter()
            .map(Notam::parse)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { notams })
    }

    /// The full corpus, unfiltered.
    pub fn notams(&self) -> &[Notam] {
        &self.notams
    }

    fn filtered(&self, window: TimeWindow, matches: impl Fn(&Notam) -> bool) -> Vec<Notam> {
        self.notams
            .iter()
            .filter(|notam| notam.validity.overlaps(window.from, window.to))
            .filter(|notam| matches(notam))
            .cloned()
            .collect()
    }
}

#[async_trait]
impl NotamProvider for FixtureNotamProvider {
    async fn notams_by_locations(
        &self,
        locations: &[IcaoCode],
        window: TimeWindow,
    ) -> Result<Vec<Notam>, Error> {
        Ok(self.filtered(window, |notam| {
            notam
                .locations
                .iter()
                .any(|location| locations.contains(location))
        }))
    }

    async fn notams_by_fir(&self, fir: &IcaoCode, window: TimeWindow) -> Result<Vec<Notam>, Error> {
        // Mirror the live API: FIR-wide NOTAMs carry the FIR as item A.
        Ok(self.filtered(window, |notam| notam.locations.contains(fir)))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use chrono::{DateTime, NaiveDate, Utc};

    use super::*;
    use crate::domain::VerticalReference;
    use crate::domain::notam::{NotamEnd, NotamKind, QCondition, QSubject};

    fn icao(code: &str) -> IcaoCode {
        IcaoCode::new(code).expect("valid test ICAO code")
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(y, mo, d)
            .and_then(|date| date.and_hms_opt(h, mi, 0))
            .expect("valid test datetime")
            .and_utc()
    }

    /// 2026-06-15 00:00 .. 2026-06-17 00:00 — the corpus dates straddle it.
    fn window() -> TimeWindow {
        TimeWindow {
            from: utc(2026, 6, 15, 0, 0),
            to: utc(2026, 6, 17, 0, 0),
        }
    }

    fn corpus_notam(provider: &FixtureNotamProvider, id: &str) -> Notam {
        provider
            .notams()
            .iter()
            .find(|notam| notam.id.to_string() == id)
            .unwrap_or_else(|| panic!("corpus contains {id}"))
            .clone()
    }

    fn ids(notams: &[Notam]) -> HashSet<String> {
        notams.iter().map(|notam| notam.id.to_string()).collect()
    }

    #[test]
    fn every_fixture_parses() {
        for (index, text) in CORPUS.iter().enumerate() {
            Notam::parse(text).unwrap_or_else(|e| {
                panic!("corpus fixture #{index} failed to parse: {e}\n---\n{text}")
            });
        }
    }

    #[test]
    fn corpus_is_complete_with_distinct_ids() {
        let provider = FixtureNotamProvider::builtin();
        assert_eq!(provider.notams().len(), 17);
        assert_eq!(ids(provider.notams()).len(), 17);
    }

    #[test]
    fn decodes_eddn_taxiway_closure_fixture() {
        let provider = FixtureNotamProvider::builtin();
        let notam = corpus_notam(&provider, "C0788/26");
        assert_eq!(notam.fir().as_str(), "EDMM");
        assert_eq!(notam.locations, vec![icao("EDDN")]);
        assert!(notam.q.scope.aerodrome);
        assert_eq!(notam.validity.from, utc(2026, 6, 15, 5, 0));
        assert_eq!(notam.validity.until, NotamEnd::At(utc(2026, 6, 17, 20, 0)));
        assert!(notam.text.contains("TWY D CLSD"));
    }

    #[test]
    fn decodes_runway_closure_fixture() {
        let provider = FixtureNotamProvider::builtin();
        let notam = corpus_notam(&provider, "A1234/26");
        assert_eq!(notam.kind, NotamKind::New);
        assert_eq!(notam.fir().as_str(), "EDGG");
        assert_eq!(notam.q.code.subject, QSubject::Runway);
        assert_eq!(notam.q.code.condition, QCondition::Closed);
        assert!(notam.q.traffic.ifr && notam.q.traffic.vfr);
        assert!(notam.q.scope.aerodrome);
        assert_eq!(notam.q.radius_nm, 5);
        assert_eq!(notam.locations, vec![icao("EDDF")]);
        assert_eq!(notam.validity.from, utc(2026, 6, 15, 6, 0));
        assert_eq!(notam.validity.until, NotamEnd::At(utc(2026, 6, 17, 18, 0)));
        assert_eq!(notam.text, "RWY 07C/25C CLSD DUE TO RWY MAINT");
    }

    #[test]
    fn decodes_parenthesized_edr_activation_fixture() {
        let provider = FixtureNotamProvider::builtin();
        let notam = corpus_notam(&provider, "D0452/26");
        assert_eq!(notam.q.code.subject, QSubject::RestrictedArea);
        assert_eq!(notam.q.code.condition, QCondition::Activated);
        assert!(notam.q.scope.nav_warning);
        assert_eq!(notam.q.upper.reference, VerticalReference::Fl(100));
        assert_eq!(notam.q.radius_nm, 10);
        assert_eq!(notam.locations, vec![icao("EDMM")]);
        assert_eq!(notam.schedule.as_deref(), Some("16 17 18 0700-1500"));
        assert_eq!(notam.items.f.as_deref(), Some("GND"));
        assert_eq!(notam.items.g.as_deref(), Some("FL100"));
        assert_eq!(notam.text, "ED-R 136A GRAFENWOEHR ACT");
    }

    #[test]
    fn decodes_gps_jamming_fixture() {
        let provider = FixtureNotamProvider::builtin();
        let notam = corpus_notam(&provider, "E0231/26");
        assert_eq!(notam.q.code.subject, QSubject::GnssAreaWide);
        assert_eq!(notam.q.code.condition, QCondition::NotAvailable);
        assert!(notam.q.scope.enroute && !notam.q.scope.aerodrome);
        assert_eq!(notam.q.radius_nm, 150);
        assert_eq!(notam.q.upper.reference, VerticalReference::Unl);
        assert_eq!(
            notam.validity.until,
            NotamEnd::Estimated(utc(2026, 6, 21, 23, 59))
        );
        assert!(notam.text.contains("POSSIBLE DEGRADATION"));
    }

    #[test]
    fn decodes_crane_and_permanent_mast_fixtures() {
        let provider = FixtureNotamProvider::builtin();

        let crane = corpus_notam(&provider, "B0815/26");
        assert_eq!(crane.q.code.subject, QSubject::Obstacle);
        assert_eq!(crane.q.code.condition, QCondition::Erected);
        assert!(crane.q.scope.aerodrome && crane.q.scope.enroute);
        assert_eq!(crane.q.upper.reference, VerticalReference::Fl(21));
        assert_eq!(
            crane.validity.until,
            NotamEnd::Estimated(utc(2026, 9, 30, 12, 0))
        );
        assert_eq!(crane.items.g.as_deref(), Some("1916FT AMSL"));

        let mast = corpus_notam(&provider, "B0820/26");
        assert_eq!(mast.validity.until, NotamEnd::Permanent);
        assert!(mast.validity.active_at(utc(2030, 1, 1, 12, 0)));
    }

    #[test]
    fn decodes_replacement_and_cancellation_fixtures() {
        let provider = FixtureNotamProvider::builtin();

        let replacement = corpus_notam(&provider, "A1300/26");
        assert_eq!(
            replacement.kind,
            NotamKind::Replacement {
                replaces: "A1198/26".parse().expect("valid id")
            }
        );

        let cancellation = corpus_notam(&provider, "A1310/26");
        assert_eq!(
            cancellation.kind,
            NotamKind::Cancellation {
                cancels: "A1241/26".parse().expect("valid id")
            }
        );
        assert_eq!(cancellation.validity.until, NotamEnd::Permanent);
    }

    #[test]
    fn decodes_plain_language_condition_fixture() {
        let provider = FixtureNotamProvider::builtin();
        let notam = corpus_notam(&provider, "C0540/26");
        assert_eq!(notam.q.code.subject, QSubject::Aerodrome);
        assert_eq!(notam.q.code.condition, QCondition::PlainLanguage);
    }

    #[tokio::test]
    async fn by_locations_filters_on_item_a_and_window() {
        let provider = FixtureNotamProvider::builtin();
        let notams = provider
            .notams_by_locations(&[icao("EDDF")], window())
            .await
            .expect("fixture provider never fails");
        // In window: runway closure, taxiway work (EST into July),
        // apron replacement, the cancellation (from 16.06, no end).
        // Out: ILS maintenance (ended 14.06), freq change (starts 20.06).
        assert_eq!(
            ids(&notams),
            HashSet::from([
                "A1234/26".to_owned(),
                "A1241/26".to_owned(),
                "A1300/26".to_owned(),
                "A1310/26".to_owned(),
            ])
        );
    }

    #[tokio::test]
    async fn by_locations_merges_multiple_locations() {
        let provider = FixtureNotamProvider::builtin();
        let eddm = provider
            .notams_by_locations(&[icao("EDDM")], window())
            .await
            .expect("fixture provider never fails");
        // PAPI, crane, permanent mast; the VOR/DME outage starts 17.06
        // 06:00, after the window closes.
        assert_eq!(
            ids(&eddm),
            HashSet::from([
                "B0612/26".to_owned(),
                "B0815/26".to_owned(),
                "B0820/26".to_owned(),
            ])
        );

        let both = provider
            .notams_by_locations(&[icao("EDDF"), icao("EDDM")], window())
            .await
            .expect("fixture provider never fails");
        assert_eq!(both.len(), 7);
    }

    #[tokio::test]
    async fn by_fir_returns_fir_wide_notams_only() {
        let provider = FixtureNotamProvider::builtin();

        let edmm = provider
            .notams_by_fir(&icao("EDMM"), window())
            .await
            .expect("fixture provider never fails");
        // ED-R activation + parachute jumping are filed against EDMM;
        // the EDDM aerodrome NOTAMs are not.
        assert_eq!(
            ids(&edmm),
            HashSet::from(["D0452/26".to_owned(), "W0871/26".to_owned()])
        );

        let edgg = provider
            .notams_by_fir(&icao("EDGG"), window())
            .await
            .expect("fixture provider never fails");
        assert_eq!(
            ids(&edgg),
            HashSet::from(["E0231/26".to_owned(), "W0903/26".to_owned()])
        );
    }

    #[tokio::test]
    async fn window_outside_all_validity_yields_nothing() {
        let provider = FixtureNotamProvider::builtin();
        let window = TimeWindow {
            from: utc(2025, 1, 1, 0, 0),
            to: utc(2025, 1, 2, 0, 0),
        };
        let notams = provider
            .notams_by_locations(&[icao("EDDF"), icao("EDDM"), icao("EDDS")], window)
            .await
            .expect("fixture provider never fails");
        assert!(notams.is_empty());
    }

    #[tokio::test]
    async fn far_future_window_still_matches_permanent_notams() {
        let provider = FixtureNotamProvider::builtin();
        let window = TimeWindow {
            from: utc(2031, 1, 1, 0, 0),
            to: utc(2031, 1, 2, 0, 0),
        };
        let notams = provider
            .notams_by_locations(&[icao("EDDF"), icao("EDDM")], window)
            .await
            .expect("fixture provider never fails");
        // The permanent mast, the permanent freq change, and the
        // cancellation (no end) remain.
        assert_eq!(
            ids(&notams),
            HashSet::from([
                "A1310/26".to_owned(),
                "A1322/26".to_owned(),
                "B0820/26".to_owned(),
            ])
        );
    }
}
