//! Compilation and PDF export.
//!
//! [`render_briefing`] is the crate's single entry point: serialize the
//! input to JSON, compile the embedded template against the self-contained
//! [`BriefWorld`](crate::world::BriefWorld), export with `typst-pdf`.
//! Deterministic by construction — the only timestamp involved is the
//! caller-provided [`BriefingInput::generated_at`], which both the template
//! and the PDF metadata use (there is a render-twice equality test).

#[cfg(test)]
mod tests;

use chrono::{DateTime, Datelike, Timelike, Utc};
use thiserror::Error;
use typst::diag::{SourceDiagnostic, Warned};
use typst::foundations::{Datetime, Smart};
use typst::layout::PagedDocument;
use typst_pdf::{PdfOptions, Timestamp};

use crate::input::BriefingInput;
use crate::world::BriefWorld;

/// Errors from briefing rendering.
///
/// `Compile`/`Export` carry the formatted typst diagnostics: with an
/// embedded, tested template they indicate a template bug (or wildly
/// out-of-contract input), not a user mistake.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BriefError {
    #[error("failed to serialize briefing input: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("briefing template failed to compile: {0}")]
    Compile(String),
    #[error("briefing PDF export failed: {0}")]
    Export(String),
}

/// Renders the briefing document to PDF bytes.
///
/// Pure function of the input: the same [`BriefingInput`] (including its
/// `generated_at` timestamp) produces the same bytes. No filesystem or
/// network access happens at render time.
pub fn render_briefing(input: &BriefingInput) -> Result<Vec<u8>, BriefError> {
    let document = compile(input)?;
    let options = PdfOptions {
        ident: Smart::Custom("strata-briefing"),
        timestamp: pdf_timestamp(input.generated_at),
        ..PdfOptions::default()
    };
    typst_pdf::pdf(&document, &options)
        .map_err(|diagnostics| BriefError::Export(format_diagnostics(&diagnostics)))
}

/// Compiles the template into a paged document. Separate from the PDF step
/// so tests can introspect the layout (page count, laid-out text) without
/// parsing PDF content streams.
pub(crate) fn compile(input: &BriefingInput) -> Result<PagedDocument, BriefError> {
    let json = serde_json::to_string(input)?;
    let world = BriefWorld::new(json, input.generated_at);
    let Warned { output, warnings } = typst::compile::<PagedDocument>(&world);
    for warning in &warnings {
        tracing::warn!(message = %warning.message, "typst template warning");
    }
    output.map_err(|diagnostics| BriefError::Compile(format_diagnostics(&diagnostics)))
}

/// The PDF metadata timestamp: the caller-provided generation time as UTC.
fn pdf_timestamp(at: DateTime<Utc>) -> Option<Timestamp> {
    let datetime = Datetime::from_ymd_hms(
        at.year(),
        at.month() as u8,
        at.day() as u8,
        at.hour() as u8,
        at.minute() as u8,
        at.second() as u8,
    )?;
    Some(Timestamp::new_utc(datetime))
}

/// One line per diagnostic, with hints appended.
fn format_diagnostics(diagnostics: &[SourceDiagnostic]) -> String {
    let lines: Vec<String> = diagnostics
        .iter()
        .map(|d| {
            let mut line = d.message.to_string();
            for hint in &d.hints {
                line.push_str(&format!(" (hint: {hint})"));
            }
            line
        })
        .collect();
    lines.join("; ")
}
