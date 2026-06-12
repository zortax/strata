// Strata VFR briefing template.
//
// Data arrives as one JSON document via `sys.inputs.data`; the contract is
// `BriefingInput` in src/input/mod.rs (units in field-name suffixes,
// altitudes pre-formatted, timestamps RFC3339 UTC). Every section renders
// unconditionally — missing data shows an honest "not available" line.
// The NOT FOR NAVIGATION disclaimer appears prominently on the cover and
// in the footer of every page.

#let data = json(bytes(sys.inputs.data))

// ── design tokens ───────────────────────────────────────────────────────
#let ink = rgb("#1c2024")
#let muted = rgb("#5f6b76")
#let accent = rgb("#1f4e79")
#let hairline = luma(208)
#let panel = luma(247)
#let good = rgb("#1e7d3c")
#let bad = rgb("#b3261e")
#let warn-fill = rgb("#fdecec")

#let sans = "Noto Sans"
#let mono-font = "JetBrains Mono"

// ── formatting helpers ──────────────────────────────────────────────────
// Quantities are numbers-or-none; none renders as an em dash.
#let or-dash(s) = if s == none { "—" } else { s }
#let f0(x) = if x == none { "—" } else { str(int(calc.round(x))) }
#let f1(x) = if x == none { "—" } else { str(calc.round(x, digits: 1)) }
#let qty(x, unit) = if x == none { "—" } else { f1(x) + " " + unit }
// Degrees, three digits, 360 for north (PLOG convention).
#let deg3(x) = if x == none { "—" } else {
  let v = calc.rem(calc.rem(int(calc.round(x)), 360) + 360, 360)
  let v = if v == 0 { 360 } else { v }
  let s = str(v)
  "0" * (3 - s.len()) + s + "°"
}
// Signed degrees (wind correction angle).
#let sdeg(x) = if x == none { "—" } else {
  let v = int(calc.round(x))
  (if v >= 0 { "+" } else { "" }) + str(v) + "°"
}
// Minutes as h:mm.
#let hmm(min) = if min == none { "—" } else {
  let total = int(calc.round(min))
  let h = int(total / 60)
  let m = total - h * 60
  str(h) + ":" + (if m < 10 { "0" } else { "" }) + str(m)
}
// RFC3339 UTC timestamp → "2026-06-11 09:30Z" / "09:30Z".
#let fmt-dt(s) = if s == none { "—" } else { s.slice(0, 16).replace("T", " ") + "Z" }
#let fmt-tm(s) = if s == none { "—" } else { s.slice(11, 16) }
// Leg wind "240/15".
#let fmt-wv(w) = if w == none { "—" } else { deg3(w.direction_deg).slice(0, 3) + "/" + f0(w.speed_kt) }
// Temperature "+15 °C".
#let fmt-temp(t) = if t == none { "—" } else {
  let v = calc.round(t, digits: 1)
  (if v >= 0 { "+" } else { "" }) + str(v) + " °C"
}

#let mono(body, size: 7pt) = text(font: mono-font, size: size, body)
// Raw reports keep their line structure (strings in content would collapse
// newlines to spaces).
#let preserve-lines(s) = s.split("\n").map(l => [#l]).join(linebreak())
#let mono-block(s, size: 7pt) = block(
  width: 100%,
  fill: panel,
  inset: 7pt,
  radius: 2pt,
  above: 5pt,
  below: 7pt,
  mono(preserve-lines(s), size: size),
)
#let unavailable(msg) = block(inset: (y: 3pt), text(style: "italic", fill: muted, msg))
// Visible provenance caveat (sample data, ISA fallbacks, …) — must not be
// mistakable for body text.
#let caveat(msg) = block(
  width: 100%,
  above: 5pt,
  below: 5pt,
  fill: warn-fill,
  stroke: 0.75pt + bad,
  radius: 2pt,
  inset: (x: 7pt, y: 5pt),
  text(size: 8pt, weight: "bold", fill: bad, msg),
)
#let snapshot-line(t) = if t != none {
  block(above: 2pt, text(size: 7.5pt, fill: muted, "Snapshot taken " + fmt-dt(t) + " — the flight stores the snapshot it was planned with."))
}
#let sub-label(s) = block(above: 8pt, below: 4pt, text(size: 8pt, weight: "bold", fill: muted, tracking: 0.6pt, upper(s)))

