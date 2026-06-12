//! ICAO Q-code subject/condition tables (Doc 8126 / Annex 15 vocabulary).
//!
//! The embedded tables cover the codes that matter for a German VFR
//! briefing (movement areas, obstacles, navaids, airspace restrictions,
//! warnings, services); anything outside the table is preserved verbatim
//! as [`QSubject::Other`] / [`QCondition::Other`] — decoding never fails.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! qcode_enum {
    (
        $(#[$meta:meta])*
        $name:ident {
            $($variant:ident => $code:literal, $desc:literal;)+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(into = "String", from = "String")]
        pub enum $name {
            $(#[doc = $desc] $variant,)+
            /// A code outside the embedded table, preserved verbatim.
            Other(String),
        }

        impl $name {
            /// Decodes a two-letter code; unknown codes land in
            /// [`Self::Other`] (never fails).
            pub fn from_code(code: &str) -> Self {
                match code {
                    $($code => Self::$variant,)+
                    other => Self::Other(other.to_owned()),
                }
            }

            /// The two-letter code as transmitted.
            pub fn code(&self) -> &str {
                match self {
                    $(Self::$variant => $code,)+
                    Self::Other(code) => code,
                }
            }

            /// Plain-language meaning, if the code is in the embedded table.
            pub fn description(&self) -> Option<&'static str> {
                match self {
                    $(Self::$variant => Some($desc),)+
                    Self::Other(_) => None,
                }
            }
        }

        impl From<String> for $name {
            fn from(code: String) -> Self {
                Self::from_code(&code)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.code().to_owned()
            }
        }

        impl fmt::Display for $name {
            /// Description when known, otherwise the raw code.
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self.description() {
                    Some(desc) => f.write_str(desc),
                    None => f.write_str(self.code()),
                }
            }
        }
    };
}

