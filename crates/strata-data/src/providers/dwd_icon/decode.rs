//! bzip2 + GRIB2 decoding of ICON-D2 single-level files into
//! [`RegularLatLonGrid`]s.
//!
//! ICON-D2 specifics (decode-verified 2026-06-10):
//!
//! - Grid: GDT 3.0 regular lat-lon, 1215×746 points, 0.02° spacing, lat
//!   43.18..58.08 N, lon −3.94..20.34 E (encoded as 356.06..20.34), scan
//!   mode `0x40` (W→E, S→N, i consecutive). A Section-6 bitmap masks ~17 %
//!   of points outside the native icosahedral domain — those decode to
//!   `NaN`.
//! - Instantaneous fields (clct, lpi, cape_ml, …) use PDT 4.0; the valid
//!   time is reference time + forecast time (minutes for ICON-D2).
//! - `tot_prec` uses PDT 4.8: an accumulation from **run start** to the
//!   *end of the overall time interval* (template octets 35–41). Files for
//!   `tot_prec` and `lpi` hold FOUR messages (15-minute sub-steps); step
//!   `SSS` covers valid times `SSS:00`, `SSS:15`, `SSS:30` and `SSS:45` —
//!   selection is by exact valid time, so the on-the-hour message is the
//!   one whose valid time equals run + step hours.
//! - Packing: DRT 5.0 simple packing — no optional grib crate features
//!   (JPEG2000/PNG/CCSDS codecs) needed.

use std::io::Read;

use chrono::{DateTime, Duration, TimeZone, Utc};
use grib::codetables::grib2::Table4_4;
use grib::{Code, ForecastTime, Grib2Read, Grib2SubmessageDecoder, GridDefinitionTemplateValues, SubMessage};

use crate::domain::{LatLon, RegularLatLonGrid};

use super::DwdIconError;

/// Decompresses a `.bz2` payload (multi-stream tolerant).
pub(super) fn decompress_bz2(compressed: &[u8]) -> Result<Vec<u8>, DwdIconError> {
    let mut decoder = bzip2::read::MultiBzDecoder::new(compressed);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).map_err(DwdIconError::Bzip2)?;
    Ok(out)
}

/// Decodes the (sub)message of `bytes` whose valid time is exactly
/// `valid_time` into the normalized grid convention (row-major from the
/// south-west node, W→E then S→N).
///
/// Files with 15-minute sub-steps (tot_prec, lpi) hold four messages;
/// only the matching one has its values unpacked.
pub(super) fn decode_grid_at(
    bytes: Vec<u8>,
    valid_time: DateTime<Utc>,
) -> Result<RegularLatLonGrid, DwdIconError> {
    let grib2 = grib::from_bytes(bytes)?;
    for (_index, submessage) in grib2.iter() {
        if message_valid_time(&submessage)? != valid_time {
            continue;
        }
        let params = grid_params(&submessage)?;
        let decoder = Grib2SubmessageDecoder::from(submessage)?;
        // The decoder expands the Section-6 bitmap: masked points (outside
        // the native ICON-D2 domain) come out as NaN.
        let raw: Vec<f32> = decoder.dispatch()?.collect();
        if raw.len() != params.ni * params.nj {
            return Err(DwdIconError::ValueCountMismatch {
                got: raw.len(),
                ni: params.ni,
                nj: params.nj,
            });
        }
        let values = normalize_scan_order(&raw, params.ni, params.nj, params.scan_mode)?;
        return Ok(RegularLatLonGrid::new(
            params.origin,
            params.lat_spacing_deg,
            params.lon_spacing_deg,
            params.ni,
            params.nj,
            values,
        )?);
    }
    Err(DwdIconError::MessageNotFound { valid_time })
}