// ── document & page setup ───────────────────────────────────────────────
#set document(title: "VFR briefing — " + data.flight.name, author: "Strata")
#set text(font: sans, size: 9pt, fill: ink, hyphenate: false)
#set par(leading: 0.55em)
#set page(
  paper: "a4",
  margin: (x: 1.7cm, top: 1.6cm, bottom: 2.1cm),
  footer: context {
    set text(size: 7pt, fill: muted)
    line(length: 100%, stroke: 0.5pt + hairline)
    v(3pt)
    grid(
      columns: (auto, 1fr, auto),
      column-gutter: 8pt,
      align: (left, center, right),
      text(weight: "bold", fill: bad, "NOT FOR NAVIGATION"),
      [#data.flight.name · generated #fmt-dt(data.generated_at)],
      counter(page).display("1 / 1", both: true),
    )
  },
)
#set heading(numbering: none)
#show heading.where(level: 1): it => {
  v(6pt)
  text(size: 12.5pt, weight: "bold", fill: ink, tracking: 0.4pt, upper(it.body))
  v(-7pt)
  line(length: 100%, stroke: 0.75pt + accent)
  v(2pt)
}

// ── cover block ─────────────────────────────────────────────────────────
#{
  set text(size: 8pt, fill: muted)
  grid(
    columns: (1fr, auto),
    text(tracking: 1.4pt, upper("Strata · VFR flight briefing")),
    text("generated " + fmt-dt(data.generated_at)),
  )
}
#v(2pt)
#text(size: 22pt, weight: "bold", data.flight.name)
#v(2pt)
#text(size: 12.5pt, fill: accent, data.flight.route.join(" → "))
#if data.flight.alternate != none [
  #v(-2pt)
  #text(size: 9pt, fill: muted, "Alternate: " + data.flight.alternate)
]
#v(8pt)

#let fact(label, value) = stack(
  spacing: 3.5pt,
  text(size: 7pt, fill: muted, tracking: 0.8pt, upper(label)),
  text(size: 10.5pt, value),
)
#grid(
  columns: (1fr, 1fr, 1fr, 1fr),
  row-gutter: 12pt,
  column-gutter: 8pt,
  fact("Aircraft", or-dash(data.flight.aircraft_type)),
  fact("Registration", or-dash(data.flight.registration)),
  fact("Callsign", or-dash(data.flight.callsign)),
  fact("Departure (UTC)", fmt-dt(data.flight.departure_time)),
  fact("Cruise altitude", or-dash(data.flight.cruise_altitude)),
  fact("Distance", qty(data.flight.total_distance_nm, "NM")),
  fact("Time en route", hmm(data.flight.total_ete_minutes)),
  fact("Trip fuel", qty(data.flight.total_fuel_liters, "L")),
)

#v(10pt)
#block(width: 100%, fill: warn-fill, stroke: 1pt + bad, radius: 3pt, inset: 10pt)[
  #text(weight: "bold", fill: bad, size: 11.5pt, tracking: 0.6pt)[NOT FOR NAVIGATION]
  #v(-3pt)
  #text(size: 8.5pt)[
    This document was generated by Strata for flight-planning support only.
    It is not an official briefing. Verify all information against current
    official sources (AIP, NOTAM, MET) before flight. Performance, fuel and
    weight-and-balance figures derive from user-entered aircraft data;
    fuel-policy values are templates, not regulatory guidance. The pilot in
    command remains solely responsible for the conduct of the flight.
  ]
]

#if data.flight.remarks != none {
  sub-label("Remarks")
  text(size: 9pt, data.flight.remarks)
}

