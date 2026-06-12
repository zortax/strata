//! Coverage countries (ISO 3166-1 alpha-2) with curated **mainland**
//! bounding boxes — the unit of data ingestion. This module replaced the
//! old single-variant `Region` type (`Region::Germany` → `Country::DE`);
//! the `Region` name is gone, everything regional takes a `Country` or a
//! `&[Country]` set now.
//!
//! Country selection scopes **ingestion only** (what gets downloaded and
//! kept current); rendering stays viewport-driven over whatever the store
//! holds.
//!
//! ## Bounding-box curation
//!
//! Boxes are hand-curated to the European mainland (plus politically
//! integral nearby islands) so that terrain/basemap ingestion never chases
//! overseas territories. The per-country notes below document every
//! non-obvious choice. Boxes of neighbouring countries overlap — that is
//! fine everywhere they are used (tile ingestion is idempotent, DEM/tile
//! id sets are deduplicated or merged).
//!
//! Gridded weather (DWD ICON-D2) covers roughly 43–58 °N / −4–20 °E;
//! countries outside that window simply have no gridded data (honest
//! absence — switching to ICON-EU for full coverage is future work).

use std::fmt;

use serde::{Deserialize, Serialize};

use super::geo::BoundingBox;

/// Defines [`Country`] plus its constant tables in one place so the code,
/// name and bbox of an entry can never drift apart.
macro_rules! countries {
    ($( $(#[doc = $doc:expr])* $code:ident, $name:literal, ($w:expr, $s:expr, $e:expr, $n:expr); )+) => {
        /// A coverage country, named by its ISO 3166-1 alpha-2 code (the
        /// code openAIP's `country=` parameter takes).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum Country {
            $( $(#[doc = $doc])* $code, )+
        }

        impl Country {
            /// Every supported country, in a stable (alphabetical-ish,
            /// declaration) order.
            pub const ALL: [Country; countries!(@count $($code)+)] = [ $(Country::$code,)+ ];

            /// ISO 3166-1 alpha-2 code, uppercase — also the openAIP
            /// `country=` query value.
            pub fn code(self) -> &'static str {
                match self { $( Self::$code => stringify!($code), )+ }
            }

            /// English short name.
            pub fn name(self) -> &'static str {
                match self { $( Self::$code => $name, )+ }
            }

            /// Curated mainland data-coverage bounding box (see the module
            /// docs for the curation rules).
            pub fn bounding_box(self) -> BoundingBox {
                match self { $( Self::$code => BoundingBox::new_unchecked($w, $s, $e, $n), )+ }
            }
        }
    };
    (@count) => { 0 };
    (@count $head:ident $($tail:ident)*) => { 1 + countries!(@count $($tail)*) };
}

countries! {
    /// Germany. Box unchanged from the original `Region::Germany`
    /// (spec: lat 47..55.2, lon 5.5..15.5) — existing stores' coverage
    /// expectations (e.g. the 25×20 elevation tile grid) depend on it.
    DE, "Germany", (5.5, 47.0, 15.5, 55.2);
    /// Austria.
    AT, "Austria", (9.5, 46.3, 17.2, 49.1);
    /// Switzerland (incl. Liechtenstein, which has no own openAIP data of
    /// its own scale and sits inside the box anyway).
    CH, "Switzerland", (5.9, 45.8, 10.6, 47.9);
    /// Metropolitan France only, **including Corsica** (an integral
    /// region) but excluding all overseas departments/territories
    /// (Guadeloupe, Martinique, Guyane, Réunion, Mayotte, Polynesia, …).
    FR, "France", (-5.2, 41.2, 9.7, 51.2);
    /// Italy incl. Sicily, Sardinia and the Pelagie islands (Lampedusa,
    /// 35.5 °N — hence the southern edge).
    IT, "Italy", (6.6, 35.4, 18.6, 47.2);
    /// Peninsular Spain + Balearics. Excludes the Canary Islands
    /// (28 °N, −18..−13 °E) and the North-African exclaves Ceuta/Melilla
    /// (south edge 36.0 keeps Tarifa, mainland Europe's southernmost
    /// point, and drops Ceuta at 35.9).
    ES, "Spain", (-9.4, 36.0, 4.4, 43.9);
    /// Mainland Portugal only — excludes Madeira (32.7 °N, −17 °E) and
    /// the Azores (−31..−25 °E).
    PT, "Portugal", (-9.6, 36.9, -6.2, 42.2);
    /// Belgium.
    BE, "Belgium", (2.5, 49.5, 6.4, 51.6);
    /// European Netherlands only — excludes the Caribbean municipalities
    /// and constituent countries (Bonaire, Curaçao, Aruba, … at 12 °N).
    NL, "Netherlands", (3.3, 50.7, 7.3, 53.7);
    /// Luxembourg.
    LU, "Luxembourg", (5.7, 49.4, 6.6, 50.2);
    /// Metropolitan Denmark incl. Bornholm (15.2 °E) — excludes the
    /// Faroe Islands (62 °N, −7 °E) and Greenland.
    DK, "Denmark", (8.0, 54.5, 15.3, 57.8);
    /// Sweden.
    SE, "Sweden", (10.9, 55.3, 24.2, 69.1);
    /// Mainland Norway only — excludes Svalbard (74..81 °N), Jan Mayen
    /// and the southern dependencies. North edge 71.2 keeps Nordkapp.
    NO, "Norway", (4.5, 57.9, 31.2, 71.2);
    /// Finland incl. Åland.
    FI, "Finland", (19.0, 59.6, 31.6, 70.2);
    /// Poland.
    PL, "Poland", (14.1, 49.0, 24.2, 54.9);
    /// Czechia.
    CZ, "Czechia", (12.0, 48.5, 18.9, 51.1);
    /// Slovakia.
    SK, "Slovakia", (16.8, 47.7, 22.6, 49.7);
    /// Hungary.
    HU, "Hungary", (16.1, 45.7, 23.0, 48.6);
    /// Slovenia.
    SI, "Slovenia", (13.3, 45.4, 16.6, 46.9);
    /// Croatia incl. the Adriatic islands (Palagruža, 42.4 °N).
    HR, "Croatia", (13.4, 42.3, 19.5, 46.6);
    /// United Kingdom: Great Britain **and Northern Ireland** (west edge
    /// −8.2 covers Fermanagh), north to the Shetlands (60.9). Excludes
    /// the Crown dependencies (Channel Islands at 49.2..49.5 °N — south
    /// edge 49.8 keeps the Isles of Scilly at 49.86 but drops Guernsey)
    /// and all overseas territories (Gibraltar, Falklands, …).
    GB, "United Kingdom", (-8.2, 49.8, 1.8, 60.9);
    /// Ireland.
    IE, "Ireland", (-10.7, 51.3, -5.9, 55.5);
    /// Greece incl. Crete/Gavdos (34.8 °N) and the whole Dodecanese out
    /// to Kastellorizo (29.6 °E) — all integral territory, so the box
    /// runs wider than the mainland.
    GR, "Greece", (19.3, 34.7, 29.7, 41.8);
    /// Romania.
    RO, "Romania", (20.2, 43.6, 29.8, 48.3);
    /// Bulgaria.
    BG, "Bulgaria", (22.3, 41.2, 28.7, 44.3);
    /// Estonia incl. the Baltic islands (Vaindloo, 59.8 °N).
    EE, "Estonia", (21.7, 57.5, 28.2, 59.9);
    /// Latvia.
    LV, "Latvia", (20.9, 55.6, 28.3, 58.1);
    /// Lithuania.
    LT, "Lithuania", (20.9, 53.9, 26.9, 56.5);
    /// Iceland.
    IS, "Iceland", (-24.6, 63.2, -13.4, 66.6);
    /// Albania.
    AL, "Albania", (19.2, 39.6, 21.1, 42.7);
    /// Serbia. The box covers the territory it administers per ISO 3166
    /// usage in openAIP; Kosovo (XK) is not a separate entry here.
    RS, "Serbia", (18.8, 42.2, 23.1, 46.2);
    /// Bosnia and Herzegovina.
    BA, "Bosnia and Herzegovina", (15.7, 42.5, 19.7, 45.3);
    /// North Macedonia.
    MK, "North Macedonia", (20.4, 40.8, 23.1, 42.4);
    /// Montenegro.
    ME, "Montenegro", (18.4, 41.8, 20.4, 43.6);
    /// Malta.
    MT, "Malta", (14.1, 35.8, 14.6, 36.1);
    /// Cyprus (the whole island — airspace data does not follow the
    /// political division).
    CY, "Cyprus", (32.2, 34.5, 34.7, 35.8);
}

/// Largest latitude span (degrees) of one aviationweather.gov bbox
/// request — see [`weather_bboxes`]. Sized to stay in the measured
/// thinning-free range while still fitting every single country's box
/// (Norway is the widest at 13.3° lat / 26.7° lon).
const MAX_WEATHER_LAT_SPAN: f64 = 14.0;
/// Largest longitude span (degrees) of one aviationweather.gov bbox
/// request — see [`weather_bboxes`] and [`MAX_WEATHER_LAT_SPAN`].
const MAX_WEATHER_LON_SPAN: f64 = 28.0;

impl Country {
    /// The default enabled set: Germany only.
    pub const DEFAULT_ENABLED: &[Country] = &[Country::DE];

    /// Parses an ISO alpha-2 code, case-insensitively.
    pub fn from_code(code: &str) -> Option<Country> {
        Self::ALL
            .into_iter()
            .find(|c| c.code().eq_ignore_ascii_case(code))
    }

    /// Smallest box containing every country's box; `None` for an empty
    /// set. Beware: for far-apart sets this spans most of Europe — weather
    /// fetches must use [`weather_bboxes`] instead.
    pub fn union_bbox(countries: &[Country]) -> Option<BoundingBox> {
        let mut iter = countries.iter().map(|c| c.bounding_box());
        let first = iter.next()?;
        Some(iter.fold(first, |acc, b| {
            BoundingBox::new_unchecked(
                acc.west().min(b.west()),
                acc.south().min(b.south()),
                acc.east().max(b.east()),
                acc.north().max(b.north()),
            )
        }))
    }
}

impl fmt::Display for Country {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl std::str::FromStr for Country {
    type Err = UnknownCountry;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Country::from_code(s).ok_or_else(|| UnknownCountry(s.to_owned()))
    }
}

/// Error of [`Country::from_str`]: not a supported alpha-2 code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "unsupported country code {0:?} (expected an ISO 3166-1 alpha-2 code of a supported European country, e.g. DE)"
)]
pub struct UnknownCountry(pub String);