/// `(curr − prev) / 1 h` for two consecutive hourly run-start accumulations
/// (mm), yielding a rate in mm/h valid at `curr`'s time. Tiny negative
/// differences (packing noise) clamp to 0; a `NaN` on either side stays
/// no-data.
pub(super) fn accumulation_rate_mm_h(
    prev: &RegularLatLonGrid,
    curr: &RegularLatLonGrid,
) -> Result<RegularLatLonGrid, DwdIconError> {
    let aligned = prev.ni() == curr.ni()
        && prev.nj() == curr.nj()
        && prev.origin() == curr.origin()
        && prev.lat_spacing_deg() == curr.lat_spacing_deg()
        && prev.lon_spacing_deg() == curr.lon_spacing_deg();
    if !aligned {
        return Err(DwdIconError::GridMismatch);
    }
    let values = prev
        .values()
        .iter()
        .zip(curr.values())
        .map(|(&p, &c)| {
            let diff = c - p;
            // Branch instead of f32::max: max(NaN, 0.0) would turn
            // no-data into a zero rate.
            if diff < 0.0 { 0.0 } else { diff }
        })
        .collect();
    Ok(RegularLatLonGrid::new(
        curr.origin(),
        curr.lat_spacing_deg(),
        curr.lon_spacing_deg(),
        curr.ni(),
        curr.nj(),
        values,
    )?)
}

/// Converts a grid of Kelvin values to °C (ICON-D2 publishes temperature
/// in Kelvin; the [`crate::domain::WeatherField::Temperature`] contract is
/// °C). `NaN` no-data stays `NaN`.
pub(super) fn kelvin_to_celsius(
    grid: &RegularLatLonGrid,
) -> Result<RegularLatLonGrid, DwdIconError> {
    const KELVIN_OFFSET: f32 = 273.15;
    let values = grid.values().iter().map(|v| v - KELVIN_OFFSET).collect();
    Ok(RegularLatLonGrid::new(
        grid.origin(),
        grid.lat_spacing_deg(),
        grid.lon_spacing_deg(),
        grid.ni(),
        grid.nj(),
        values,
    )?)
}

/// The instant a message's data is valid for: end of the overall time
/// interval for statistically processed fields (PDT 4.8 accumulations),
/// reference time + forecast time otherwise.
fn message_valid_time<R: Grib2Read>(
    submessage: &SubMessage<'_, R>,
) -> Result<DateTime<Utc>, DwdIconError> {
    match submessage.prod_def().prod_tmpl_num() {
        8 => {
            // PDT 4.8: "time of end of overall time interval", section
            // octets 35–41 (year:u16, month, day, hour, minute, second).
            // The section payload exposed by the grib crate starts at
            // octet 6, so the field sits at payload bytes 29..36.
            let payload: Vec<u8> = submessage.prod_def().iter().copied().collect();
            let b = payload.get(29..36).ok_or(DwdIconError::InvalidTime)?;
            let year = u16::from_be_bytes([b[0], b[1]]);
            utc(
                i32::from(year),
                u32::from(b[2]),
                u32::from(b[3]),
                u32::from(b[4]),
                u32::from(b[5]),
                u32::from(b[6]),
            )
        }
        _ => {
            let rt = submessage.identification().ref_time_unchecked();
            let ref_time = utc(
                i32::from(rt.year),
                u32::from(rt.month),
                u32::from(rt.day),
                u32::from(rt.hour),
                u32::from(rt.minute),
                u32::from(rt.second),
            )?;
            let ft = submessage
                .prod_def()
                .forecast_time()
                .ok_or(DwdIconError::InvalidTime)?;
            Ok(ref_time + Duration::minutes(forecast_minutes(&ft)?))
        }
    }
}

fn forecast_minutes(ft: &ForecastTime) -> Result<i64, DwdIconError> {
    let value = i64::from(ft.value);
    match ft.unit {
        Code::Name(Table4_4::Minute) => Ok(value),
        Code::Name(Table4_4::Hour) => Ok(value * 60),
        Code::Name(Table4_4::Day) => Ok(value * 60 * 24),
        _ => Err(DwdIconError::UnsupportedTimeUnit),
    }
}

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Result<DateTime<Utc>, DwdIconError> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, s)
        .single()
        .ok_or(DwdIconError::InvalidTime)
}

