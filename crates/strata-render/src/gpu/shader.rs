//! Embedded WGSL library with `//#include` resolution.
//!
//! Shaders live in `src/shaders/`, one file per concern; shared structs and
//! functions live in `common.wgsl` and are pulled in with a line of the form
//!
//! ```wgsl
//! //#include common.wgsl
//! ```
//!
//! at pipeline-creation time. Each file is included at most once per resolved
//! shader (so diamond includes and cycles are safe), and the directive line
//! is replaced by the included file's resolved content.

use crate::error::RenderError;

use rustc_hash::FxHashMap;

/// Include directive prefix; the rest of the line is the file name.
pub const INCLUDE_DIRECTIVE: &str = "//#include";

const EMBEDDED_SOURCES: &[(&str, &str)] = &[
    ("common.wgsl", include_str!("../shaders/common.wgsl")),
    ("fill.wgsl", include_str!("../shaders/fill.wgsl")),
    ("line.wgsl", include_str!("../shaders/line.wgsl")),
    (
        "raster_tile.wgsl",
        include_str!("../shaders/raster_tile.wgsl"),
    ),
    ("symbol.wgsl", include_str!("../shaders/symbol.wgsl")),
    ("text.wgsl", include_str!("../shaders/text.wgsl")),
    (
        "weather_grid.wgsl",
        include_str!("../shaders/weather_grid.wgsl"),
    ),
];

/// Named WGSL sources with include resolution.
pub struct ShaderLibrary {
    sources: FxHashMap<&'static str, &'static str>,
}

impl ShaderLibrary {
    /// The shaders embedded from `src/shaders/`.
    pub fn embedded() -> Self {
        Self::from_sources(EMBEDDED_SOURCES.iter().copied())
    }

    /// A library over explicit sources (tests, future hot-reload).
    pub fn from_sources(sources: impl IntoIterator<Item = (&'static str, &'static str)>) -> Self {
        Self {
            sources: sources.into_iter().collect(),
        }
    }

    /// Names of all known shader files.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.sources.keys().copied()
    }

    /// The raw (unresolved) source of a file.
    pub fn raw_source(&self, name: &str) -> Option<&'static str> {
        self.sources.get(name).copied()
    }

    /// Fully resolved WGSL for `name` with all includes expanded.
    pub fn resolve(&self, name: &str) -> Result<String, RenderError> {
        let mut included = Vec::new();
        let mut out = String::new();
        self.resolve_into(name, name, &mut included, &mut out)?;
        Ok(out)
    }

    /// Resolve `name` and create a (validated) shader module from it.
    pub fn create_module(
        &self,
        device: &wgpu::Device,
        name: &str,
    ) -> Result<wgpu::ShaderModule, RenderError> {
        let source = self.resolve(name)?;
        Ok(device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(name),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        }))
    }

    fn resolve_into(
        &self,
        name: &str,
        root: &str,
        included: &mut Vec<String>,
        out: &mut String,
    ) -> Result<(), RenderError> {
        if included.iter().any(|n| n == name) {
            return Ok(()); // already emitted (diamond include or cycle)
        }
        included.push(name.to_owned());
        let source =
            self.sources
                .get(name)
                .copied()
                .ok_or_else(|| RenderError::ShaderNotFound {
                    name: name.to_owned(),
                    referenced_from: root.to_owned(),
                })?;
        for line in source.lines() {
            if let Some(rest) = line.trim_start().strip_prefix(INCLUDE_DIRECTIVE) {
                let include_name = rest.trim();
                if include_name.is_empty() {
                    return Err(RenderError::ShaderNotFound {
                        name: "<empty include>".to_owned(),
                        referenced_from: name.to_owned(),
                    });
                }
                self.resolve_into(include_name, name, included, out)?;
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
        Ok(())
    }
}

impl Default for ShaderLibrary {
    fn default() -> Self {
        Self::embedded()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_single_include() {
        let lib = ShaderLibrary::from_sources([
            ("common.wgsl", "fn shared() -> f32 { return 1.0; }"),
            ("main.wgsl", "//#include common.wgsl\nfn user() {}"),
        ]);
        let resolved = lib.resolve("main.wgsl").expect("resolve");
        assert!(resolved.contains("fn shared()"));
        assert!(resolved.contains("fn user()"));
        assert!(!resolved.contains(INCLUDE_DIRECTIVE));
        // include content precedes the including file's content
        let shared_at = resolved.find("fn shared").expect("shared");
        let user_at = resolved.find("fn user").expect("user");
        assert!(shared_at < user_at);
    }

    #[test]
    fn diamond_include_emits_once() {
        let lib = ShaderLibrary::from_sources([
            ("common.wgsl", "fn shared() {}"),
            ("mid.wgsl", "//#include common.wgsl\nfn mid() {}"),
            (
                "main.wgsl",
                "//#include common.wgsl\n//#include mid.wgsl\nfn user() {}",
            ),
        ]);
        let resolved = lib.resolve("main.wgsl").expect("resolve");
        assert_eq!(resolved.matches("fn shared").count(), 1);
    }

    #[test]
    fn include_cycle_terminates() {
        let lib = ShaderLibrary::from_sources([
            ("a.wgsl", "//#include b.wgsl\nfn a() {}"),
            ("b.wgsl", "//#include a.wgsl\nfn b() {}"),
        ]);
        let resolved = lib.resolve("a.wgsl").expect("must not loop");
        assert_eq!(resolved.matches("fn a").count(), 1);
        assert_eq!(resolved.matches("fn b").count(), 1);
    }

    #[test]
    fn unknown_include_errors() {
        let lib = ShaderLibrary::from_sources([("main.wgsl", "//#include nope.wgsl")]);
        let err = lib.resolve("main.wgsl").expect_err("must fail");
        let RenderError::ShaderNotFound {
            name,
            referenced_from,
        } = err;
        assert_eq!(name, "nope.wgsl");
        assert_eq!(referenced_from, "main.wgsl");
    }

    #[test]
    fn unknown_root_shader_errors() {
        let lib = ShaderLibrary::embedded();
        assert!(lib.resolve("missing.wgsl").is_err());
    }

    #[test]
    fn all_embedded_shaders_resolve_and_validate_with_naga() {
        let lib = ShaderLibrary::embedded();
        for name in lib.names().collect::<Vec<_>>() {
            let resolved = lib.resolve(name).unwrap_or_else(|e| panic!("{name}: {e}"));
            let module = naga::front::wgsl::parse_str(&resolved)
                .unwrap_or_else(|e| panic!("{name} failed to parse: {e}"));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name} failed validation: {e:?}"));
        }
    }
}
