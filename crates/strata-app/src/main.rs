mod app;
mod assets;
// TOML config (~/.config/strata/config.toml), loaded once in `app::run`.
mod config;
mod convert;
// Flight/aircraft file library (~/.local/share/strata/flights + aircraft).
mod flight_io;
// Atomic file writes with process-wide ordering (flight/aircraft/config).
mod fsutil;
mod gridded_weather;
mod map_view;
// App-side impls of strata-plan's source traits (store, WMM, gridded winds).
mod sources;
mod state;
mod tile_sources;
mod ui;

fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    app::run();
}
