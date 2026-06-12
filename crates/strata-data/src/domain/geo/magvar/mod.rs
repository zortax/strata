//! Magnetic variation (declination) from the NOAA/BGS World Magnetic Model.
//!
//! Backed by the vendored, public-domain WMM2025 coefficient set
//! (`WMM2025.COF`, epoch 2025.0, released 2024-11-13). The model is
//! **valid from 2025-01-01 through 2029-12-31** (decimal years
//! 2025.0..=2030.0); dates outside that window are clamped to the nearest
//! edge with a `tracing::warn` — never an error, never a panic.
//!
//! Why vendored instead of a crate (evaluated 2026-06):
//! - `wmm` 0.2.3 (last release 2022-11) ships only WMM2020 coefficients,
//!   whose validity ended at 2025.0 — unusable for current dates.
//! - `world_magnetic_model` 0.4.0 is maintained and does carry WMM2025, but
//!   pulls `uom` + `time` (duplicating our chrono/newtype discipline) and an
//!   `askama` template-engine build-dependency — not "light".
//!
//! The evaluation below is the standard spherical-harmonic synthesis from
//! the WMM2025 Technical Report (NOAA NCEI; degree/order 12), verified in
//! the unit tests against NOAA's official test-value table and the 100-row
//! test corpus shipped inside `WMM2025COF.zip`.
//!
//! Declination is computed at height 0 km above the WGS84 ellipsoid; over
//! the GA altitude band the difference is far below the model's own ~0.5°
//! uncertainty. Declination is undefined at the geographic poles; this
//! module is used for the Germany region (see the `geo` module docs).

use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate};

use super::{Degrees, LatLon};

/// Maximum spherical-harmonic degree and order of the WMM.
const MAX_DEGREE: usize = 12;

/// Geomagnetic reference radius (km), fixed by the WMM definition.
const GEOMAGNETIC_RADIUS_KM: f64 = 6371.2;

/// WGS84 semi-major axis (km) and flattening.
const WGS84_A_KM: f64 = 6378.137;
const WGS84_F: f64 = 1.0 / 298.257_223_563;

/// Validity window of the vendored coefficient set, as decimal years.
const VALID_FROM: f64 = 2025.0;
const VALID_UNTIL: f64 = 2030.0;

/// The vendored WMM2025 coefficient file (public domain, NOAA NCEI).
const WMM2025_COF: &str = include_str!("WMM2025.COF");

/// Magnetic variation (declination) at `position` on `date`, in degrees,
/// **east-positive** (a declination of +3.5° means magnetic north lies 3.5°
/// east of true north; magnetic course = true course − declination).
///
/// Evaluated with the World Magnetic Model WMM2025 at the WGS84 ellipsoid
/// surface. The coefficient set is valid 2025-01-01 ..= 2029-12-31; dates
/// outside the window are clamped to the nearest edge (with a
/// `tracing::warn`), so the function never fails.
pub fn magvar(position: LatLon, date: NaiveDate) -> Degrees {
    let mut year = decimal_year(date);
    if !(VALID_FROM..=VALID_UNTIL).contains(&year) {
        tracing::warn!(
            %date,
            valid_from = VALID_FROM,
            valid_until = VALID_UNTIL,
            "date outside WMM2025 validity window; clamping to model edge"
        );
        year = year.clamp(VALID_FROM, VALID_UNTIL);
    }
    let field = MODEL.field_at(position.lat(), position.lon(), 0.0, year);
    Degrees(field.declination_deg())
}

/// `2026-07-01` → `2026.495…` (NOAA convention: elapsed fraction of the
/// calendar year, day resolution).
fn decimal_year(date: NaiveDate) -> f64 {
    let days_in_year = if date.leap_year() { 366.0 } else { 365.0 };
    f64::from(date.year()) + f64::from(date.ordinal0()) / days_in_year
}

static MODEL: LazyLock<WmmModel> = LazyLock::new(|| {
    // Infallible in practice: the file is embedded at compile time and its
    // shape is pinned by the `parses_vendored_coefficients` test.
    WmmModel::parse(WMM2025_COF).expect("vendored WMM2025.COF is well-formed")
});

/// Gauss coefficients `g`/`h` (nT) at the model epoch plus their secular
/// variation `dg`/`dh` (nT/year), indexed `[degree n][order m]`.
struct WmmModel {
    epoch: f64,
    g: [[f64; MAX_DEGREE + 1]; MAX_DEGREE + 1],
    h: [[f64; MAX_DEGREE + 1]; MAX_DEGREE + 1],
    dg: [[f64; MAX_DEGREE + 1]; MAX_DEGREE + 1],
    dh: [[f64; MAX_DEGREE + 1]; MAX_DEGREE + 1],
}