/// Bounding boxes for live METAR/TAF station fetches over the enabled
/// countries.
///
/// aviationweather.gov accepts arbitrarily large `bbox` values but
/// silently **thins the stations** it returns once the box grows past
/// roughly 15–25 degrees per axis (measured 2026-06: a 12°×25° central
/// Europe box returned 195 METARs while a strictly larger 20°×35° box
/// returned 183 and a 38°×59° all-Europe box only 104). One union box is
/// therefore wrong for far-apart country sets; this helper greedily
/// clusters the countries' boxes into the fewest unions that stay within
/// [`MAX_WEATHER_LAT_SPAN`] × [`MAX_WEATHER_LON_SPAN`] — callers issue one
/// request per returned box and merge by station id (duplicates from
/// overlapping boxes collapse).
pub fn weather_bboxes(countries: &[Country]) -> Vec<BoundingBox> {
    let mut remaining: Vec<BoundingBox> = {
        // Dedupe, keeping a stable order.
        let mut seen = Vec::new();
        for c in countries {
            if !seen.contains(c) {
                seen.push(*c);
            }
        }
        seen.iter().map(|c| c.bounding_box()).collect()
    };
    let mut clusters = Vec::new();
    while !remaining.is_empty() {
        let mut cluster = remaining.remove(0);
        // Keep absorbing whichever remaining box still fits; restart the
        // scan after each merge so order matters less.
        loop {
            let fit = remaining.iter().position(|b| {
                let u = union2(cluster, *b);
                u.north() - u.south() <= MAX_WEATHER_LAT_SPAN
                    && u.east() - u.west() <= MAX_WEATHER_LON_SPAN
            });
            match fit {
                Some(index) => cluster = union2(cluster, remaining.remove(index)),
                None => break,
            }
        }
        clusters.push(cluster);
    }
    clusters
}

