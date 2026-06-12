//! Command-line definition (clap derive). Pure parsing — no IO — so every
//! flag and value parser is unit-testable via `try_parse_from`.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use strata_data::domain::{BoundingBox, Country};

#[derive(Debug, Parser)]
#[command(
    name = "strata-ingest",
    version,
    about = "Strata data ingestion: download, normalize and write the local store",
    propagate_version = true
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Args)]
pub struct GlobalArgs {
    /// Countries to ingest, as comma-separated ISO 3166-1 alpha-2 codes
    /// (e.g. `DE,AT,CH`)
    #[arg(
        long,
        global = true,
        default_value = "DE",
        value_name = "CODES",
        value_delimiter = ',',
        value_parser = parse_country
    )]
    pub countries: Vec<Country>,

    /// Data directory [default: ~/.local/share/strata; env: STRATA_DATA_DIR]
    #[arg(long, global = true, value_name = "DIR")]
    pub data_dir: Option<PathBuf>,

    /// Bounding-box override `WEST,SOUTH,EAST,NORTH` in degrees — shrinks
    /// basemap/terrain coverage for cheap smoke runs
    #[arg(long, global = true, value_name = "W,S,E,N", value_parser = parse_bbox)]
    pub bbox: Option<BoundingBox>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Fetch openAIP aeronautical data (airspaces, airports, navaids,
    /// reporting points, obstacles) into the store
    Aero,

    /// Extract the vector basemap from the latest Protomaps build into an
    /// MBTiles file (resumes if interrupted)
    Basemap {
        /// Highest zoom level to extract
        #[arg(long, default_value_t = 13)]
        maxzoom: u8,
    },

    /// Render Copernicus GLO-30 hillshade tiles into the store and
    /// max-pool the DEM into the elevation grid
    Terrain {
        /// Lowest zoom level to render
        #[arg(long, default_value_t = 5)]
        minzoom: u8,
        /// Highest zoom level to render
        #[arg(long, default_value_t = 11)]
        maxzoom: u8,
    },

    /// Max-pool the GLO-30 DEM into the store's elevation grid only
    /// (backfill for stores that already have hillshade tiles; reuses the
    /// dem-cache)
    Elevation,

    /// Run aero, terrain and basemap in sequence
    All {
        /// Highest basemap zoom level to extract
        #[arg(long, default_value_t = 13)]
        basemap_maxzoom: u8,
        /// Lowest terrain zoom level to render
        #[arg(long, default_value_t = 5)]
        terrain_minzoom: u8,
        /// Highest terrain zoom level to render
        #[arg(long, default_value_t = 11)]
        terrain_maxzoom: u8,
    },

    /// Show datasets in the data dir: source, AIRAC cycle, staleness, tile
    /// counts and file sizes
    Status,
}

fn parse_country(s: &str) -> Result<Country, String> {
    Country::from_code(s.trim()).ok_or_else(|| {
        let codes: Vec<&str> = Country::ALL.iter().map(|c| c.code()).collect();
        format!(
            "unsupported country '{s}' — expected one of: {}",
            codes.join(", ")
        )
    })
}