qcode_enum! {
    /// Q-code letters 2–3: *what* the NOTAM is about.
    QSubject {
        // AGA — facilities and services
        Aerodrome => "FA", "aerodrome";
        FireFighting => "FF", "fire fighting and rescue service";
        MetService => "FM", "meteorological service";
        FuelAvailability => "FU", "fuel availability";
        Customs => "FZ", "customs/immigration";
        // AGA — movement and landing area
        MovementArea => "MA", "movement area";
        DeclaredDistances => "MD", "declared distances";
        ParkingArea => "MK", "parking area";
        Apron => "MN", "apron";
        Runway => "MR", "runway";
        Stopway => "MS", "stopway";
        Threshold => "MT", "threshold";
        Taxiway => "MX", "taxiway";
        // Obstacles & other information
        AisService => "OA", "aeronautical information service";
        Obstacle => "OB", "obstacle";
        ObstacleLights => "OL", "obstacle lights";
        // Lighting facilities
        ApproachLighting => "LA", "approach lighting system";
        RunwayEdgeLights => "LE", "runway edge lights";
        Papi => "LP", "precision approach path indicator (PAPI)";
        LandingAreaLighting => "LR", "all landing area lighting facilities";
        ThresholdLights => "LT", "threshold lights";
        Vasis => "LV", "visual approach slope indicator system (VASIS)";
        TaxiwayCentreLineLights => "LX", "taxiway centre line lights";
        // ILS
        Ils => "IC", "instrument landing system";
        IlsGlidePath => "IG", "ILS glide path";
        IlsLocalizer => "IL", "ILS localizer";
        // Terrestrial navaids
        AllNavaids => "NA", "all radio navigation facilities";
        Ndb => "NB", "nondirectional radio beacon (NDB)";
        Dme => "ND", "distance measuring equipment (DME)";
        VorDme => "NM", "VOR/DME";
        Tacan => "NN", "TACAN";
        Vortac => "NT", "VORTAC";
        Vor => "NV", "VOR";
        // GNSS
        GnssAirfield => "GA", "GNSS airfield-specific operations";
        GnssAreaWide => "GW", "GNSS area-wide operations";
        // Airspace organization
        MinimumAltitude => "AA", "minimum altitude";
        ControlZone => "AC", "control zone (CTR)";
        ControlArea => "AE", "control area (CTA)";
        FlightInformationRegion => "AF", "flight information region (FIR)";
        ReportingPoint => "AP", "reporting point";
        AtsRoute => "AR", "ATS route";
        TerminalControlArea => "AT", "terminal control area (TMA)";
        AerodromeTrafficZone => "AZ", "aerodrome traffic zone (ATZ)";
        // Airspace restrictions
        AirspaceReservation => "RA", "airspace reservation";
        DangerArea => "RD", "danger area";
        MilitaryOperatingArea => "RM", "military operating area";
        ProhibitedArea => "RP", "prohibited area";
        RestrictedArea => "RR", "restricted area";
        TemporaryRestrictedArea => "RT", "temporary restricted/reserved area";
        // ATM services
        Atis => "SA", "automatic terminal information service (ATIS)";
        AreaControlCentre => "SC", "area control centre (ACC)";
        FlightInformationService => "SE", "flight information service (FIS)";
        Afis => "SF", "aerodrome flight information service (AFIS)";
        ApproachControl => "SP", "approach control service";
        Tower => "ST", "aerodrome control tower (TWR)";
        // Communications & surveillance
        AirGroundFacility => "CA", "air/ground communication facility";
        Ssr => "CS", "secondary surveillance radar (SSR)";
        // Procedures
        Star => "PA", "standard instrument arrival (STAR)";
        StandardVfrArrival => "PB", "standard VFR arrival";
        Sid => "PD", "standard instrument departure (SID)";
        StandardVfrDeparture => "PE", "standard VFR departure";
        InstrumentApproachProcedure => "PI", "instrument approach procedure";
        VfrApproachProcedure => "PK", "VFR approach procedure";
        FlightPlanProcessing => "PL", "flight plan processing";
        NoiseRestriction => "PN", "noise operating restriction";
        TransitionAltitude => "PT", "transition altitude or transition level";
        // Navigation warnings
        AirDisplay => "WA", "air display";
        Aerobatics => "WB", "aerobatics";
        CaptiveBalloon => "WC", "captive balloon or kite";
        Demolition => "WD", "demolition of explosives";
        Exercises => "WE", "exercises";
        GliderFlying => "WG", "glider flying";
        BannerTowing => "WJ", "banner/target towing";
        FreeBalloon => "WL", "ascent of free balloon";
        MissileGunFiring => "WM", "missile, gun or rocket firing";
        ParachuteJumping => "WP", "parachute jumping exercise, paragliding or hang gliding";
        HazardousMaterials => "WR", "radioactive materials or toxic chemicals";
        MassAircraftMovement => "WT", "mass movement of aircraft";
        UnmannedAircraft => "WU", "unmanned aircraft";
        FormationFlight => "WV", "formation flight";
        VolcanicActivity => "WW", "significant volcanic activity";
        ModelFlying => "WZ", "model flying";
        // Special
        PlainLanguage => "XX", "plain-language subject";
        Checklist => "KK", "checklist";
    }
}

