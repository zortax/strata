//! RADOLAN binary frame parsing (RV product flavor).
//!
//! A frame is an ASCII header terminated by an ETX octet (`0x03`) followed
//! by a row-major little-endian `u16` payload that starts at the
//! **south-west** pixel and runs west→east, south→north — exactly the
//! [`RegularLatLonGrid`](crate::domain::RegularLatLonGrid) storage
//! convention, just in projected (not lat-lon) space.
//!
//! Header layout per the RADOLAN/RADVOR composite format spec v2.6 and the
//! RV format description: a fixed prefix (product id, `DDhhmm` measurement
//! time, WMO number, `MMYY`) followed by keyed tokens (`BY` product length,
//! `VS` format version, `SW` software, `PR` precision as a power of ten,
//! `INT` interval minutes, `GP` rows×columns, `VV` forecast minutes, `MF`
//! module flags, `MS` station list). The spec mandates parsing by key, not
//! position.
//!
//! Payload bit layout (16 bits per pixel): bits 1–12 data value, bit 13
//! secondary-data flag (value stays valid), bit 14 no-data, bit 15
//! negative sign, bit 16 clutter. No-data and clutter decode to NaN.

use chrono::{DateTime, Duration, TimeZone, Utc};

use super::DwdRadarError;

/// Bit 14: no-data ("Fehlkennung").
const NO_DATA: u16 = 0x2000;
/// Bit 16: clutter mark.
const CLUTTER: u16 = 0x8000;
/// Bit 15: negative sign.
const NEGATIVE: u16 = 0x4000;
/// Bits 1–12: the data value.
const VALUE_MASK: u16 = 0x0FFF;

/// Headers are well under 1 KiB even with the maximal `MS` station list
/// (999 chars); a missing ETX in this window means a corrupt frame.
const MAX_HEADER_BYTES: usize = 2048;

/// One decoded RADOLAN frame: header metadata plus the precipitation
/// amounts in mm per `interval_minutes`, row-major from the south-west
/// pixel (`values_mm[j * nx + i]`, i west→east, j south→north), NaN for
/// no-data/clutter.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RadolanFrame {
    /// Two-letter product id from the header (e.g. `RV`).
    pub product: String,
    /// Measurement/analysis time from the header prefix.
    pub analysis_time: DateTime<Utc>,
    /// `VV` lead time: minutes this frame lies after [`Self::analysis_time`]
    /// (0 = the analysis itself).
    pub forecast_minutes: u32,
    /// `INT` accumulation interval in minutes (5 for RV).
    pub interval_minutes: u32,
    /// `PR` exponent: values scale by 10^exponent mm (−2 for RV).
    pub precision_exponent: i32,
    /// Columns (west→east), from `GP`.
    pub nx: usize,
    /// Rows (south→north), from `GP`.
    pub ny: usize,
    /// Precipitation amount in mm per interval; NaN = no-data.
    pub values_mm: Vec<f32>,
}

impl RadolanFrame {
    /// The instant this frame is valid for.
    pub fn valid_time(&self) -> DateTime<Utc> {
        self.analysis_time + Duration::minutes(i64::from(self.forecast_minutes))
    }

    /// Converts the per-interval amounts to a rate in mm/h (×60/INT),
    /// clamping negatives to 0 and keeping NaN as no-data.
    pub fn rate_mm_h(&self) -> Vec<f32> {
        let scale = 60.0 / self.interval_minutes as f32;
        self.values_mm
            .iter()
            .map(|&v| if v.is_nan() { v } else { (v * scale).max(0.0) })
            .collect()
    }
}

