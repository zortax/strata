//! Rendering tests (plan §7 "PDF: golden-file smoke — renders, page count,
//! contains disclaimer — not pixel-exact").
//!
//! Text assertions use **typst's own layout introspection**: the compiled
//! [`PagedDocument`] is walked frame-by-frame and every laid-out text run
//! collected. That checks the real, post-layout document without a PDF
//! text-extraction dependency (PDF content streams are compressed and
//! subset-encoded; parsing them back would test the extractor, not us).

use typst::layout::{Frame, FrameItem, PagedDocument};

use super::{compile, render_briefing};
use crate::fixtures::{full_input, minimal_input};

/// All text runs of one page, in layout order, whitespace-normalized.
/// Runs split at style/font boundaries (e.g. the "→" falling back to the
/// mono font mid-line), so consecutive whitespace collapses to one space
/// to keep phrases assertable.
fn page_text(frame: &Frame) -> String {
    fn collect(frame: &Frame, out: &mut String) {
        for (_, item) in frame.items() {
            match item {
                FrameItem::Group(group) => collect(&group.frame, out),
                FrameItem::Text(text) => {
                    out.push_str(&text.text);
                    out.push(' ');
                }
                _ => {}
            }
        }
    }
    let mut out = String::new();
    collect(frame, &mut out);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn document_text(document: &PagedDocument) -> String {
    document
        .pages
        .iter()
        .map(|page| page_text(&page.frame))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn full_briefing_renders_a_pdf() {
    let pdf = render_briefing(&full_input()).expect("full briefing renders");
    assert!(pdf.starts_with(b"%PDF"), "output must be a PDF document");
    assert!(pdf.len() > 10_000, "a full briefing is not a stub PDF");
}

#[test]
fn full_briefing_lays_out_at_least_three_pages() {
    let document = compile(&full_input()).expect("full briefing compiles");
    assert!(
        document.pages.len() >= 3,
        "full briefing should span at least 3 pages, got {}",
        document.pages.len()
    );
}

#[test]
fn disclaimer_appears_on_every_page() {
    let document = compile(&full_input()).expect("full briefing compiles");
    for (index, page) in document.pages.iter().enumerate() {
        let text = page_text(&page.frame);
        assert!(
            text.contains("NOT FOR NAVIGATION"),
            "page {} is missing the disclaimer",
            index + 1
        );
    }
}

#[test]
fn full_briefing_contains_every_section_with_its_data() {
    let document = compile(&full_input()).expect("full briefing compiles");
    let text = document_text(&document);

    // Cover block.
    assert!(text.contains("Bavaria Test Hop"));
    assert!(text.contains("EDMA → DON → TRU → ROT → EDDN"));
    assert!(text.contains("D-EABC"));
    assert!(text.contains("2026-06-12 08:00Z"));
    // Nav log: TOC/TOD rows, a leg value, the totals.
    assert!(text.contains("NAV LOG"));
    assert!(text.contains("TOC") && text.contains("TOD"));
    assert!(text.contains("Langen Info 128.950"));
    assert!(text.contains("Expect RWY 28"));
    // Fuel ladder + verdict.
    assert!(text.contains("FUEL PLAN"));
    assert!(text.contains("Final reserve"));
    assert!(text.contains("Minimum required"));
    assert!(text.contains("Margin +61 L"));
    // W&B: loading, states, verdict.
    assert!(text.contains("WEIGHT & BALANCE"));
    assert!(text.contains("Empty aircraft"));
    assert!(text.contains("WITHIN"));
    assert!(text.contains("All loading states are within the certified envelope."));
    // Weather: raw + decoded METAR/TAF, winds aloft, freezing level.
    assert!(text.contains("WEATHER"));
    assert!(text.contains("EDMA 110920Z 24008KT 9999 FEW035 SCT100 18/09 Q1021"));
    assert!(text.contains("QNH 1021"));
    assert!(text.contains("No METAR/TAF available for this aerodrome."));
    assert!(text.contains("9800 ft AMSL"));
    // Provenance caveats render visibly with their sections.
    assert!(text.contains("Winds aloft: ISA estimate — no forecast data."));
    assert!(text.contains("Built-in sample NOTAMs — not a real briefing."));
    // NOTAM cards: id, relevance, summary, raw.
    assert!(text.contains("NOTAMS"));
    assert!(text.contains("B0612/26"));
    assert!(text.contains("Route corridor, 2 NM off track"));
    assert!(text.contains("TWY D CLSD DUE TO MAINT, USE TWY C"));
}

/// `None` source notes render no caveat — the absence is as deliberate as
/// the presence.
#[test]
fn absent_source_notes_render_no_caveats() {
    let mut input = full_input();
    if let Some(weather) = &mut input.weather {
        weather.winds_source_note = None;
    }
    if let Some(notams) = &mut input.notams {
        notams.source_note = None;
    }
    let document = compile(&input).expect("briefing compiles");
    let text = document_text(&document);
    assert!(!text.contains("Winds aloft: ISA estimate"));
    assert!(!text.contains("Built-in sample NOTAMs"));
}

#[test]
fn missing_sections_render_with_honest_unavailable_lines() {
    let document = compile(&minimal_input()).expect("minimal briefing compiles");
    let text = document_text(&document);

    // Every section heading is still present...
    for heading in [
        "NAV LOG",
        "FUEL PLAN",
        "WEIGHT & BALANCE",
        "WEATHER",
        "NOTAMS",
    ] {
        assert!(
            text.contains(heading),
            "missing section heading {heading:?}"
        );
    }
    // ...with an explicit unavailability note, never silently absent.
    assert!(text.contains("Nav log not available"));
    assert!(text.contains("Fuel plan not available"));
    assert!(text.contains("Weight & balance not available"));
    assert!(text.contains("Weather briefing not available"));
    assert!(text.contains("NOTAM briefing not available"));
    // And the disclaimer still stands.
    assert!(text.contains("NOT FOR NAVIGATION"));
}

#[test]
fn empty_notam_list_is_distinct_from_unavailable() {
    let mut input = full_input();
    input.notams = Some(crate::input::NotamSection {
        snapshot_time: input.notams.as_ref().and_then(|n| n.snapshot_time),
        notams: Vec::new(),
        source_note: None,
    });
    let document = compile(&input).expect("briefing compiles");
    let text = document_text(&document);
    assert!(text.contains("No relevant NOTAMs for this route and validity window."));
    assert!(!text.contains("NOTAM briefing not available"));
}

#[test]
fn rendering_is_deterministic() {
    let input = full_input();
    let first = render_briefing(&input).expect("renders");
    let second = render_briefing(&input).expect("renders");
    assert_eq!(first, second, "same input must produce identical PDF bytes");
}

#[test]
fn minimal_briefing_renders_a_pdf() {
    let pdf = render_briefing(&minimal_input()).expect("minimal briefing renders");
    assert!(pdf.starts_with(b"%PDF"));
}
