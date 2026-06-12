//! Live weather: METAR, TAF, SIGMET. Dynamic data — never persisted as
//! authoritative, only TTL-cached.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::airport::IcaoCode;
use super::geo::Polygon;

/// One US statute mile in meters (flight-category thresholds are US-defined).
const METERS_PER_STATUTE_MILE: f64 = 1_609.344;

/// A METAR observation: raw text plus an optional decode (the decode may be
/// absent when the report could not be parsed; the raw text always stands).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metar {
    pub raw: String,
    pub station: IcaoCode,
    pub observed_at: DateTime<Utc>,
    pub decoded: Option<MetarDecode>,
}

/// Decoded METAR body. Fields are `Option` because AUTO stations routinely
/// report unavailable groups (`/////KT`, `//////`); whatever could not be
/// attributed at all lands in `unparsed_tokens`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetarDecode {
    pub wind: Option<Wind>,
    pub visibility: Option<Visibility>,
    pub weather: Vec<WxPhenomenon>,
    pub clouds: Vec<CloudLayer>,
    /// Vertical visibility (`VVnnn`) in feet, reported instead of clouds in
    /// obscured conditions; acts as the ceiling.
    pub vertical_visibility_ft: Option<u32>,
    pub temperature_c: Option<i16>,
    pub dewpoint_c: Option<i16>,
    pub qnh: Option<Qnh>,
    pub trend: Option<Trend>,
    pub auto: bool,
    pub remarks: Option<String>,
    pub unparsed_tokens: Vec<String>,
}

impl MetarDecode {
    /// Ceiling: lowest broken/overcast cloud base, else vertical visibility.
    pub fn ceiling_ft_agl(&self) -> Option<u32> {
        self.clouds
            .iter()
            .filter(|c| c.amount.is_ceiling())
            .filter_map(|c| c.base_ft_agl)
            .min()
            .or(self.vertical_visibility_ft)
    }