/// Parses one RADOLAN frame (header + payload).
pub(super) fn parse_frame(bytes: &[u8]) -> Result<RadolanFrame, DwdRadarError> {
    let etx = bytes
        .iter()
        .take(MAX_HEADER_BYTES)
        .position(|&b| b == 0x03)
        .ok_or(DwdRadarError::InvalidHeader("no ETX terminator"))?;
    let header = std::str::from_utf8(&bytes[..etx])
        .map_err(|_| DwdRadarError::InvalidHeader("header is not ASCII"))?;
    let (meta, nx, ny) = parse_header(header)?;

    let payload = &bytes[etx + 1..];
    let expected = nx * ny * 2;
    if payload.len() != expected {
        return Err(DwdRadarError::PayloadSizeMismatch {
            got: payload.len(),
            expected,
            nx,
            ny,
        });
    }
    let factor = 10f32.powi(meta.precision_exponent);
    let values_mm = payload
        .chunks_exact(2)
        .map(|c| decode_value(u16::from_le_bytes([c[0], c[1]]), factor))
        .collect();
    Ok(RadolanFrame {
        product: meta.product,
        analysis_time: meta.analysis_time,
        forecast_minutes: meta.forecast_minutes,
        interval_minutes: meta.interval_minutes,
        precision_exponent: meta.precision_exponent,
        nx,
        ny,
        values_mm,
    })
}

/// One 16-bit pixel → mm per interval (or NaN).
fn decode_value(v: u16, factor: f32) -> f32 {
    if v & (NO_DATA | CLUTTER) != 0 {
        return f32::NAN;
    }
    let magnitude = f32::from(v & VALUE_MASK) * factor;
    if v & NEGATIVE != 0 { -magnitude } else { magnitude }
}

/// Header fields needed downstream (dims returned separately).
struct HeaderMeta {
    product: String,
    analysis_time: DateTime<Utc>,
    forecast_minutes: u32,
    interval_minutes: u32,
    precision_exponent: i32,
}

fn parse_header(header: &str) -> Result<(HeaderMeta, usize, usize), DwdRadarError> {
    let mut cur = Cursor { rest: header };
    // Fixed positional prefix: A2 product, 3I2 DDhhmm, I5 WMO, 2I2 MMYY.
    let product = cur.take(2)?.to_owned();
    let day = int_field(cur.take(2)?)?;
    let hour = int_field(cur.take(2)?)?;
    let minute = int_field(cur.take(2)?)?;
    let _wmo = cur.take(5)?;
    let month = int_field(cur.take(2)?)?;
    let year = int_field(cur.take(2)?)?;
    let analysis_time = Utc
        .with_ymd_and_hms(2000 + year as i32, month, day, hour, minute, 0)
        .single()
        .ok_or(DwdRadarError::InvalidTimestamp)?;

    // Keyed tokens, any order (the spec mandates key-driven parsing).
    let mut precision_exponent = None;
    let mut interval_minutes = None;
    let mut dims = None;
    let mut forecast_minutes = 0u32;
    while !cur.rest.is_empty() {
        // `INT` before the two-char keys: it is the only three-char key.
        if cur.eat("INT") {
            interval_minutes = Some(cur.number_u32()?);
        } else if cur.eat("BY") {
            cur.number()?; // product length; payload is validated by GP
        } else if cur.eat("VS") {
            cur.number()?;
        } else if cur.eat("SW") {
            cur.take(9)?; // 1X,A8 software version
        } else if cur.eat("PR") {
            cur.take(1)?; // 1X
            precision_exponent = Some(parse_precision(cur.take(4)?)?);
        } else if cur.eat("GP") {
            dims = Some(parse_dims(cur.take(9)?)?);
        } else if cur.eat("VV") {
            forecast_minutes = cur.number_u32()?;
        } else if cur.eat("MF") {
            cur.number()?;
        } else if cur.eat("MS") {
            let len = int_field(cur.take(3)?)? as usize;
            cur.take(len)?; // station list text
        } else {
            return Err(DwdRadarError::InvalidHeader("unknown header token"));
        }
    }

    let interval_minutes =
        interval_minutes.ok_or(DwdRadarError::InvalidHeader("missing INT token"))?;
    if interval_minutes == 0 {
        return Err(DwdRadarError::InvalidHeader("zero INT interval"));
    }
    let precision_exponent =
        precision_exponent.ok_or(DwdRadarError::InvalidHeader("missing PR token"))?;
    let (ny, nx) = dims.ok_or(DwdRadarError::InvalidHeader("missing GP token"))?;
    Ok((
        HeaderMeta {
            product,
            analysis_time,
            forecast_minutes,
            interval_minutes,
            precision_exponent,
        },
        nx,
        ny,
    ))
}

