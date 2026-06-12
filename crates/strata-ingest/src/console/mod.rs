//! CLI front end over the `strata_ingest` library: configuration
//! resolution, command dispatch, and per-stage stdout reporting. Progress
//! events are rendered by [`adapter`].

mod adapter;
mod report;
mod status;

use std::env;
use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use strata_data::paths;
use strata_ingest::{CancellationToken, EventSink, IngestConfig, IngestError, Ingestion};

use crate::cli::{Command, GlobalArgs};

const API_KEY_ENV: &str = "OPENAIP_API_KEY";

/// Resolves CLI arguments and environment into the library config. The
/// openAIP key is read here (env / `.env` via dotenvy in `main`) — the
/// library itself never touches the environment.
///
/// Also performs the one-shot migrations: a pre-rename default data dir
/// (`~/.local/share/<legacy>` → `~/.local/share/strata`) and the
/// pre-multi-country basemap archive (`basemap-de.mbtiles` →
/// `basemap.mbtiles`), so existing ingested data keeps being found.
pub fn resolve(args: &GlobalArgs) -> Result<IngestConfig> {
    let data_dir = resolve_data_dir(
        args.data_dir.clone(),
        paths::env_var_with_legacy(paths::DATA_DIR_ENV, paths::LEGACY_DATA_DIR_ENV),
        dirs::data_dir(),
    )
    .context("cannot determine a data directory — pass --data-dir or set STRATA_DATA_DIR")?;
    if let Some(base) = dirs::data_dir()
        && data_dir == base.join(paths::DIR_NAME)
    {
        paths::migrate_legacy_dir(&base.join(paths::LEGACY_DIR_NAME), &data_dir);
    }
    paths::migrate_legacy_basemap(&data_dir);
    Ok(IngestConfig {
        data_dir,
        countries: args.countries.clone(),
        openaip_api_key: env::var(API_KEY_ENV).ok(),
        bbox_override: args.bbox,
    })
}

pub async fn run(command: Command, config: IngestConfig) -> Result<()> {
    match command {
        Command::Aero => {
            report::aero(&stage(&config, |r| async move { r.aero().await }).await?);
        }
        Command::Basemap { maxzoom } => {
            report::basemap(
                &stage(&config, move |r| async move { r.basemap(maxzoom).await }).await?,
            );
        }
        Command::Terrain { minzoom, maxzoom } => {
            report::terrain(
                &stage(&config, move |r| async move {
                    r.terrain(minzoom, maxzoom).await
                })
                .await?,
            );
        }
        Command::Elevation => {
            report::elevation(&stage(&config, |r| async move { r.elevation().await }).await?);
        }
        Command::All {
            basemap_maxzoom,
            terrain_minzoom,
            terrain_maxzoom,
        } => {
            // Stage by stage (not `Ingestion::all`) so each summary prints
            // as soon as its stage completes — the original output order.
            report::aero(&stage(&config, |r| async move { r.aero().await }).await?);
            report::terrain(
                &stage(&config, move |r| async move {
                    r.terrain(terrain_minzoom, terrain_maxzoom).await
                })
                .await?,
            );
            report::basemap(
                &stage(
                    &config,
                    move |r| async move { r.basemap(basemap_maxzoom).await },
                )
                .await?,
            );
        }
        Command::Status => status::run(&config)?,
    }
    Ok(())
}

/// Runs one ingestion stage with a fresh event channel and an indicatif
/// renderer attached. The renderer drains the channel after the runner
/// (the only sender) is dropped, so bars are finalized before the caller
/// prints the stage summary.
async fn stage<T, F, Fut>(config: &IngestConfig, run: F) -> Result<T>
where
    F: FnOnce(Ingestion) -> Fut,
    Fut: Future<Output = Result<T, IngestError>>,
{
    let (sink, events) = EventSink::channel();
    let runner = Ingestion::new(config.clone(), sink, CancellationToken::new());
    let renderer = tokio::spawn(adapter::render(events));
    let result = run(runner).await;
    let _ = renderer.await;
    Ok(result?)
}

/// Precedence: `--data-dir` flag > `$STRATA_DATA_DIR` (empty = unset) >
/// `<XDG data dir>/strata` (`~/.local/share/strata`).
fn resolve_data_dir(
    flag: Option<PathBuf>,
    env_value: Option<OsString>,
    xdg_data_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(dir) = flag {
        return Some(dir);
    }
    if let Some(value) = env_value
        && !value.is_empty()
    {
        return Some(PathBuf::from(value));
    }
    xdg_data_dir.map(|base| base.join(paths::DIR_NAME))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn flag_beats_env_and_default() {
        let dir = resolve_data_dir(
            Some(p("/flag")),
            Some(OsString::from("/env")),
            Some(p("/home/u/.local/share")),
        );
        assert_eq!(dir.as_deref(), Some(Path::new("/flag")));
    }

    #[test]
    fn env_beats_default() {
        let dir = resolve_data_dir(
            None,
            Some(OsString::from("/env")),
            Some(p("/home/u/.local/share")),
        );
        assert_eq!(dir.as_deref(), Some(Path::new("/env")));
    }

    #[test]
    fn default_is_xdg_data_dir_plus_strata() {
        let dir = resolve_data_dir(None, None, Some(p("/home/u/.local/share")));
        assert_eq!(
            dir.as_deref(),
            Some(Path::new("/home/u/.local/share/strata"))
        );
    }

    #[test]
    fn empty_env_counts_as_unset() {
        let dir = resolve_data_dir(None, Some(OsString::new()), Some(p("/home/u/.local/share")));
        assert_eq!(
            dir.as_deref(),
            Some(Path::new("/home/u/.local/share/strata"))
        );
    }

    #[test]
    fn nothing_resolvable_is_none() {
        assert_eq!(resolve_data_dir(None, None, None), None);
    }
}