/// Grid geometry extracted from a GDT 3.0 (regular lat-lon) definition,
/// normalized to degrees with the south-west node as origin.
struct GridParams {
    origin: LatLon,
    lat_spacing_deg: f64,
    lon_spacing_deg: f64,
    ni: usize,
    nj: usize,
    scan_mode: u8,
}

fn grid_params<R: Grib2Read>(submessage: &SubMessage<'_, R>) -> Result<GridParams, DwdIconError> {
    let template = GridDefinitionTemplateValues::try_from(submessage.grid_def())?;
    let GridDefinitionTemplateValues::Template0(def) = template else {
        return Err(DwdIconError::UnsupportedGrid(
            "only GDT 3.0 (regular lat-lon) is supported",
        ));
    };
    let grid = &def.lat_lon.grid;
    // Basic angle 0 (or missing) means coordinates are in 10⁻⁶ degree
    // units; anything else changes the unit and is not produced by DWD.
    if !(grid.initial_production_domain_basic_angle == 0
        || grid.initial_production_domain_basic_angle == u32::MAX)
    {
        return Err(DwdIconError::UnsupportedGrid("non-zero basic angle"));
    }
    let ni = grid.ni as usize;
    let nj = grid.nj as usize;
    let lat1 = micro_deg(f64::from(grid.first_point_lat));
    let lat2 = micro_deg(f64::from(grid.last_point_lat));
    let lon1 = signed_lon(micro_deg(f64::from(grid.first_point_lon)));
    let lon2 = signed_lon(micro_deg(f64::from(grid.last_point_lon)));
    let south = lat1.min(lat2);
    let west = lon1.min(lon2);
    let lon_spacing_deg = spacing(def.lat_lon.i_direction_inc, (lon1 - lon2).abs(), ni);
    let lat_spacing_deg = spacing(def.lat_lon.j_direction_inc, (lat1 - lat2).abs(), nj);
    // Guard against misread corners/increments: the corner span must equal
    // (n−1) steps to within one step.
    let lon_consistent = ((ni - 1) as f64 * lon_spacing_deg - (lon1 - lon2).abs()).abs()
        <= lon_spacing_deg;
    let lat_consistent = ((nj - 1) as f64 * lat_spacing_deg - (lat1 - lat2).abs()).abs()
        <= lat_spacing_deg;
    if !(lon_consistent && lat_consistent) {
        return Err(DwdIconError::UnsupportedGrid(
            "grid increments inconsistent with corner points",
        ));
    }
    Ok(GridParams {
        origin: LatLon::new(south, west)?,
        lat_spacing_deg,
        lon_spacing_deg,
        ni,
        nj,
        scan_mode: def.lat_lon.scanning_mode.0,
    })
}

fn micro_deg(v: f64) -> f64 {
    v * 1e-6
}

/// GRIB2 longitudes are 0..360; normalize to −180..180 (no antimeridian
/// handling — the ICON-D2 domain is far from it).
fn signed_lon(lon: f64) -> f64 {
    if lon > 180.0 { lon - 360.0 } else { lon }
}

/// Direction increment in degrees; a missing increment (all bits set) is
/// derived from the corner span.
fn spacing(increment: u32, span_deg: f64, n: usize) -> f64 {
    if increment == u32::MAX {
        span_deg / (n - 1) as f64
    } else {
        micro_deg(f64::from(increment))
    }
}