    /// `None` when visibility is unknown.
    pub fn flight_category(&self) -> Option<FlightCategory> {
        self.visibility
            .map(|vis| FlightCategory::from_ceiling_and_visibility(self.ceiling_ft_agl(), vis))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WindDirection {
    /// True degrees the wind blows *from*.
    Degrees(u16),
    /// `VRB`.
    Variable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Wind {
    pub direction: WindDirection,
    pub speed_kt: u16,
    pub gust_kt: Option<u16>,
    /// Variable-direction range group, e.g. `180V240` -> `(180, 240)`.
    pub variable_range: Option<(u16, u16)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Visibility {
    /// Prevailing visibility in meters; `9999` means 10 km or more.
    Meters(u32),
    /// Ceiling and visibility OK.
    Cavok,
}

impl Visibility {
    /// Visibility floor in meters (`Cavok` implies at least 10 km).
    pub fn meters(self) -> u32 {
        match self {
            Self::Meters(m) => m,
            Self::Cavok => 10_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WxIntensity {
    /// `-` prefix.
    Light,
    /// No prefix.
    Moderate,
    /// `+` prefix.
    Heavy,
    /// `VC` prefix.
    Vicinity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WxDescriptor {
    /// `MI`
    Shallow,
    /// `BC`
    Patches,
    /// `PR`
    Partial,
    /// `DR`
    LowDrifting,
    /// `BL`
    Blowing,
    /// `SH`
    Showers,
    /// `TS`
    Thunderstorm,
    /// `FZ`
    Freezing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WxKind {
    /// `DZ`
    Drizzle,
    /// `RA`
    Rain,
    /// `SN`
    Snow,
    /// `SG`
    SnowGrains,
    /// `IC`
    IceCrystals,
    /// `PL`
    IcePellets,
    /// `GR`
    Hail,
    /// `GS`
    SmallHail,
    /// `UP`
    UnknownPrecipitation,
    /// `BR`
    Mist,
    /// `FG`
    Fog,
    /// `FU`
    Smoke,
    /// `VA`
    VolcanicAsh,
    /// `DU`
    WidespreadDust,
    /// `SA`
    Sand,
    /// `HZ`
    Haze,
    /// `PO`
    DustWhirls,
    /// `SQ`
    Squalls,
    /// `FC`
    FunnelCloud,
    /// `SS`
    Sandstorm,
    /// `DS`
    DustStorm,
}

/// One decoded present-weather group. A group with multiple precipitation
/// codes (e.g. `-RASN`) decodes into one phenomenon per code. At least one
/// of `descriptor`/`kind` is set (`TS` alone is descriptor-only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WxPhenomenon {
    pub intensity: WxIntensity,
    pub descriptor: Option<WxDescriptor>,
    pub kind: Option<WxKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CloudAmount {
    /// `FEW` (1–2 oktas)
    Few,
    /// `SCT` (3–4 oktas)
    Scattered,
    /// `BKN` (5–7 oktas)
    Broken,
    /// `OVC` (8 oktas)
    Overcast,
    /// `NCD` — no cloud detected (automatic stations).
    NoCloudDetected,
    /// `NSC` — no significant cloud.
    NoSignificantCloud,
}

impl CloudAmount {
    /// Whether this layer constitutes a ceiling (broken or overcast).
    pub fn is_ceiling(self) -> bool {
        matches!(self, Self::Broken | Self::Overcast)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CloudKind {
    /// `CB`
    Cumulonimbus,
    /// `TCU`
    ToweringCumulus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CloudLayer {
    pub amount: CloudAmount,
    /// `None` when the base is unreported (`OVC051///`-style or NCD/NSC).
    pub base_ft_agl: Option<u32>,
    pub kind: Option<CloudKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Qnh {
    /// `Qnnnn` (hectopascal), the European form.
    Hpa(u16),
    /// `Annnn` (inches of mercury), the North American form.
    InHg(f32),
}

impl Qnh {
    pub fn as_hpa(self) -> f32 {
        match self {
            Self::Hpa(hpa) => f32::from(hpa),
            Self::InHg(in_hg) => in_hg * 33.863_89,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Trend {
    Nosig,
    Becmg,
    Tempo,
}

/// US flight category. Variants are ordered by increasing severity, so
/// `Ord::max` of two categories yields the more restrictive one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FlightCategory {
    /// Ceiling > 3000 ft AND visibility > 5 SM.
    Vfr,
    /// Ceiling 1000–3000 ft and/or visibility 3–5 SM.
    Mvfr,
    /// Ceiling 500–999 ft and/or visibility 1 to < 3 SM.
    Ifr,
    /// Ceiling < 500 ft and/or visibility < 1 SM.
    Lifr,
}

impl FlightCategory {
    /// Derives the category from ceiling (ft AGL; `None` = unlimited) and
    /// visibility, using the US thresholds. The worse of the two governs.
    pub fn from_ceiling_and_visibility(ceiling_ft_agl: Option<u32>, visibility: Visibility) -> Self {
        let by_ceiling = match ceiling_ft_agl {
            None => Self::Vfr,
            Some(ft) if ft < 500 => Self::Lifr,
            Some(ft) if ft < 1_000 => Self::Ifr,
            Some(ft) if ft <= 3_000 => Self::Mvfr,
            Some(_) => Self::Vfr,
        };
        let by_visibility = match visibility {
            Visibility::Cavok => Self::Vfr,
            Visibility::Meters(m) => {
                let sm = f64::from(m) / METERS_PER_STATUTE_MILE;
                if sm < 1.0 {
                    Self::Lifr
                } else if sm < 3.0 {
                    Self::Ifr
                } else if sm <= 5.0 {
                    Self::Mvfr
                } else {
                    Self::Vfr
                }
            }
        };
        by_ceiling.max(by_visibility)
    }
}

/// Shared forecast element set of a TAF base forecast or change group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TafGroup {
    pub wind: Option<Wind>,
    pub visibility: Option<Visibility>,
    pub weather: Vec<WxPhenomenon>,
    pub clouds: Vec<CloudLayer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TafChangeKind {
    /// `FMddhhmm` — rapid change from the given time.
    Fm,
    /// `BECMG` — gradual change over the window.
    Becmg,
    /// `TEMPO` — temporary fluctuations within the window.
    Tempo,
    /// `PROBnn` — probability of conditions in the window.
    Prob(u8),
    /// `PROBnn TEMPO`.
    ProbTempo(u8),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TafChange {
    pub kind: TafChangeKind,
    pub valid_from: DateTime<Utc>,
    pub valid_to: DateTime<Utc>,
    pub group: TafGroup,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Taf {
    pub raw: String,
    pub station: IcaoCode,
    pub issued_at: DateTime<Utc>,
    pub valid_from: DateTime<Utc>,
    pub valid_to: DateTime<Utc>,
    pub base: TafGroup,
    pub changes: Vec<TafChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SigmetHazard {
    Thunderstorm,
    Turbulence,
    Icing,
    MountainWave,
    VolcanicAsh,
    TropicalCyclone,
    DustStorm,
    Sandstorm,
    RadioactiveCloud,
    /// Raw hazard code without a dedicated variant.
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sigmet {
    /// Issuing FIR identifier, e.g. "EDGG".
    pub fir: String,
    pub hazard: SigmetHazard,
    pub geometry: Polygon,
    pub valid_from: DateTime<Utc>,
    pub valid_to: DateTime<Utc>,
    pub raw: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(amount: CloudAmount, base: u32) -> CloudLayer {
        CloudLayer {
            amount,
            base_ft_agl: Some(base),
            kind: None,
        }
    }

    fn decode_with(clouds: Vec<CloudLayer>, visibility: Option<Visibility>) -> MetarDecode {
        MetarDecode {
            wind: None,
            visibility,
            weather: vec![],
            clouds,
            vertical_visibility_ft: None,
            temperature_c: None,
            dewpoint_c: None,
            qnh: None,
            trend: None,
            auto: false,
            remarks: None,
            unparsed_tokens: vec![],
        }
    }

    #[test]
    fn cavok_unlimited_ceiling_is_vfr() {
        assert_eq!(
            FlightCategory::from_ceiling_and_visibility(None, Visibility::Cavok),
            FlightCategory::Vfr
        );
    }

    #[test]
    fn ceiling_thresholds() {
        let v = Visibility::Meters(9_999);
        let cat = |c| FlightCategory::from_ceiling_and_visibility(Some(c), v);
        assert_eq!(cat(400), FlightCategory::Lifr);
        assert_eq!(cat(500), FlightCategory::Ifr);
        assert_eq!(cat(999), FlightCategory::Ifr);
        assert_eq!(cat(1_000), FlightCategory::Mvfr);
        assert_eq!(cat(3_000), FlightCategory::Mvfr);
        assert_eq!(cat(3_100), FlightCategory::Vfr);
    }

    #[test]
    fn visibility_thresholds() {
        let cat = |m| FlightCategory::from_ceiling_and_visibility(None, Visibility::Meters(m));
        assert_eq!(cat(1_400), FlightCategory::Lifr); // < 1 SM
        assert_eq!(cat(1_610), FlightCategory::Ifr); // just over 1 SM
        assert_eq!(cat(4_000), FlightCategory::Ifr); // < 3 SM
        assert_eq!(cat(5_000), FlightCategory::Mvfr); // 3..=5 SM
        assert_eq!(cat(8_000), FlightCategory::Mvfr); // 4.97 SM
        assert_eq!(cat(9_999), FlightCategory::Vfr);
    }

    #[test]
    fn worse_of_ceiling_and_visibility_governs() {
        assert_eq!(
            FlightCategory::from_ceiling_and_visibility(Some(2_500), Visibility::Meters(1_200)),
            FlightCategory::Lifr
        );
        assert_eq!(
            FlightCategory::from_ceiling_and_visibility(Some(800), Visibility::Meters(9_999)),
            FlightCategory::Ifr
        );
    }

    #[test]
    fn ceiling_is_lowest_broken_or_overcast() {
        let decode = decode_with(
            vec![
                layer(CloudAmount::Few, 1_600),
                layer(CloudAmount::Broken, 4_600),
                layer(CloudAmount::Overcast, 7_000),
            ],
            Some(Visibility::Meters(9_999)),
        );
        assert_eq!(decode.ceiling_ft_agl(), Some(4_600));
        assert_eq!(decode.flight_category(), Some(FlightCategory::Vfr));
    }

    #[test]
    fn vertical_visibility_acts_as_ceiling() {
        let mut decode = decode_with(vec![], Some(Visibility::Meters(300)));
        decode.vertical_visibility_ft = Some(200);
        assert_eq!(decode.ceiling_ft_agl(), Some(200));
        assert_eq!(decode.flight_category(), Some(FlightCategory::Lifr));
    }

    #[test]
    fn unknown_visibility_means_unknown_category() {
        let decode = decode_with(vec![layer(CloudAmount::Broken, 1_200)], None);
        assert_eq!(decode.flight_category(), None);
    }

    #[test]
    fn qnh_conversion() {
        assert_eq!(Qnh::Hpa(1_013).as_hpa(), 1_013.0);
        assert!((Qnh::InHg(29.92).as_hpa() - 1_013.2).abs() < 0.5);
    }
}