/// Magnetic field vector in the local geodetic frame, nT.
/// `x` north, `y` east, `z` down.
struct FieldVector {
    x: f64,
    y: f64,
    /// Production code only needs declination (`x`, `y`); the down component
    /// is kept for the NOAA fixture tests (inclination / total field).
    #[cfg_attr(not(test), allow(dead_code))]
    z: f64,
}

impl FieldVector {
    /// Declination in degrees, east-positive.
    fn declination_deg(&self) -> f64 {
        self.y.atan2(self.x).to_degrees()
    }
}

impl WmmModel {
    /// Parses the NOAA `.COF` format: a header line
    /// `epoch model-name release-date`, then one `n m g h dg dh` row per
    /// coefficient, terminated by filler lines of `9`s.
    fn parse(cof: &str) -> Result<Self, String> {
        let mut lines = cof.lines();
        let header = lines.next().ok_or("empty .COF file")?;
        let epoch: f64 = header
            .split_whitespace()
            .next()
            .ok_or("blank .COF header")?
            .parse()
            .map_err(|e| format!("bad .COF epoch: {e}"))?;

        let zero = [[0.0; MAX_DEGREE + 1]; MAX_DEGREE + 1];
        let mut model = WmmModel {
            epoch,
            g: zero,
            h: zero,
            dg: zero,
            dh: zero,
        };

        for line in lines {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.is_empty() || fields[0].starts_with("9999") {
                continue; // terminator / padding
            }
            let [n, m, g, h, dg, dh] = fields[..] else {
                return Err(format!("malformed .COF row: {line:?}"));
            };
            let n: usize = n.parse().map_err(|e| format!("bad degree: {e}"))?;
            let m: usize = m.parse().map_err(|e| format!("bad order: {e}"))?;
            if n == 0 || n > MAX_DEGREE || m > n {
                return Err(format!("coefficient indices out of range: n={n} m={m}"));
            }
            let parse = |v: &str| -> Result<f64, String> {
                v.parse().map_err(|e| format!("bad coefficient: {e}"))
            };
            model.g[n][m] = parse(g)?;
            model.h[n][m] = parse(h)?;
            model.dg[n][m] = parse(dg)?;
            model.dh[n][m] = parse(dh)?;
        }
        Ok(model)
    }