// ── nav log ─────────────────────────────────────────────────────────────
#let navlog-table(nav) = {
  set text(features: ("tnum",))
  let header = (
    [Waypoint], [Altitude], [TT], [MT], [MH], [W/V], [WCA], [TAS#linebreak()kt],
    [GS#linebreak()kt], [Dist#linebreak()NM], [ETE#linebreak()min], [ETA#linebreak()UTC],
    [Fuel#linebreak()L], [Rem#linebreak()L], [Frequency], [Notes],
  ).map(c => text(size: 7pt, weight: "bold", fill: muted, c))
  let cells = ()
  for row in nav.rows {
    let special = row.kind != "waypoint"
    let row-cells = (
      if special {
        text(fill: accent, style: "italic", weight: "bold", row.label)
      } else {
        text(weight: "bold", row.label)
      },
      or-dash(row.altitude),
      deg3(row.true_track_deg),
      deg3(row.magnetic_track_deg),
      deg3(row.magnetic_heading_deg),
      fmt-wv(row.wind),
      sdeg(row.wind_correction_angle_deg),
      f0(row.tas_kt),
      f0(row.ground_speed_kt),
      f1(row.distance_nm),
      f0(row.ete_minutes),
      fmt-tm(row.eta),
      f1(row.leg_fuel_liters),
      f1(row.remaining_fuel_liters),
      or-dash(row.frequency),
      row.notes,
    )
    if special {
      cells += row-cells.map(c => table.cell(fill: accent.transparentize(92%))[#c])
    } else {
      cells += row-cells.map(c => [#c])
    }
  }
  let bold(c) = text(weight: "bold")[#c]
  let totals = (
    (bold("Totals"),) + ([],) * 8
      + (bold(f1(nav.total_distance_nm)), bold(hmm(nav.total_ete_minutes)), [])
      + (bold(f1(nav.total_fuel_liters)),) + ([],) * 3
  )
  table(
    columns: (auto, auto, auto, auto, auto, auto, auto, auto, auto, auto, auto, auto, auto, auto, auto, 1fr),
    align: (left, left, right, right, right, right, right, right, right, right, right, right, right, right, left, left),
    stroke: none,
    inset: (x: 4pt, y: 3.5pt),
    table.header(..header),
    table.hline(stroke: 0.75pt + ink),
    ..cells,
    table.hline(stroke: 0.75pt + ink),
    ..totals,
  )
}

#if data.navlog == none {
  [= Nav log]
  unavailable("Nav log not available — the flight has not been computed.")
} else {
  // The PLOG goes landscape: sixteen columns breathe better that way.
  set page(flipped: true)
  set text(size: 7.5pt)
  [= Nav log]
  navlog-table(data.navlog)
  text(
    size: 7pt,
    fill: muted,
    "Leg values describe the interval arriving at each checkpoint. TOC/TOD rows split their legs; distances sum to the route total. ETE is wind-corrected; fuel follows the phase plan.",
  )
}

// ── fuel plan ───────────────────────────────────────────────────────────
#let fuel-table(f) = {
  set text(features: ("tnum",))
  let row(label, liters) = ([#label], [#f1(liters)])
  table(
    columns: (1fr, auto),
    align: (left, right),
    stroke: none,
    inset: (x: 4pt, y: 3.5pt),
    table.header(
      text(size: 7pt, weight: "bold", fill: muted)[Ladder], text(size: 7pt, weight: "bold", fill: muted)[Liters],
    ),
    table.hline(stroke: 0.75pt + ink),
    ..row("Taxi", f.taxi_liters),
    ..row("Trip", f.trip_liters),
    ..row("Contingency", f.contingency_liters),
    ..row("Alternate", f.alternate_liters),
    ..row("Final reserve", f.final_reserve_liters),
    ..row("Extra", f.extra_liters),
    table.hline(stroke: 0.5pt + hairline),
    text(weight: "bold")[Minimum required], text(weight: "bold")[#f1(f.minimum_required_liters)],
    [Loaded fuel], [#f1(f.loaded_liters)],
    table.hline(stroke: 0.75pt + ink),
    {
      let c = if f.margin_liters < 0 { bad } else { good }
      text(weight: "bold", fill: c)[Margin]
    },
    {
      let c = if f.margin_liters < 0 { bad } else { good }
      text(weight: "bold", fill: c)[#f1(f.margin_liters)]
    },
  )
}

#let fuel-verdict(f) = {
  let under = f.margin_liters < 0
  let c = if under { bad } else { good }
  block(width: 100%, fill: c.transparentize(92%), stroke: 0.75pt + c, radius: 3pt, inset: 9pt, {
    text(size: 8pt, fill: muted, tracking: 0.8pt, upper("Fuel verdict"))
    linebreak()
    text(size: 13pt, weight: "bold", fill: c,
      if under { "UNDER MINIMUM by " + f1(-f.margin_liters) + " L" }
      else { "Margin +" + f1(f.margin_liters) + " L" })
    if f.endurance_minutes != none {
      linebreak()
      text(size: 9pt, "Endurance " + hmm(f.endurance_minutes) + " h at planned cruise")
    }
  })
  if f.policy_note != none {
    block(above: 6pt, text(size: 7.5pt, style: "italic", fill: muted, f.policy_note))
  }
}

= Fuel plan
#if data.fuel == none {
  unavailable("Fuel plan not available — the flight has not been computed.")
} else {
  grid(columns: (1.1fr, 1fr), column-gutter: 16pt, fuel-table(data.fuel), fuel-verdict(data.fuel))
}

// ── weight & balance ────────────────────────────────────────────────────
#let wb-loading-table(rows) = {
  set text(features: ("tnum",))
  let total-mass = rows.map(r => r.mass_kg).sum(default: 0)
  let total-moment = rows.map(r => r.mass_kg * r.arm_m).sum(default: 0)
  let total-arm = if total-mass > 0 { total-moment / total-mass } else { none }
  let hdr(c) = text(size: 7pt, weight: "bold", fill: muted, c)
  table(
    columns: (1fr, auto, auto, auto),
    align: (left, right, right, right),
    stroke: none,
    inset: (x: 4pt, y: 3.5pt),
    table.header(hdr[Station], hdr[Mass kg], hdr[Arm m], hdr[Moment kg·m]),
    table.hline(stroke: 0.75pt + ink),
    ..rows.map(r => ([#r.station], [#f1(r.mass_kg)], [#str(calc.round(r.arm_m, digits: 3))], [#f1(r.mass_kg * r.arm_m)])).flatten(),
    table.hline(stroke: 0.75pt + ink),
    text(weight: "bold")[Total (ramp)],
    text(weight: "bold")[#f1(total-mass)],
    text(weight: "bold")[#if total-arm == none { "—" } else { str(calc.round(total-arm, digits: 3)) }],
    text(weight: "bold")[#f1(total-moment)],
  )
}

#let wb-states-table(states) = {
  set text(features: ("tnum",))
  let hdr(c) = text(size: 7pt, weight: "bold", fill: muted, c)
  table(
    columns: (1fr, auto, auto, auto),
    align: (left, right, right, right),
    stroke: none,
    inset: (x: 4pt, y: 3.5pt),
    table.header(hdr[State], hdr[Mass kg], hdr[CG arm m], hdr[Envelope]),
    table.hline(stroke: 0.75pt + ink),
    ..states
      .map(s => (
        [#s.label],
        [#f1(s.mass_kg)],
        [#str(calc.round(s.cg_arm_m, digits: 3))],
        if s.within_limits { text(weight: "bold", fill: good)[WITHIN] } else { text(weight: "bold", fill: bad)[OUTSIDE] },
      ))
      .flatten(),
  )
}

// The CG envelope figure: polygon + fuel-burn track + per-state points in
// (arm, mass) space, linearly scaled into a fixed box.
#let cg-figure(wb) = {
  let state-points = wb.states.map(s => (arm_m: s.cg_arm_m, mass_kg: s.mass_kg))
  let all = wb.envelope + wb.burn_track + state-points
  let xs = all.map(p => p.arm_m)
  let ys = all.map(p => p.mass_kg)
  let xpad = calc.max((calc.max(..xs) - calc.min(..xs)) * 0.10, 0.001)
  let ypad = calc.max((calc.max(..ys) - calc.min(..ys)) * 0.10, 1)
  let xmin = calc.min(..xs) - xpad
  let xmax = calc.max(..xs) + xpad
  let ymin = calc.min(..ys) - ypad
  let ymax = calc.max(..ys) + ypad
  let w = 220pt
  let h = 150pt
  let px(v) = (v - xmin) / (xmax - xmin) * w
  let py(v) = h - (v - ymin) / (ymax - ymin) * h
  let at(p) = (px(p.arm_m), py(p.mass_kg))
  box(width: w, height: h, {
    place(rect(width: w, height: h, stroke: 0.5pt + hairline, fill: white))
    place(polygon(fill: accent.transparentize(90%), stroke: 1pt + accent, ..wb.envelope.map(at)))
    if wb.burn_track.len() >= 2 {
      place(curve(
        stroke: (paint: muted, thickness: 0.8pt, dash: "dashed"),
        curve.move(at(wb.burn_track.first())),
        ..wb.burn_track.slice(1).map(p => curve.line(at(p))),
      ))
    }
    for s in wb.states {
      let c = if s.within_limits { good } else { bad }
      let x = px(s.cg_arm_m)
      let y = py(s.mass_kg)
      place(dx: x - 2pt, dy: y - 2pt, circle(radius: 2pt, fill: c))
      place(dx: x + 4pt, dy: y - 8pt, text(size: 6.5pt, weight: "bold", fill: c, s.label))
    }
    place(dx: 3pt, dy: 3pt, text(size: 6pt, fill: muted, "mass kg ↑ / CG arm m →"))
    place(dx: 3pt, dy: h - 10pt, text(size: 6pt, fill: muted, str(calc.round(xmin, digits: 2))))
    place(dx: w - 22pt, dy: h - 10pt, text(size: 6pt, fill: muted, str(calc.round(xmax, digits: 2))))
  })
}

= Weight & balance
#if data.weight_balance == none {
  unavailable("Weight & balance not available — no loading scenario has been computed.")
} else {
  let wb = data.weight_balance
  let all-within = wb.states.all(s => s.within_limits)
  if wb.states.len() > 0 {
    block(above: 6pt, below: 6pt,
      if all-within { text(weight: "bold", fill: good, "All loading states are within the certified envelope.") }
      else { text(weight: "bold", fill: bad, "OUT OF ENVELOPE — at least one loading state lies outside certified limits.") })
  }
  grid(
    columns: (1.15fr, 1fr),
    column-gutter: 16pt,
    {
      sub-label("Loading")
      if wb.loading.len() == 0 { unavailable("No loading table available.") } else { wb-loading-table(wb.loading) }
      sub-label("Mass & CG states")
      if wb.states.len() == 0 { unavailable("No computed W&B states available.") } else { wb-states-table(wb.states) }
    },
    {
      sub-label("CG envelope")
      if wb.envelope.len() < 3 {
        unavailable("Envelope figure not available — the aircraft profile has no CG envelope.")
      } else {
        cg-figure(wb)
        block(above: 4pt, text(size: 6.5pt, fill: muted, "Dashed: CG travel as fuel burns, takeoff → zero fuel."))
      }
    },
  )
  if wb.notes != none {
    block(above: 6pt, text(size: 7.5pt, style: "italic", fill: muted, wb.notes))
  }
}

// ── weather ─────────────────────────────────────────────────────────────
#let category-badge(cat) = if cat != none {
  let c = if cat == "VFR" { good } else if cat == "MVFR" { rgb("#1a6fb5") } else if cat == "IFR" { bad } else if cat == "LIFR" { rgb("#7b2d8b") } else { muted }
  box(fill: c.transparentize(86%), stroke: 0.5pt + c, radius: 2pt, inset: (x: 5pt, y: 2.5pt), text(size: 7.5pt, weight: "bold", fill: c, cat))
}

#let aerodrome-weather(a) = block(width: 100%, above: 11pt, {
  grid(
    columns: (auto, 1fr, auto),
    column-gutter: 8pt,
    align: (left + horizon, left + horizon, right + horizon),
    text(size: 10.5pt, weight: "bold", a.icao) + if a.name != none { text(size: 9pt, fill: muted, "  " + a.name) },
    text(size: 8pt, fill: muted, a.role),
    category-badge(a.flight_category),
  )
  if a.metar_raw == none and a.taf_raw == none and a.metar_decoded == none and a.taf_decoded == none {
    unavailable("No METAR/TAF available for this aerodrome.")
  }
  if a.metar_raw != none { mono-block(a.metar_raw) }
  if a.metar_decoded != none { block(above: 3pt, text(size: 8pt, preserve-lines(a.metar_decoded))) }
  if a.taf_raw != none { mono-block(a.taf_raw) }
  if a.taf_decoded != none { block(above: 3pt, text(size: 8pt, preserve-lines(a.taf_decoded))) }
})

#let winds-aloft-table(rows) = {
  set text(features: ("tnum",))
  let hdr(c) = text(size: 7pt, weight: "bold", fill: muted, c)
  table(
    columns: (1.4fr, 1fr, auto, auto),
    align: (left, left, right, right),
    stroke: none,
    inset: (x: 4pt, y: 3.5pt),
    table.header(hdr[Leg], hdr[Altitude], hdr[Wind], hdr[Temp]),
    table.hline(stroke: 0.75pt + ink),
    ..rows
      .map(r => (
        [#r.leg],
        [#r.altitude],
        [#(deg3(r.direction_deg).slice(0, 3) + "/" + f0(r.speed_kt) + " kt")],
        [#fmt-temp(r.temperature_c)],
      ))
      .flatten(),
  )
}

= Weather
#if data.weather == none {
  unavailable("Weather briefing not available — no weather snapshot was taken for this flight.")
} else {
  let wx = data.weather
  snapshot-line(wx.snapshot_time)
  sub-label("Aerodromes")
  if wx.aerodromes.len() == 0 {
    unavailable("No aerodrome weather available.")
  } else {
    for a in wx.aerodromes { aerodrome-weather(a) }
  }
  sub-label("Winds aloft (per leg, planned altitude)")
  if wx.at("winds_source_note", default: none) != none {
    caveat("Winds aloft: " + wx.winds_source_note + ".")
  }
  if wx.winds_aloft.len() == 0 {
    unavailable("No winds-aloft data available.")
  } else {
    winds-aloft-table(wx.winds_aloft)
  }
  sub-label("Freezing level")
  if wx.freezing_level == none {
    unavailable("Freezing level not available.")
  } else {
    text(size: 9pt, wx.freezing_level)
  }
}

// ── NOTAMs ──────────────────────────────────────────────────────────────
#let notam-card(n) = block(
  width: 100%,
  above: 9pt,
  fill: panel,
  stroke: (left: 2pt + accent),
  radius: 2pt,
  inset: 8pt,
  {
    grid(
      columns: (auto, 1fr, auto),
      column-gutter: 8pt,
      align: (left + horizon, left + horizon, right + horizon),
      mono(text(weight: "bold", n.id), size: 9pt),
      text(size: 9pt, weight: "bold", n.location),
      if n.relevance != none { text(size: 7.5pt, fill: muted, n.relevance) },
    )
    block(above: 4pt, text(size: 7.5pt, fill: muted, "Valid " + n.validity))
    if n.schedule != none { block(above: 2pt, text(size: 7.5pt, fill: muted, "Schedule " + n.schedule)) }
    if n.limits != none { block(above: 2pt, text(size: 7.5pt, fill: muted, "Limits " + n.limits)) }
    block(above: 5pt, text(size: 8.5pt, n.summary))
    block(above: 5pt, mono(preserve-lines(n.raw), size: 6.5pt))
  },
)

= NOTAMs
#if data.notams == none {
  unavailable("NOTAM briefing not available — no NOTAM snapshot was taken for this flight.")
} else {
  if data.notams.at("source_note", default: none) != none {
    caveat(data.notams.source_note + ".")
  }
  snapshot-line(data.notams.snapshot_time)
  if data.notams.notams.len() == 0 {
    unavailable("No relevant NOTAMs for this route and validity window.")
  } else {
    text(size: 7.5pt, fill: muted, "Ordered by relevance to the planned route. Raw text always governs.")
    for n in data.notams.notams { notam-card(n) }
  }
}