fn parse_bbox(s: &str) -> Result<BoundingBox, String> {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    let &[west, south, east, north] = parts.as_slice() else {
        return Err(format!(
            "expected 4 comma-separated degrees WEST,SOUTH,EAST,NORTH, got {} value(s)",
            parts.len()
        ));
    };
    let num = |name: &str, v: &str| -> Result<f64, String> {
        v.parse::<f64>()
            .map_err(|_| format!("{name} '{v}' is not a number"))
    };
    BoundingBox::new(
        num("west", west)?,
        num("south", south)?,
        num("east", east)?,
        num("north", north)?,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("strata-ingest").chain(args.iter().copied()))
    }

    #[test]
    fn cli_definition_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn aero_parses_with_defaults() {
        let cli = parse(&["aero"]).unwrap();
        assert!(matches!(cli.command, Command::Aero));
        assert_eq!(cli.global.countries, vec![Country::DE], "default is Germany");
        assert_eq!(cli.global.data_dir, None);
        assert_eq!(cli.global.bbox, None);
    }

    #[test]
    fn countries_parse_as_a_comma_separated_list() {
        let cli = parse(&["--countries", "DE,AT", "aero"]).unwrap();
        assert_eq!(cli.global.countries, vec![Country::DE, Country::AT]);
        // Repeated flags accumulate too.
        let cli = parse(&["--countries", "DE", "--countries", "CH", "aero"]).unwrap();
        assert_eq!(cli.global.countries, vec![Country::DE, Country::CH]);
    }

    #[test]
    fn country_codes_are_case_insensitive_and_trimmed() {
        for arg in ["de", "DE", "De", " de "] {
            let cli = parse(&["--countries", arg, "aero"]).unwrap();
            assert_eq!(cli.global.countries, vec![Country::DE]);
        }
        let cli = parse(&["--countries", "de, at", "aero"]).unwrap();
        assert_eq!(cli.global.countries, vec![Country::DE, Country::AT]);
    }

    #[test]
    fn unknown_country_gives_friendly_error() {
        let err = parse(&["--countries", "xx", "aero"]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unsupported country 'xx'"), "got: {msg}");
        assert!(msg.contains("DE"), "got: {msg}");
    }

    #[test]
    fn global_flags_work_after_the_subcommand() {
        let cli = parse(&["aero", "--countries", "DE", "--data-dir", "/tmp/x"]).unwrap();
        assert_eq!(cli.global.data_dir.as_deref(), Some(std::path::Path::new("/tmp/x")));
    }

    #[test]
    fn bbox_parses_west_south_east_north() {
        let cli = parse(&["--bbox", "9.5, 49.0 ,10.5,50.0", "terrain"]).unwrap();
        let bbox = cli.global.bbox.unwrap();
        assert_eq!(bbox.west(), 9.5);
        assert_eq!(bbox.south(), 49.0);
        assert_eq!(bbox.east(), 10.5);
        assert_eq!(bbox.north(), 50.0);
    }

    #[test]
    fn bbox_rejects_wrong_arity() {
        let err = parse(&["--bbox", "9.5,49.0,10.5", "terrain"]).unwrap_err();
        assert!(err.to_string().contains("got 3 value(s)"), "got: {err}");
    }

    #[test]
    fn bbox_rejects_non_numeric() {
        let err = parse(&["--bbox", "a,49.0,10.5,50.0", "terrain"]).unwrap_err();
        assert!(err.to_string().contains("west 'a' is not a number"), "got: {err}");
    }

    #[test]
    fn bbox_rejects_inverted_bounds() {
        assert!(parse(&["--bbox", "10.5,49.0,9.5,50.0", "terrain"]).is_err());
        assert!(parse(&["--bbox", "9.5,91.0,10.5,95.0", "terrain"]).is_err());
    }

    #[test]
    fn basemap_maxzoom_defaults_to_13() {
        let cli = parse(&["basemap"]).unwrap();
        assert!(matches!(cli.command, Command::Basemap { maxzoom: 13 }));
        let cli = parse(&["basemap", "--maxzoom", "9"]).unwrap();
        assert!(matches!(cli.command, Command::Basemap { maxzoom: 9 }));
    }

    #[test]
    fn terrain_zooms_default_to_5_and_11() {
        let cli = parse(&["terrain"]).unwrap();
        assert!(matches!(cli.command, Command::Terrain { minzoom: 5, maxzoom: 11 }));
        let cli = parse(&["terrain", "--minzoom", "6", "--maxzoom", "8"]).unwrap();
        assert!(matches!(cli.command, Command::Terrain { minzoom: 6, maxzoom: 8 }));
    }

    #[test]
    fn all_carries_per_stage_zoom_flags() {
        let cli = parse(&["all"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::All { basemap_maxzoom: 13, terrain_minzoom: 5, terrain_maxzoom: 11 }
        ));
        let cli = parse(&["all", "--basemap-maxzoom", "10", "--terrain-maxzoom", "9"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::All { basemap_maxzoom: 10, terrain_minzoom: 5, terrain_maxzoom: 9 }
        ));
    }

    #[test]
    fn status_parses() {
        let cli = parse(&["status"]).unwrap();
        assert!(matches!(cli.command, Command::Status));
    }

    #[test]
    fn elevation_parses_with_global_flags() {
        let cli = parse(&["elevation"]).unwrap();
        assert!(matches!(cli.command, Command::Elevation));
        let cli = parse(&["elevation", "--bbox", "9.5,49.0,10.5,50.0"]).unwrap();
        assert!(cli.global.bbox.is_some());
    }
}
