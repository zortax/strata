# Strata

A desktop VFR map explorer and flight planner for Europe, built with Rust and 
GPUI.

## Features

- Vector basemap and terrain hillshade rendering (wgpu)
- Airspaces, airports and navaids with chart-style rendering, ingested
  per enabled country (Germany by default, the rest of Europe opt-in)
- Live METAR/TAF and SIGMET for the enabled countries
- Gridded weather overlays (DWD ICON-D2 forecast + RV radar, central
  Europe coverage) with a time slider
- VFR flight planning: route editing on the map, nav log with winds aloft,
  fuel and weight & balance, vertical profile with conflict checks, and a
  printable briefing PDF
- Search, themes
- Offline-first local data store with in-app ingestion

## Data sources

- [openAIP](https://www.openaip.net) (CC BY-NC) — airspaces, airports,
  navaids, reporting points, obstacles
- [OpenStreetMap](https://www.openstreetmap.org) via
  [Protomaps](https://protomaps.com) (ODbL) — vector basemap
- [DWD Open Data](https://opendata.dwd.de) — ICON-D2 forecast and RV radar
  composites
- [Copernicus DEM GLO-30](https://dataspace.copernicus.eu) — terrain
- [NOAA aviationweather.gov](https://aviationweather.gov) — METAR/TAF,
  SIGMET

An openAIP account/API key (free) is required for data ingestion.

## Building

Linux only (the map renders into a wgpu surface embedded in the window).

```sh
cargo run -p strata-app
```

Data ingestion runs in-app, or from the command line:

```sh
cargo run -p strata-ingest -- --help
```

## Configuration

`~/.config/strata/config.toml`, plus a settings UI in-app. Enabled
countries (`countries = ["DE", "AT"]`) control which countries' data are
downloaded and kept current — the map always renders everything the local
store holds.

## Disclaimer: Not for Primary Navigation

Strata is built using real-world aeronautical data and is designed to be a
highly capable tool for VFR flight planning, situational awareness, and
route exploration.

However, this software is provided strictly for educational and pre-flight
planning assistance purposes and is NOT an authoritative source of truth.

Aeronautical data changes constantly. The data provided by Strata may be
delayed, incomplete, or inaccurate due to parsing errors, upstream provider
delays, or software bugs. By using this software, you acknowledge and agree
to the following:

- **Pilot in Command (PIC) Responsibility:** As the PIC, you are solely
  responsible for the safety of your flight and for complying with all
  applicable aviation regulations.
- **Always Verify:** You must always verify your route, airspace
  restrictions, frequencies, weather, and NOTAMs against official, current,
  and legally authoritative sources (e.g., official national AIPs,
  certified EFB applications, or standard weather briefing services) prior
  to flight.
- **No In-Flight Navigation:** Strata is not certified for in-flight
  primary navigation.
- **Use at Your Own Risk:** The authors and contributors of Strata assume
  no liability or responsibility for any direct, indirect, incidental, or
  consequential damages, incidents, or regulatory violations arising out of
  the use or inability to use this software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
