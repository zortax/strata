//! Weather/NOTAM snapshots stored inside the flight document.
//!
//! Opaque-but-versioned: the planning core round-trips the payload
//! verbatim (`serde_json::Value`) and interprets only the timestamp and
//! version. The app's briefing layer owns the payload shape; bumping it
//! means bumping the snapshot's own `format_version`, independent of the
//! flight document version.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Current payload format version written into [`WeatherSnapshot`].
pub const WEATHER_SNAPSHOT_FORMAT_VERSION: u32 = 1;
/// Current payload format version written into [`NotamSnapshot`].
pub const NOTAM_SNAPSHOT_FORMAT_VERSION: u32 = 1;

/// The weather state the flight was planned with (design §3.4 "Weather":
/// snapshot semantics with an explicit timestamp + refresh).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherSnapshot {
    /// Version of `payload`'s shape. Files predating the field are
    /// version 1 (the first shape) — keep the default at 1 forever.
    #[serde(default = "first_version")]
    pub format_version: u32,
    /// When the snapshot was taken (UTC).
    pub taken_at: DateTime<Utc>,
    /// Opaque payload owned by the app's briefing layer.
    #[serde(default)]
    pub payload: Value,
}

impl WeatherSnapshot {
    pub fn new(taken_at: DateTime<Utc>, payload: Value) -> Self {
        Self {
            format_version: WEATHER_SNAPSHOT_FORMAT_VERSION,
            taken_at,
            payload,
        }
    }
}

/// The NOTAM state the flight was planned with; same semantics as
/// [`WeatherSnapshot`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotamSnapshot {
    /// Version of `payload`'s shape; see [`WeatherSnapshot::format_version`].
    #[serde(default = "first_version")]
    pub format_version: u32,
    /// When the snapshot was taken (UTC).
    pub taken_at: DateTime<Utc>,
    /// Opaque payload owned by the app's briefing layer.
    #[serde(default)]
    pub payload: Value,
}

impl NotamSnapshot {
    pub fn new(taken_at: DateTime<Utc>, payload: Value) -> Self {
        Self {
            format_version: NOTAM_SNAPSHOT_FORMAT_VERSION,
            taken_at,
            payload,
        }
    }
}

fn first_version() -> u32 {
    1
}