/// `PR` value, e.g. `E-02` → −2.
fn parse_precision(s: &str) -> Result<i32, DwdRadarError> {
    s.trim()
        .strip_prefix('E')
        .and_then(|exp| exp.parse().ok())
        .ok_or(DwdRadarError::InvalidHeader("malformed PR precision"))
}

/// `GP` value, e.g. `1200x1100` → (rows, columns).
fn parse_dims(s: &str) -> Result<(usize, usize), DwdRadarError> {
    let malformed = || DwdRadarError::InvalidHeader("malformed GP dimensions");
    let (rows, cols) = s.split_once('x').ok_or_else(malformed)?;
    let ny = rows.trim().parse().map_err(|_| malformed())?;
    let nx = cols.trim().parse().map_err(|_| malformed())?;
    Ok((ny, nx))
}

fn int_field(s: &str) -> Result<u32, DwdRadarError> {
    s.trim()
        .parse()
        .map_err(|_| DwdRadarError::InvalidHeader("expected an integer field"))
}

/// Minimal forward-only view over the header text.
struct Cursor<'a> {
    rest: &'a str,
}

impl<'a> Cursor<'a> {
    /// Consumes `key` if the remaining header starts with it.
    fn eat(&mut self, key: &str) -> bool {
        match self.rest.strip_prefix(key) {
            Some(r) => {
                self.rest = r;
                true
            }
            None => false,
        }
    }

    /// Consumes exactly `n` bytes (the header is ASCII, so bytes = chars).
    fn take(&mut self, n: usize) -> Result<&'a str, DwdRadarError> {
        if self.rest.len() < n {
            return Err(DwdRadarError::InvalidHeader("truncated header field"));
        }
        let (v, r) = self.rest.split_at(n);
        self.rest = r;
        Ok(v)
    }

    /// Consumes leading spaces then a run of digits.
    fn number(&mut self) -> Result<u64, DwdRadarError> {
        let s = self.rest.trim_start_matches(' ');
        let end = s
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(s.len());
        if end == 0 {
            return Err(DwdRadarError::InvalidHeader("expected a number"));
        }
        let value = s[..end]
            .parse()
            .map_err(|_| DwdRadarError::InvalidHeader("number out of range"))?;
        self.rest = &s[end..];
        Ok(value)
    }

    fn number_u32(&mut self) -> Result<u32, DwdRadarError> {
        u32::try_from(self.number()?)
            .map_err(|_| DwdRadarError::InvalidHeader("number out of range"))
    }
}

/// Test fixtures shared with the sibling modules' tests.
#[cfg(test)]
pub(super) mod testutil {
    use std::io::Read;

    /// Real analysis frame (lead 000) from the RV tarball published
    /// 2026-06-10 17:10 UTC, re-bzip2ed standalone.
    pub const RV_FIXTURE: &[u8] = include_bytes!(
        "../../../tests/fixtures/dwd_radar/DE1200_RV2606101710_000.bz2"
    );