fn union2(a: BoundingBox, b: BoundingBox) -> BoundingBox {
    BoundingBox::new_unchecked(
        a.west().min(b.west()),
        a.south().min(b.south()),
        a.east().max(b.east()),
        a.north().max(b.north()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::geo::LatLon;

    fn ll(lat: f64, lon: f64) -> LatLon {
        LatLon::new(lat, lon).unwrap()
    }

    #[test]
    fn codes_are_unique_uppercase_alpha2() {
        let mut codes: Vec<&str> = Country::ALL.iter().map(|c| c.code()).collect();
        for code in &codes {
            assert_eq!(code.len(), 2, "{code}");
            assert!(
                code.chars().all(|c| c.is_ascii_uppercase()),
                "{code} must be uppercase ASCII"
            );
        }
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), Country::ALL.len(), "duplicate country code");
    }

    #[test]
    fn names_are_unique_and_nonempty() {
        let mut names: Vec<&str> = Country::ALL.iter().map(|c| c.name()).collect();
        assert!(names.iter().all(|n| !n.is_empty()));
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Country::ALL.len(), "duplicate country name");
    }

    #[test]
    fn bboxes_are_plausible_european_mainland_boxes() {
        for c in Country::ALL {
            let b = c.bounding_box();
            assert!(b.west() < b.east(), "{c}: west < east");
            assert!(b.south() < b.north(), "{c}: south < north");
            // Europe-ish window — catches transposed coordinates and
            // accidentally included overseas territory.
            assert!(b.west() >= -25.0 && b.east() <= 35.0, "{c}: lon range");
            assert!(b.south() >= 34.0 && b.north() <= 72.0, "{c}: lat range");
            // No country's mainland needs more than this.
            assert!(b.east() - b.west() <= 27.0, "{c}: lon span");
            assert!(b.north() - b.south() <= 14.0, "{c}: lat span");
        }
    }

    #[test]
    fn germany_box_is_pinned_to_the_original_region() {
        // Existing stores' coverage expectations (25×20 elevation tile
        // grid) depend on this exact box — never change it casually.
        let b = Country::DE.bounding_box();
        assert_eq!(
            (b.west(), b.south(), b.east(), b.north()),
            (5.5, 47.0, 15.5, 55.2)
        );
        assert!(b.contains(ll(52.52, 13.4))); // Berlin
        assert!(b.contains(ll(48.14, 11.58))); // Munich
        assert!(!b.contains(ll(48.86, 2.35))); // Paris
    }

    /// The hand-curated overseas exclusions, pinned per tricky country.
    #[test]
    fn overseas_territories_are_excluded() {
        let fr = Country::FR.bounding_box();
        assert!(fr.contains(ll(48.86, 2.35)), "Paris");
        assert!(fr.contains(ll(42.0, 9.0)), "Corsica is metropolitan");
        assert!(!fr.contains(ll(-21.1, 55.5)), "Réunion");
        assert!(!fr.contains(ll(16.25, -61.55)), "Guadeloupe");
        assert!(!fr.contains(ll(4.9, -52.3)), "Guyane (Cayenne)");

        let es = Country::ES.bounding_box();
        assert!(es.contains(ll(40.4, -3.7)), "Madrid");
        assert!(es.contains(ll(39.6, 2.9)), "Mallorca");
        assert!(es.contains(ll(36.01, -5.6)), "Tarifa (mainland south tip)");
        assert!(!es.contains(ll(28.1, -15.4)), "Canary Islands");
        assert!(!es.contains(ll(35.89, -5.32)), "Ceuta");

        let pt = Country::PT.bounding_box();
        assert!(pt.contains(ll(38.7, -9.1)), "Lisbon");
        assert!(!pt.contains(ll(32.7, -16.9)), "Madeira");
        assert!(!pt.contains(ll(37.7, -25.7)), "Azores");

        let nl = Country::NL.bounding_box();
        assert!(nl.contains(ll(52.37, 4.9)), "Amsterdam");
        assert!(!nl.contains(ll(12.1, -68.3)), "Bonaire");

        let dk = Country::DK.bounding_box();
        assert!(dk.contains(ll(55.68, 12.57)), "Copenhagen");
        assert!(dk.contains(ll(55.1, 14.9)), "Bornholm");
        assert!(!dk.contains(ll(62.0, -6.8)), "Faroe Islands");
        assert!(!dk.contains(ll(64.2, -51.7)), "Greenland (Nuuk)");

        let no = Country::NO.bounding_box();
        assert!(no.contains(ll(59.9, 10.75)), "Oslo");
        assert!(no.contains(ll(71.0, 25.8)), "Nordkapp area");
        assert!(!no.contains(ll(78.2, 15.6)), "Svalbard (Longyearbyen)");

        let gb = Country::GB.bounding_box();
        assert!(gb.contains(ll(51.5, -0.13)), "London");
        assert!(gb.contains(ll(54.6, -5.93)), "Belfast (NI included)");
        assert!(gb.contains(ll(60.15, -1.15)), "Shetland (Lerwick)");
        assert!(gb.contains(ll(49.86, -6.3)), "Isles of Scilly");
        assert!(
            !gb.contains(ll(49.45, -2.58)),
            "Guernsey (Crown dependency)"
        );
        assert!(!gb.contains(ll(36.14, -5.35)), "Gibraltar");
    }

    #[test]
    fn every_capital_is_inside_its_box() {
        // (country, capital lat, lon)
        let capitals = [
            (Country::DE, 52.52, 13.40),
            (Country::AT, 48.21, 16.37),
            (Country::CH, 46.95, 7.45),
            (Country::FR, 48.86, 2.35),
            (Country::IT, 41.90, 12.50),
            (Country::ES, 40.42, -3.70),
            (Country::PT, 38.72, -9.14),
            (Country::BE, 50.85, 4.35),
            (Country::NL, 52.37, 4.90),
            (Country::LU, 49.61, 6.13),
            (Country::DK, 55.68, 12.57),
            (Country::SE, 59.33, 18.07),
            (Country::NO, 59.91, 10.75),
            (Country::FI, 60.17, 24.94),
            (Country::PL, 52.23, 21.01),
            (Country::CZ, 50.08, 14.44),
            (Country::SK, 48.15, 17.11),
            (Country::HU, 47.50, 19.04),
            (Country::SI, 46.06, 14.51),
            (Country::HR, 45.81, 15.98),
            (Country::GB, 51.51, -0.13),
            (Country::IE, 53.35, -6.26),
            (Country::GR, 37.98, 23.73),
            (Country::RO, 44.43, 26.10),
            (Country::BG, 42.70, 23.32),
            (Country::EE, 59.44, 24.75),
            (Country::LV, 56.95, 24.11),
            (Country::LT, 54.69, 25.28),
            (Country::IS, 64.15, -21.94),
            (Country::AL, 41.33, 19.82),
            (Country::RS, 44.79, 20.45),
            (Country::BA, 43.86, 18.41),
            (Country::MK, 41.99, 21.43),
            (Country::ME, 42.44, 19.26),
            (Country::MT, 35.90, 14.51),
            (Country::CY, 35.17, 33.36),
        ];
        assert_eq!(
            capitals.len(),
            Country::ALL.len(),
            "one capital per country"
        );
        for (c, lat, lon) in capitals {
            assert!(c.bounding_box().contains(ll(lat, lon)), "{c} capital");
        }
    }

    #[test]
    fn from_code_roundtrips_case_insensitively() {
        for c in Country::ALL {
            assert_eq!(Country::from_code(c.code()), Some(c));
            assert_eq!(Country::from_code(&c.code().to_lowercase()), Some(c));
            assert_eq!(c.code().parse::<Country>(), Ok(c));
        }
        assert_eq!(Country::from_code("XX"), None);
        assert_eq!(Country::from_code(""), None);
        assert!("zz".parse::<Country>().is_err());
    }

    #[test]
    fn serde_uses_the_alpha2_code() {
        assert_eq!(serde_json::to_string(&Country::DE).unwrap(), "\"DE\"");
        assert_eq!(
            serde_json::from_str::<Country>("\"AT\"").unwrap(),
            Country::AT
        );
    }

    #[test]
    fn default_enabled_is_germany_only() {
        assert_eq!(Country::DEFAULT_ENABLED, &[Country::DE]);
    }

    #[test]
    fn union_bbox_spans_all_inputs() {
        assert_eq!(Country::union_bbox(&[]), None);
        let de = Country::DE.bounding_box();
        assert_eq!(Country::union_bbox(&[Country::DE]), Some(de));
        let u = Country::union_bbox(&[Country::DE, Country::AT]).unwrap();
        assert_eq!(u.west(), 5.5);
        assert_eq!(u.south(), 46.3);
        assert_eq!(u.east(), 17.2);
        assert_eq!(u.north(), 55.2);
    }

    #[test]
    fn weather_bboxes_keep_one_box_for_germany_and_neighbours() {
        // The default set must keep today's single-request behavior.
        assert_eq!(
            weather_bboxes(&[Country::DE]),
            vec![Country::DE.bounding_box()]
        );
        // DE+AT+CH unions to ~11.7° × 9.4° — still one request.
        let boxes = weather_bboxes(&[Country::DE, Country::AT, Country::CH]);
        assert_eq!(boxes.len(), 1);
        assert!(boxes[0].contains(ll(47.0, 16.0)), "covers Austria");
    }

    #[test]
    fn weather_bboxes_split_far_apart_sets() {
        // Iceland and Cyprus cannot share a sanely sized box.
        let boxes = weather_bboxes(&[Country::IS, Country::CY]);
        assert_eq!(boxes.len(), 2);
        // All of Europe stays within the measured thinning-free spans and
        // every country is covered by some box.
        let boxes = weather_bboxes(&Country::ALL);
        assert!(boxes.len() > 1);
        for b in &boxes {
            assert!(b.north() - b.south() <= MAX_WEATHER_LAT_SPAN + 1e-9);
            assert!(b.east() - b.west() <= MAX_WEATHER_LON_SPAN + 1e-9);
        }
        for c in Country::ALL {
            let cb = c.bounding_box();
            assert!(
                boxes.iter().any(|b| b.contains_bbox(&cb)),
                "{c} not covered by any weather box"
            );
        }
        // Duplicates collapse.
        assert_eq!(weather_bboxes(&[Country::DE, Country::DE]).len(), 1);
    }
}
