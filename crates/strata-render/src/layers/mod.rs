//! Feature layers over the `set_*` data pushed by the app: airspace
//! polygons, point-feature symbols, SIGMET overlays and the gridded
//! weather overlays.
//!
//! All conversion/tessellation runs on the worker pool ([`crate::workers`]);
//! `prepare` only drains results, uploads buffers and refreshes the
//! per-dataset local-origin uniform (group 1 — see `pipelines`). Labels are
//! exposed via `labels()` / `visible_labels()` for the renderer to forward
//! into the [`crate::text::TextSystem`] each frame.

mod airspace;
mod pipelines;
mod points;
pub mod polylabel;
mod route;
pub mod style;
pub mod symbols;
mod tess;
#[cfg(test)]
mod tests_gpu;
mod weather;
mod weather_grid;

pub use airspace::{AIRSPACE_LABEL_MIN_ZOOM, AirspaceLayer, DEFAULT_AIRSPACE_MESH_CACHE_BYTES};
pub use points::PointLayer;
pub use route::{LEG_LABEL_MIN_ZOOM, LEG_LABEL_OFFSET_PX, RouteLayer};
pub use weather::{SIGMET_LABEL_MIN_ZOOM, WeatherLayer};
pub use weather_grid::GriddedWeatherLayer;

#[cfg(test)]
mod tests {
    use crate::gpu::shader::ShaderLibrary;
    use crate::layers::pipelines::{
        FILL_AIRSPACE_SHADER, LINE_DASH_SHADER, ROUTE_LINE_SHADER, ROUTE_RING_SHADER,
        WEATHER_SHADER, library_with,
    };

    /// The aero shaders are not registered in the embedded library (owned
    /// elsewhere), so the global naga test cannot see them — validate here.
    #[test]
    fn aero_layer_shaders_resolve_and_validate_with_naga() {
        for shader in [
            FILL_AIRSPACE_SHADER,
            LINE_DASH_SHADER,
            ROUTE_LINE_SHADER,
            ROUTE_RING_SHADER,
            WEATHER_SHADER,
        ] {
            let library = library_with(&ShaderLibrary::embedded(), shader);
            let resolved = library
                .resolve(shader.0)
                .unwrap_or_else(|e| panic!("{}: {e}", shader.0));
            let module = naga::front::wgsl::parse_str(&resolved)
                .unwrap_or_else(|e| panic!("{} failed to parse: {e}", shader.0));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{} failed validation: {e:?}", shader.0));
        }
    }
}