    pub fn decompress(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        bzip2::read::MultiBzDecoder::new(bytes)
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    /// Builds a synthetic mini frame: 3 columns × 2 rows, RV-style header
    /// (product RV, 10th 17:10 UTC, WMO 10000, June 2026, lead +30 min).
    pub fn synthetic_frame(pixels: [u16; 6]) -> Vec<u8> {
        let mut bytes =
            b"RV101710100000626BY       107VS 5SW  P42001HPR E-02INT   5GP   2x   3VV 030MF 00000008MS  0"
                .to_vec();
        bytes.push(0x03);
        for v in pixels {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::{RV_FIXTURE, decompress, synthetic_frame};
    use super::*;

    #[test]
    fn synthetic_header_parses_every_field() {
        let frame = parse_frame(&synthetic_frame([0; 6])).unwrap();
        assert_eq!(frame.product, "RV");
        assert_eq!(
            frame.analysis_time,
            Utc.with_ymd_and_hms(2026, 6, 10, 17, 10, 0).unwrap()
        );
        assert_eq!(frame.forecast_minutes, 30);
        assert_eq!(frame.interval_minutes, 5);
        assert_eq!(frame.precision_exponent, -2);
        assert_eq!((frame.nx, frame.ny), (3, 2));
        assert_eq!(
            frame.valid_time(),
            Utc.with_ymd_and_hms(2026, 6, 10, 17, 40, 0).unwrap()
        );
    }

    #[test]
    fn payload_decodes_flags_sign_and_precision() {
        let frame = parse_frame(&synthetic_frame([
            0,               // 0.00 mm
            123,             // 1.23 mm (E-02 scaling)
            0x2000 | 2500,   // no-data → NaN, regardless of value bits
            0x8000 | 2490,   // clutter → NaN
            0x4000 | 1,      // negative sign → −0.01 mm
            0x1000 | 4095,   // secondary-data flag: value stays valid
        ]))
        .unwrap();
        let v = &frame.values_mm;
        assert_eq!(v[0], 0.0);
        assert!((v[1] - 1.23).abs() < 1e-6);
        assert!(v[2].is_nan());
        assert!(v[3].is_nan());
        assert!((v[4] - -0.01).abs() < 1e-6);
        assert!((v[5] - 40.95).abs() < 1e-4);
    }

    #[test]
    fn rate_scales_to_mm_h_and_clamps_negatives() {
        let frame = parse_frame(&synthetic_frame([
            0,
            123,
            0x2000,
            0,
            0x4000 | 1,
            50,
        ]))
        .unwrap();
        let rate = frame.rate_mm_h();
        assert_eq!(rate[0], 0.0);
        assert!((rate[1] - 14.76).abs() < 1e-4); // 1.23 mm / 5 min × 12
        assert!(rate[2].is_nan());
        assert_eq!(rate[4], 0.0); // negative clamps, does not become NaN
        assert!((rate[5] - 6.0).abs() < 1e-5);
    }

    #[test]
    fn corrupt_frames_are_rejected() {
        // No ETX at all.
        assert!(matches!(
            parse_frame(b"RV101710100000626BY"),
            Err(DwdRadarError::InvalidHeader("no ETX terminator"))
        ));
        // Unknown token.
        let mut bad = synthetic_frame([0; 6]);
        bad[17] = b'Z'; // BY → ZY
        bad[18] = b'Y';
        assert!(matches!(
            parse_frame(&bad),
            Err(DwdRadarError::InvalidHeader("unknown header token"))
        ));
        // Payload shorter than GP advertises.
        let mut short = synthetic_frame([0; 6]);
        short.truncate(short.len() - 2);
        assert!(matches!(
            parse_frame(&short),
            Err(DwdRadarError::PayloadSizeMismatch { got: 10, expected: 12, nx: 3, ny: 2 })
        ));
        // Nonsense month.
        let mut bad_time = synthetic_frame([0; 6]);
        bad_time[13] = b'9'; // month 96
        assert!(matches!(
            parse_frame(&bad_time),
            Err(DwdRadarError::InvalidTimestamp)
        ));
    }

    #[test]
    fn real_rv_analysis_frame_parses() {
        let frame = parse_frame(&decompress(RV_FIXTURE)).unwrap();
        assert_eq!(frame.product, "RV");
        assert_eq!(
            frame.analysis_time,
            Utc.with_ymd_and_hms(2026, 6, 10, 17, 10, 0).unwrap()
        );
        assert_eq!(frame.forecast_minutes, 0);
        assert_eq!(frame.interval_minutes, 5);
        assert_eq!(frame.precision_exponent, -2);
        assert_eq!((frame.nx, frame.ny), (1100, 1200));
        assert_eq!(frame.values_mm.len(), 1100 * 1200);

        // Decode-verified ground truth for this frame (Python reference
        // implementation against the same bytes).
        let nan = frame.values_mm.iter().filter(|v| v.is_nan()).count();
        assert_eq!(nan, 619_413);
        let max = frame
            .values_mm
            .iter()
            .filter(|v| !v.is_nan())
            .fold(f32::MIN, |a, &b| a.max(b));
        assert!((max - 6.35).abs() < 1e-4); // raw 635 × 0.01 mm / 5 min
        let rate = frame.rate_mm_h();
        let max_rate = rate.iter().filter(|v| !v.is_nan()).fold(f32::MIN, |a, &b| a.max(b));
        assert!((max_rate - 76.2).abs() < 1e-3);
    }
}