/// Reorders raw values from the file's scanning mode (GRIB2 flag table
/// 3.4) into the normalized `values[j * ni + i]` convention (i: W→E,
/// j: S→N). ICON-D2 ships `0x40` which is already normalized; the other
/// non-boustrophedon modes are handled for robustness.
fn normalize_scan_order(
    raw: &[f32],
    ni: usize,
    nj: usize,
    mode: u8,
) -> Result<Vec<f32>, DwdIconError> {
    debug_assert_eq!(raw.len(), ni * nj);
    if mode & 0x10 != 0 {
        return Err(DwdIconError::UnsupportedGrid(
            "boustrophedon scanning is not supported",
        ));
    }
    let i_negative = mode & 0x80 != 0; // bit 1: points scan E→W
    let j_positive = mode & 0x40 != 0; // bit 2: points scan S→N
    let j_consecutive = mode & 0x20 != 0; // bit 3: adjacent points are in j
    let mut out = vec![f32::NAN; raw.len()];
    for (k, &v) in raw.iter().enumerate() {
        let (i_scan, j_scan) = if j_consecutive {
            (k / nj, k % nj)
        } else {
            (k % ni, k / ni)
        };
        let i = if i_negative { ni - 1 - i_scan } else { i_scan };
        let j = if j_positive { j_scan } else { nj - 1 - j_scan };
        out[j * ni + i] = v;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real DWD file, run 2026-06-10 15 UTC, step 001 — four PDT 4.0
    /// messages (valid +60/+75/+90/+105 minutes).
    const LPI_FIXTURE: &[u8] = include_bytes!(
        "../../../tests/fixtures/dwd_icon/icon-d2_germany_regular-lat-lon_single-level_2026061015_001_2d_lpi.grib2.bz2"
    );
    /// Real DWD file, run 2026-06-10 15 UTC, step 000 — one PDT 4.0 message.
    const CLCT_FIXTURE: &[u8] = include_bytes!(
        "../../../tests/fixtures/dwd_icon/icon-d2_germany_regular-lat-lon_single-level_2026061015_000_2d_clct.grib2.bz2"
    );
    /// Real DWD pressure-level file (run 2026-06-11 00 UTC, step 001,
    /// 850 hPa u-wind, one PDT 4.0 message), downloaded from
    /// `https://opendata.dwd.de/weather/nwp/icon-d2/grib/00/u/` and cropped
    /// to lat 47–55 N, lon 5–15 E with `crop_fixture.py` (full files are
    /// ~1 MB; the crop masks the Section-6 bitmap — grid geometry and the
    /// kept packed values are byte-identical to the original).
    const U850_FIXTURE: &[u8] = include_bytes!(
        "../../../tests/fixtures/dwd_icon/icon-d2_germany_regular-lat-lon_pressure-level_2026061100_001_850_u.grib2.bz2"
    );
    /// Real DWD file (run 2026-06-11 00 UTC, step 048, height of the 0 °C
    /// isotherm, one PDT 4.0 message), downloaded from
    /// `https://opendata.dwd.de/weather/nwp/icon-d2/grib/00/hzerocl/` and
    /// cropped like [`U850_FIXTURE`]. Step 048 holds a single message;
    /// earlier hzerocl steps carry four 15-minute sub-step messages.
    const HZEROCL_FIXTURE: &[u8] = include_bytes!(
        "../../../tests/fixtures/dwd_icon/icon-d2_germany_regular-lat-lon_single-level_2026061100_048_2d_hzerocl.grib2.bz2"
    );

    fn run_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 10, 15, 0, 0).unwrap()
    }

    fn grid(ni: usize, nj: usize, values: Vec<f32>) -> RegularLatLonGrid {
        RegularLatLonGrid::new(
            LatLon::new(50.0, 10.0).unwrap(),
            0.5,
            0.5,
            ni,
            nj,
            values,
        )
        .unwrap()
    }

    #[test]
    fn clct_fixture_decodes_end_to_end() {
        let bytes = decompress_bz2(CLCT_FIXTURE).expect("valid bz2");
        let grid = decode_grid_at(bytes, run_time()).expect("step-0 message decodes");

        // ICON-D2 regular lat-lon grid (verified source facts).
        assert_eq!(grid.ni(), 1215);
        assert_eq!(grid.nj(), 746);
        assert!((grid.lat_spacing_deg() - 0.02).abs() < 1e-9);
        assert!((grid.lon_spacing_deg() - 0.02).abs() < 1e-9);
        let extent = grid.extent();
        assert!((extent.west() - -3.94).abs() < 1e-6);
        assert!((extent.south() - 43.18).abs() < 1e-6);
        assert!((extent.east() - 20.34).abs() < 1e-6);
        assert!((extent.north() - 58.08).abs() < 1e-6);

        // The Section-6 bitmap masks points outside the native domain:
        // 906390 total − 754862 defined (file's Section 5) = 151528 NaN.
        let nan_count = grid.values().iter().filter(|v| v.is_nan()).count();
        assert_eq!(grid.values().len(), 1215 * 746);
        assert_eq!(nan_count, 151_528);

        // Cloud cover is a percentage.
        assert!(
            grid.values()
                .iter()
                .filter(|v| !v.is_nan())
                .all(|v| (0.0..=100.0).contains(v))
        );

        // Frankfurt area lies well inside the native domain.
        let v = grid
            .sample(LatLon::new(50.0, 8.6).unwrap())
            .expect("inside the domain");
        assert!((0.0..=100.0).contains(&v));
    }

    #[test]
    fn u850_pressure_level_fixture_decodes_end_to_end() {
        let bytes = decompress_bz2(U850_FIXTURE).expect("valid bz2");
        let run = Utc.with_ymd_and_hms(2026, 6, 11, 0, 0, 0).unwrap();
        let grid = decode_grid_at(bytes, run + Duration::hours(1)).expect("step-1 message decodes");

        // Pressure-level files share the regular-lat-lon ICON-D2 grid.
        assert_eq!(grid.ni(), 1215);
        assert_eq!(grid.nj(), 746);
        assert!((grid.lat_spacing_deg() - 0.02).abs() < 1e-9);
        assert!((grid.lon_spacing_deg() - 0.02).abs() < 1e-9);
        let extent = grid.extent();
        assert!((extent.west() - -3.94).abs() < 1e-6);
        assert!((extent.south() - 43.18).abs() < 1e-6);
        assert!((extent.east() - 20.34).abs() < 1e-6);
        assert!((extent.north() - 58.08).abs() < 1e-6);

        // The fixture crop keeps the 47–55 N / 5–15 E window: 501 × 401
        // nodes, all inside the native ICON-D2 domain.
        let data_count = grid.values().iter().filter(|v| !v.is_nan()).count();
        assert_eq!(data_count, 501 * 401);

        // Plausible 850 hPa wind components.
        assert!(
            grid.values()
                .iter()
                .filter(|v| !v.is_nan())
                .all(|v| (-60.0..=60.0).contains(v))
        );
        let frankfurt = grid
            .sample(LatLon::new(50.0, 8.6).unwrap())
            .expect("inside the cropped window");
        assert!((-60.0..=60.0).contains(&frankfurt));
        // Outside the crop window (but inside the grid) is no-data.
        assert_eq!(grid.sample(LatLon::new(44.0, 0.0).unwrap()), None);
    }

    #[test]
    fn hzerocl_fixture_decodes_to_plausible_freezing_levels() {
        let bytes = decompress_bz2(HZEROCL_FIXTURE).expect("valid bz2");
        let run = Utc.with_ymd_and_hms(2026, 6, 11, 0, 0, 0).unwrap();
        let grid =
            decode_grid_at(bytes, run + Duration::hours(48)).expect("step-48 message decodes");

        assert_eq!(grid.ni(), 1215);
        assert_eq!(grid.nj(), 746);
        let data_count = grid.values().iter().filter(|v| !v.is_nan()).count();
        assert_eq!(data_count, 501 * 401);

        // hzerocl is meters AMSL; plausible June freezing levels over
        // Germany sit a few km up (the fixture spans ~2000–4100 m).
        assert!(
            grid.values()
                .iter()
                .filter(|v| !v.is_nan())
                .all(|v| (0.0..=8000.0).contains(v))
        );
        let frankfurt = grid
            .sample(LatLon::new(50.0, 8.6).unwrap())
            .expect("inside the cropped window");
        assert!((1000.0..=6000.0).contains(&frankfurt));
    }

    #[test]
    fn kelvin_grids_convert_to_celsius() {
        let k = grid(2, 2, vec![273.15, 288.15, f32::NAN, 218.15]);
        let c = kelvin_to_celsius(&k).unwrap();
        let close = |i, j, want: f32| {
            let got = c.value_at(i, j).unwrap();
            assert!((got - want).abs() < 1e-3, "({i},{j}): got {got}, want {want}");
        };
        close(0, 0, 0.0);
        close(1, 0, 15.0);
        assert!(c.value_at(0, 1).unwrap().is_nan());
        close(1, 1, -55.0);
        assert_eq!(c.origin(), k.origin());
        assert_eq!(c.ni(), 2);
    }

    #[test]
    fn lpi_fixture_selects_messages_by_valid_time() {
        let bytes = decompress_bz2(LPI_FIXTURE).expect("valid bz2");

        // Step-001 file: the on-the-hour message is run + 1 h.
        let on_hour = decode_grid_at(bytes.clone(), run_time() + Duration::hours(1))
            .expect("on-the-hour message exists");
        assert_eq!(on_hour.ni(), 1215);
        assert_eq!(on_hour.nj(), 746);

        // The 15-minute sub-step is a different message with different data.
        let quarter = decode_grid_at(
            bytes.clone(),
            run_time() + Duration::minutes(75),
        )
        .expect("+75 min sub-step message exists");
        assert_ne!(on_hour.values(), quarter.values());

        // Times not in this file: the run itself and the next hour.
        assert!(matches!(
            decode_grid_at(bytes.clone(), run_time()),
            Err(DwdIconError::MessageNotFound { .. })
        ));
        assert!(matches!(
            decode_grid_at(bytes, run_time() + Duration::hours(2)),
            Err(DwdIconError::MessageNotFound { .. })
        ));
    }

    #[test]
    fn scan_order_0x40_is_already_normalized() {
        // ICON-D2's mode: W→E, S→N, i consecutive.
        let raw = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let out = normalize_scan_order(&raw, 3, 2, 0x40).unwrap();
        assert_eq!(out, raw);
    }

    #[test]
    fn scan_order_north_to_south_flips_rows() {
        // Mode 0x00: W→E, N→S — raw rows arrive north first.
        let raw = [4.0, 5.0, 6.0, 1.0, 2.0, 3.0];
        let out = normalize_scan_order(&raw, 3, 2, 0x00).unwrap();
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn scan_order_east_to_west_flips_columns() {
        // Mode 0xC0: E→W, S→N.
        let raw = [3.0, 2.0, 1.0, 6.0, 5.0, 4.0];
        let out = normalize_scan_order(&raw, 3, 2, 0xC0).unwrap();
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn scan_order_j_consecutive_transposes() {
        // Mode 0x60: S→N with adjacent points along j (column-major).
        let raw = [1.0, 4.0, 2.0, 5.0, 3.0, 6.0];
        let out = normalize_scan_order(&raw, 3, 2, 0x60).unwrap();
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn boustrophedon_scanning_is_rejected() {
        assert!(matches!(
            normalize_scan_order(&[0.0; 6], 3, 2, 0x50),
            Err(DwdIconError::UnsupportedGrid(_))
        ));
    }

    #[test]
    fn accumulation_diff_yields_a_clamped_rate() {
        // Accumulations in mm since run start, one hour apart.
        let prev = grid(2, 2, vec![0.0, 1.0, f32::NAN, 2.0]);
        let curr = grid(2, 2, vec![0.5, 0.9, f32::NAN, 4.5]);
        let rate = accumulation_rate_mm_h(&prev, &curr).unwrap();
        let values = rate.values();
        assert_eq!(values[0], 0.5);
        // Packing noise can make accumulations decrease a hair: clamp to 0.
        assert_eq!(values[1], 0.0);
        assert!(values[2].is_nan());
        assert_eq!(values[3], 2.5);
        assert_eq!(rate.origin(), curr.origin());
    }

    #[test]
    fn accumulation_diff_rejects_misaligned_grids() {
        let prev = grid(2, 2, vec![0.0; 4]);
        let curr = grid(3, 2, vec![0.0; 6]);
        assert!(matches!(
            accumulation_rate_mm_h(&prev, &curr),
            Err(DwdIconError::GridMismatch)
        ));
    }
}