    /// Field vector at geodetic latitude/longitude (degrees), `height_km`
    /// above the WGS84 ellipsoid, at `decimal_year`.
    ///
    /// Standard WMM synthesis: time-adjust the Gauss coefficients, convert
    /// geodetic → geocentric spherical, sum the Schmidt semi-normalized
    /// spherical-harmonic series, rotate the result back into the geodetic
    /// frame (WMM2025 Technical Report, eqs. 4–13).
    fn field_at(&self, lat_deg: f64, lon_deg: f64, height_km: f64, decimal_year: f64) -> FieldVector {
        let dt = decimal_year - self.epoch;
        let lat = lat_deg.to_radians();
        let lon = lon_deg.to_radians();

        // Geodetic → geocentric spherical (radius r km, geocentric latitude).
        let e2 = WGS84_F * (2.0 - WGS84_F);
        let rc = WGS84_A_KM / (1.0 - e2 * lat.sin().powi(2)).sqrt();
        let p = (rc + height_km) * lat.cos(); // distance from rotation axis
        let zc = (rc * (1.0 - e2) + height_km) * lat.sin();
        let r = p.hypot(zc);
        let lat_c = (zc / r).asin(); // geocentric latitude

        let (s, c) = (lat_c.sin(), lat_c.cos());

        // Schmidt semi-normalized associated Legendre functions P[n][m] at
        // sin(lat_c) and their derivatives dP[n][m] with respect to lat_c.
        let mut pnm = [[0.0_f64; MAX_DEGREE + 1]; MAX_DEGREE + 1];
        let mut dpnm = [[0.0_f64; MAX_DEGREE + 1]; MAX_DEGREE + 1];
        pnm[0][0] = 1.0;
        for n in 1..=MAX_DEGREE {
            for m in 0..=n {
                if n == m {
                    // Diagonal: P(n,n) = sqrt((2n-1)/(2n)) * cos * P(n-1,n-1),
                    // with the n = 1 factor equal to 1.
                    let k = if n == 1 {
                        1.0
                    } else {
                        ((2 * n - 1) as f64 / (2 * n) as f64).sqrt()
                    };
                    pnm[n][n] = k * c * pnm[n - 1][n - 1];
                    dpnm[n][n] = k * (c * dpnm[n - 1][n - 1] - s * pnm[n - 1][n - 1]);
                } else {
                    // P(n,m) = ((2n-1) sin P(n-1,m) - K P(n-2,m)) / sqrt(n²-m²)
                    let norm = ((n * n - m * m) as f64).sqrt();
                    // The P(n-2,m) term vanishes for n-2 < m.
                    let (k, p2, dp2) = if n >= m + 2 {
                        let k = (((n - 1) * (n - 1) - m * m) as f64).sqrt();
                        (k, pnm[n - 2][m], dpnm[n - 2][m])
                    } else {
                        (0.0, 0.0, 0.0)
                    };
                    let a = (2 * n - 1) as f64;
                    pnm[n][m] = (a * s * pnm[n - 1][m] - k * p2) / norm;
                    dpnm[n][m] =
                        (a * (s * dpnm[n - 1][m] + c * pnm[n - 1][m]) - k * dp2) / norm;
                }
            }
        }

        // Harmonic synthesis in the geocentric frame.
        // x' north, y' east, z' down; (a/r)^(n+2) radial attenuation.
        let ar = GEOMAGNETIC_RADIUS_KM / r;
        let mut xp = 0.0;
        let mut yp = 0.0;
        let mut zp = 0.0;
        let mut arn = ar * ar; // becomes (a/r)^(n+2) inside the loop
        for n in 1..=MAX_DEGREE {
            arn *= ar;
            for m in 0..=n {
                let g = self.g[n][m] + dt * self.dg[n][m];
                let h = self.h[n][m] + dt * self.dh[n][m];
                let (sin_ml, cos_ml) = (m as f64 * lon).sin_cos();
                let gh_cos = g * cos_ml + h * sin_ml;
                xp -= arn * gh_cos * dpnm[n][m];
                yp += arn * m as f64 * (g * sin_ml - h * cos_ml) * pnm[n][m];
                zp -= arn * (n + 1) as f64 * gh_cos * pnm[n][m];
            }
        }
        // The east component carries a 1/cos(lat_c) factor (longitude
        // derivative); finite everywhere except the exact poles, where
        // declination is undefined anyway.
        yp /= c;

        // Rotate geocentric → geodetic frame (rotation by lat_c - lat
        // about the east axis).
        let psi = lat_c - lat;
        let (sin_psi, cos_psi) = psi.sin_cos();
        FieldVector {
            x: xp * cos_psi - zp * sin_psi,
            y: yp,
            z: xp * sin_psi + zp * cos_psi,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Official WMM2025 test-value table (12 rows, 2025.0/2027.5 at 0 and
    /// 100 km), as published by NOAA NCEI:
    /// <https://www.ncei.noaa.gov/sites/default/files/2025-02/WMM2025_TEST_VALUES.txt>
    /// Columns: date, height(km), lat, lon, X, Y, Z, H, F, Incl, Decl, …
    const OFFICIAL_TABLE: &str =
        include_str!("../../../../tests/fixtures/wmm/WMM2025_TEST_VALUES.txt");

    /// 100-row machine-precision test corpus shipped inside NOAA's
    /// `WMM2025COF.zip` (file `WMM2025_TestValues.txt`).
    /// Columns: year, alt(km), lat, lon, Decl, Incl, H, X, Y, Z, F, …
    const EXTENDED_CORPUS: &str =
        include_str!("../../../../tests/fixtures/wmm/WMM2025_TestValues_extended.txt");

    fn data_rows(fixture: &str) -> impl Iterator<Item = Vec<f64>> + '_ {
        fixture
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(|l| {
                l.split_whitespace()
                    .map(|v| v.parse().expect("numeric fixture field"))
                    .collect()
            })
    }

    #[test]
    fn parses_vendored_coefficients() {
        assert_eq!(MODEL.epoch, 2025.0);
        // Spot-check first and last coefficient rows of WMM2025.COF.
        assert_eq!(MODEL.g[1][0], -29351.8);
        assert_eq!(MODEL.dg[1][0], 12.0);
        assert_eq!(MODEL.h[2][2], -815.1);
        assert_eq!(MODEL.g[12][12], -0.7);
        assert_eq!(MODEL.dh[12][12], -0.1);
    }

    #[test]
    fn matches_official_noaa_test_values() {
        let mut rows = 0;
        for row in data_rows(OFFICIAL_TABLE) {
            let [date, height, lat, lon, x, y, z, h, f, incl, decl, ..] = row[..] else {
                panic!("short row in official table");
            };
            let field = MODEL.field_at(lat, lon, height, date);
            // Published precision: 0.1 nT for field components, 0.01° for
            // angles (+ a rounding half-step of slack).
            assert!((field.x - x).abs() < 0.05, "X at {lat},{lon}: {} vs {x}", field.x);
            assert!((field.y - y).abs() < 0.05, "Y at {lat},{lon}: {} vs {y}", field.y);
            assert!((field.z - z).abs() < 0.05, "Z at {lat},{lon}: {} vs {z}", field.z);
            let h_got = field.x.hypot(field.y);
            let f_got = h_got.hypot(field.z);
            assert!((h_got - h).abs() < 0.05, "H at {lat},{lon}: {h_got} vs {h}");
            assert!((f_got - f).abs() < 0.05, "F at {lat},{lon}: {f_got} vs {f}");
            let incl_got = field.z.atan2(h_got).to_degrees();
            assert!(
                (field.declination_deg() - decl).abs() < 0.005,
                "D at {lat},{lon}: {} vs {decl}",
                field.declination_deg()
            );
            assert!((incl_got - incl).abs() < 0.005, "I at {lat},{lon}: {incl_got} vs {incl}");
            rows += 1;
        }
        assert_eq!(rows, 12, "official table should contribute 12 rows");
    }

    #[test]
    fn matches_extended_noaa_corpus() {
        let mut rows = 0;
        for row in data_rows(EXTENDED_CORPUS) {
            let [year, alt, lat, lon, decl, incl, h, x, y, z, f, ..] = row[..] else {
                panic!("short row in extended corpus");
            };
            let field = MODEL.field_at(lat, lon, alt, year);
            // Components are published to 1e-6 nT; an independent f64
            // implementation of the same algorithm agrees to ~1e-6.
            for (got, want, name) in [
                (field.x, x, "X"),
                (field.y, y, "Y"),
                (field.z, z, "Z"),
                (field.x.hypot(field.y), h, "H"),
                (field.x.hypot(field.y).hypot(field.z), f, "F"),
            ] {
                assert!(
                    (got - want).abs() < 1e-3,
                    "{name} at {lat},{lon},{alt},{year}: {got} vs {want}"
                );
            }
            assert!((field.declination_deg() - decl).abs() < 0.005);
            let incl_got = field.z.atan2(field.x.hypot(field.y)).to_degrees();
            assert!((incl_got - incl).abs() < 0.005);
            rows += 1;
        }
        assert_eq!(rows, 100, "extended corpus should contribute 100 rows");
    }

    fn ll(lat: f64, lon: f64) -> LatLon {
        LatLon::new(lat, lon).unwrap()
    }

    #[test]
    fn germany_declination_is_a_few_degrees_east() {
        let date = NaiveDate::from_ymd_opt(2026, 6, 11).unwrap();
        // West → east across Germany; all currently ~+3..+5° E.
        let aachen = magvar(ll(50.78, 6.08), date).0;
        let frankfurt = magvar(ll(50.03, 8.57), date).0;
        let goerlitz = magvar(ll(51.15, 14.99), date).0;
        for (name, d) in [("Aachen", aachen), ("Frankfurt", frankfurt), ("Görlitz", goerlitz)] {
            assert!((2.5..=5.5).contains(&d), "{name}: {d}° outside expected band");
        }
        // Declination increases eastward across Germany.
        assert!(aachen < frankfurt && frankfurt < goerlitz);
    }

    #[test]
    fn clamps_dates_outside_validity_window() {
        let position = ll(50.03, 8.57);
        // Before the window → identical to the first valid day.
        let early = magvar(position, NaiveDate::from_ymd_opt(2019, 6, 1).unwrap());
        let from = magvar(position, NaiveDate::from_ymd_opt(2025, 1, 1).unwrap());
        assert_eq!(early, from);
        // After the window → pinned to decimal year 2030.0.
        let late = magvar(position, NaiveDate::from_ymd_opt(2042, 3, 14).unwrap());
        let until = MODEL
            .field_at(position.lat(), position.lon(), 0.0, VALID_UNTIL)
            .declination_deg();
        assert_eq!(late.0, until);
        // Clamped results stay sane (no panic, no garbage).
        assert!(early.0.is_finite() && late.0.is_finite());
        assert!(late.0 > from.0, "declination in Germany is currently increasing");
    }

    #[test]
    fn decimal_year_convention() {
        let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert_eq!(decimal_year(d(2025, 1, 1)), 2025.0);
        assert!((decimal_year(d(2027, 7, 2)) - 2027.5).abs() < 2e-3); // mid-year
        assert!((decimal_year(d(2028, 12, 31)) - 2028.997).abs() < 1e-3); // leap year
    }
}
