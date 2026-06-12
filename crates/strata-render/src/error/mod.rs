//! Renderer error type.

/// Errors surfaced by renderer initialization and shader handling.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("shader '{name}' not found (referenced from '{referenced_from}')")]
    ShaderNotFound {
        name: String,
        referenced_from: String,
    },
}
