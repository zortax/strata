# strata-brief

Briefing PDF generation for Strata's flight planner (implementation plan §1,
design §4 "Briefing PDF"). One public function:

```rust
pub fn render_briefing(input: &BriefingInput) -> Result<Vec<u8>, BriefError>
```

`BriefingInput` (`src/input/mod.rs`) is a tree of **plain serializable
data** — strings, `f64` quantities with the unit in the field name,
`chrono` timestamps. No `strata-plan` types appear in the contract; the app
converts its computed flight into this shape, so the crate is reusable by
any frontend.

## How it renders

[typst](https://typst.app) is used **as a library** (`typst` +
`typst-pdf`, currently 0.14.x). The pieces:

- **`src/world/mod.rs`** — a minimal `typst::World`: the template and fonts
  are embedded via `include_str!`/`include_bytes!`, the input is passed as
  JSON through `sys.inputs.data`, and every other file request is answered
  `NotFound`. **No filesystem or network access at render time.**
  `datetime.today()` is backed by the caller-provided generation timestamp.
- **`assets/briefing.typ`** — the embedded template: cover block (flight
  facts, route summary, the NOT FOR NAVIGATION disclaimer), landscape nav
  log with TOC/TOD rows and totals, fuel ladder with verdict, weight &
  balance tables **plus the drawn CG-envelope figure** (polygon, fuel-burn
  track, per-state points), weather (raw + decoded METAR/TAF, winds aloft,
  freezing level) and NOTAM cards. Every section renders unconditionally —
  missing data produces an explicit "not available" line. The disclaimer
  also repeats in the footer of every page.
- **Determinism:** the same `BriefingInput` (including `generated_at`)
  produces byte-identical PDFs — the timestamp is part of the input, the
  PDF ident and metadata timestamp are fixed from it, and a render-twice
  test pins this down.

## Fonts

Vendored in `assets/fonts/`, both licensed under the **SIL Open Font
License 1.1** (full text + copyright notices in `assets/fonts/OFL.txt`):

| Family | Files | Use |
|---|---|---|
| Noto Sans | Regular, Bold, Italic | body text |
| JetBrains Mono | Regular, Bold | raw METAR/TAF/NOTAM text, NOTAM ids |

Noto Sans (latin-greek-cyrillic build) lacks a few symbols (e.g. `→`);
typst's font-book fallback resolves those from JetBrains Mono.

## Testing approach

PDF assertions are deliberately *not* pixel-exact (plan §7): the smoke test
checks the `%PDF` magic and that a full briefing lays out ≥ 3 pages. Text
assertions (disclaimer on every page, section content, honest
"not available" lines) use **typst's own layout introspection** — the
compiled `PagedDocument` is walked frame-by-frame and the laid-out text
runs collected. That verifies the real post-layout document without a PDF
text-extraction dev-dependency (content streams are compressed and
subset-encoded; parsing them back would mostly test the extractor).

## Build-time note

typst is by far the heaviest dependency in the workspace (~250 transitive
crates; tens of seconds of extra cold-build time). That is the reason this
crate exists separately: only `strata-app` links it, and a template or
input change rebuilds `strata-brief` alone in seconds.
