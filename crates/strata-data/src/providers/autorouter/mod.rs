//! autorouter.aero NOTAM provider (`https://api.autorouter.aero`).
//!
//! **License**: the autorouter API is licensed for *end-user* use only —
//! data fetched through it serves the local user's briefing and must never
//! be redistributed. Note this on the settings "Data sources" page.
//!
//! [`AutorouterClient`] is the app's NOTAM source. Authentication follows
//! <https://www.autorouter.aero/wiki/api/authentication/> exactly (page
//! verified 2026-06): OAuth2 client-credentials against
//! `POST /v1.0/oauth2/token` with the **account email/password** as
//! `client_id`/`client_secret`, yielding a one-hour Bearer token. NOTAMs
//! come from `GET /v1.0/notam` with a JSON `itemas` location list,
//! validity-window epochs and offset/limit pagination
//! (<https://www.autorouter.aero/wiki/api/>).
//!
//! Credentials come from the app config's `[autorouter]` section, passed
//! into the constructor — this crate never reads the environment, and the
//! secret is never logged. Without credentials the app has **no** NOTAM
//! provider (the Briefing tab says so honestly); there is no runtime
//! fixture fallback.
//!
//! [`FixtureNotamProvider`] — a hand-built corpus of realistic German
//! NOTAMs embedded at compile time — exists for **unit tests only**
//! (decoder/relevance/briefing tests). It is compiled under `cfg(test)`
//! and the `test-support` feature; dependent crates enable the feature
//! from their `[dev-dependencies]` so it can never reach a runtime build.

mod client;
#[cfg(any(test, feature = "test-support"))]
mod fixture;
mod rows;

pub use client::AutorouterClient;
#[cfg(any(test, feature = "test-support"))]
pub use fixture::FixtureNotamProvider;

/// Provider label used in [`crate::Error::Provider`] and log events.
const PROVIDER: &str = "autorouter";