qcode_enum! {
    /// Q-code letters 4–5: the *condition* of the subject.
    QCondition {
        // Availability
        WithdrawnForMaintenance => "AC", "withdrawn for maintenance";
        AvailableDaylight => "AD", "available for daylight operation";
        FlightChecked => "AF", "flight checked and found reliable";
        HoursOfService => "AH", "hours of service are now";
        ResumedNormalOperation => "AK", "resumed normal operations";
        SubjectToLimitations => "AL", "operative subject to previously published limitations";
        MilitaryOnly => "AM", "military operations only";
        AvailableNight => "AN", "available for night operation";
        Operational => "AO", "operational";
        PriorPermissionRequired => "AP", "available, prior permission required";
        OnRequest => "AR", "available on request";
        Unserviceable => "AS", "unserviceable";
        NotAvailable => "AU", "not available";
        CompletelyWithdrawn => "AW", "completely withdrawn";
        // Changes
        Activated => "CA", "activated";
        Completed => "CC", "completed";
        Deactivated => "CD", "deactivated";
        Erected => "CE", "erected";
        FrequencyChanged => "CF", "operating frequency(ies) changed to";
        Downgraded => "CG", "downgraded to";
        Changed => "CH", "changed";
        CallSignChanged => "CI", "identification or radio call sign changed to";
        Realigned => "CL", "realigned";
        Displaced => "CM", "displaced";
        Cancelled => "CN", "cancelled";
        Operating => "CO", "operating";
        ReducedPower => "CP", "operating on reduced power";
        TemporarilyReplaced => "CR", "temporarily replaced by";
        Installed => "CS", "installed";
        OnTest => "CT", "on test, do not use";
        // Hazard conditions
        GrassCutting => "HG", "grass cutting in progress";
        HazardDueTo => "HH", "hazard due to";
        LaunchPlanned => "HJ", "launch planned";
        BirdMigration => "HK", "bird migration in progress";
        MarkedBy => "HM", "marked by";
        SnowClearance => "HP", "snow clearance in progress";
        StandingWater => "HR", "standing water";
        LaunchInProgress => "HU", "launch in progress";
        WorkCompleted => "HV", "work completed";
        WorkInProgress => "HW", "work in progress";
        BirdConcentration => "HX", "concentration of birds";
        // Limitations
        ReservedForBasedAircraft => "LB", "reserved for aircraft based therein";
        Closed => "LC", "closed";
        Unsafe => "LD", "unsafe";
        InterferenceFrom => "LF", "interference from";
        WithoutIdentification => "LG", "operating without identification";
        UnserviceableForHeavier => "LH", "unserviceable for aircraft heavier than";
        ClosedToIfr => "LI", "closed to IFR operations";
        ClosedAtNight => "LN", "closed to all night operations";
        ProhibitedTo => "LP", "prohibited to";
        RestrictedToRunwaysAndTaxiways => "LR", "aircraft restricted to runways and taxiways";
        SubjectToInterruption => "LS", "subject to interruption";
        LimitedTo => "LT", "limited to";
        ClosedToVfr => "LV", "closed to VFR operations";
        WillTakePlace => "LW", "will take place";
        CautionAdvised => "LX", "operating but caution advised";
        // Special
        PlainLanguage => "XX", "plain-language condition";
        Trigger => "TT", "trigger NOTAM";
        Checklist => "KK", "checklist";
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_subject_codes_decode() {
        assert_eq!(QSubject::from_code("MR"), QSubject::Runway);
        assert_eq!(QSubject::from_code("OB"), QSubject::Obstacle);
        assert_eq!(QSubject::from_code("RR"), QSubject::RestrictedArea);
        assert_eq!(QSubject::from_code("GW"), QSubject::GnssAreaWide);
        assert_eq!(QSubject::from_code("WP"), QSubject::ParachuteJumping);
    }

    #[test]
    fn known_condition_codes_decode() {
        assert_eq!(QCondition::from_code("LC"), QCondition::Closed);
        assert_eq!(QCondition::from_code("AS"), QCondition::Unserviceable);
        assert_eq!(QCondition::from_code("CA"), QCondition::Activated);
        assert_eq!(QCondition::from_code("LW"), QCondition::WillTakePlace);
        assert_eq!(QCondition::from_code("XX"), QCondition::PlainLanguage);
    }

    #[test]
    fn unknown_codes_fall_back_to_other_verbatim() {
        let subject = QSubject::from_code("FB");
        assert_eq!(subject, QSubject::Other("FB".to_owned()));
        assert_eq!(subject.code(), "FB");
        assert_eq!(subject.description(), None);
        assert_eq!(subject.to_string(), "FB");

        let condition = QCondition::from_code("HA");
        assert_eq!(condition, QCondition::Other("HA".to_owned()));
        assert_eq!(condition.code(), "HA");
    }

    #[test]
    fn round_trip_code_for_every_table_entry_is_stable() {
        for code in ["MR", "MX", "FA", "OB", "RR", "RD", "GW", "NV", "ST"] {
            assert_eq!(QSubject::from_code(code).code(), code);
        }
        for code in ["LC", "AS", "AU", "CA", "CE", "HW", "LW", "TT", "KK"] {
            assert_eq!(QCondition::from_code(code).code(), code);
        }
    }

    #[test]
    fn display_uses_description_when_known() {
        assert_eq!(QSubject::Runway.to_string(), "runway");
        assert_eq!(QCondition::Closed.to_string(), "closed");
    }

    #[test]
    fn serde_round_trips_as_code_string() {
        let json = serde_json::to_string(&QSubject::Runway).expect("serialize");
        assert_eq!(json, "\"MR\"");
        let back: QSubject = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, QSubject::Runway);

        let other: QSubject = serde_json::from_str("\"ZZ\"").expect("deserialize");
        assert_eq!(other, QSubject::Other("ZZ".to_owned()));
    }
}
