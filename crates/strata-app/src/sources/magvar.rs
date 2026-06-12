//! [`MagvarSource`] over `strata-data`'s WMM2025 evaluation.

use chrono::NaiveDate;
use strata_data::domain::{LatLon, magvar};
use strata_plan::sources::{MagvarSource, SourceError};
use strata_plan::units::MagneticVariation;

/// WMM-backed magnetic variation. Stateless and infallible: the model is
/// compiled in, out-of-window dates clamp to the model edge (with a warning
/// from `strata-data`), and both sides use the east-positive convention.
pub struct WmmMagvarSource;

impl MagvarSource for WmmMagvarSource {
    fn magvar(&self, p: LatLon, date: NaiveDate) -> Result<MagneticVariation, SourceError> {
        Ok(MagneticVariation(magvar(p, date).0))
    }
}

#[cfg(test)]
mod tests {
    use strata_plan::units::DegreesTrue;

    use super::*;

    #[test]
    fn germany_variation_is_small_and_east_positive() {
        let p = LatLon::new(50.0, 10.0).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 6, 14).unwrap();
        let variation = WmmMagvarSource.magvar(p, date).unwrap();
        // Central Germany 2026: roughly +3..4° east declination.
        assert!(
            variation.0 > 2.0 && variation.0 < 6.0,
            "variation {variation:?}"
        );
        // East-positive convention: magnetic = true − variation.
        let magnetic = DegreesTrue::new(100.0).to_magnetic(variation);
        assert!(magnetic.0 < 100.0);
    }
}
