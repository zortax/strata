//! Versioned-document loading shared by the flight and aircraft files.
//!
//! Documents are pretty JSON with a `format_version` field. Loading is
//! *tolerant* (unknown fields ignored, missing optional fields defaulted —
//! same discipline as the app config) but *versioned*: files newer than the
//! running build are refused instead of silently mangled, and files older
//! than the current version are migrated step-by-step on the raw
//! `serde_json::Value` before typed deserialization.

use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

/// Errors loading a versioned JSON document.
#[derive(Debug, Error)]
pub enum VersionError {
    #[error("not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("document root must be a JSON object")]
    NotAnObject,
    #[error("document is format version {found}, newer than the supported {supported}")]
    TooNew { found: u32, supported: u32 },
    #[error("no migration registered from format version {from}")]
    NoMigration { from: u32 },
}

/// Parses `json`, migrating older `format_version`s up to `current` one
/// step at a time via `migrate(value, from) -> value-at-from+1`.
///
/// Files without a (numeric) `format_version` field are treated as
/// **version 1** — the first version ever written — never as `current`,
/// so pre-field files keep migrating correctly forever.
pub fn load_versioned<T, M>(json: &str, current: u32, migrate: M) -> Result<T, VersionError>
where
    T: DeserializeOwned,
    M: Fn(Value, u32) -> Result<Value, VersionError>,
{
    let mut value: Value = serde_json::from_str(json)?;
    if !value.is_object() {
        return Err(VersionError::NotAnObject);
    }
    let mut version = value
        .get("format_version")
        .and_then(Value::as_u64)
        .map_or(1, |v| u32::try_from(v).unwrap_or(u32::MAX));
    if version > current {
        return Err(VersionError::TooNew {
            found: version,
            supported: current,
        });
    }
    while version < current {
        tracing::debug!(
            from = version,
            to = version + 1,
            "migrating document format"
        );
        value = migrate(value, version)?;
        version += 1;
        if let Some(object) = value.as_object_mut() {
            object.insert("format_version".to_owned(), version.into());
        }
    }
    Ok(serde_json::from_value(value)?)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, PartialEq, Deserialize)]
    struct Doc {
        format_version: u32,
        #[serde(default)]
        name: String,
    }

    fn no_migrations(_: Value, from: u32) -> Result<Value, VersionError> {
        Err(VersionError::NoMigration { from })
    }

    #[test]
    fn current_version_loads_directly() {
        let doc: Doc =
            load_versioned(r#"{"format_version": 1, "name": "x"}"#, 1, no_migrations).unwrap();
        assert_eq!(
            doc,
            Doc {
                format_version: 1,
                name: "x".into()
            }
        );
    }

    #[test]
    fn missing_version_is_treated_as_one() {
        // Current is also 1 here, so no migration runs — but the typed
        // struct still needs the field, proving the default path is the
        // caller's serde default, not this loader.
        let result: Result<Doc, _> = load_versioned(r#"{"name": "x"}"#, 1, no_migrations);
        // Doc has no serde default for format_version -> typed parse fails,
        // which is the caller's choice; the loader itself accepted v1.
        assert!(matches!(result, Err(VersionError::Json(_))));
    }

    #[test]
    fn newer_version_is_refused() {
        let result: Result<Doc, _> = load_versioned(r#"{"format_version": 2}"#, 1, no_migrations);
        assert!(matches!(
            result,
            Err(VersionError::TooNew {
                found: 2,
                supported: 1
            })
        ));
    }

    #[test]
    fn migration_steps_run_in_order() {
        // current = 3, file = v1: expects migrate(1) then migrate(2).
        let doc: Doc = load_versioned(
            r#"{"format_version": 1, "name": "a"}"#,
            3,
            |mut value, from| {
                let object = value.as_object_mut().ok_or(VersionError::NotAnObject)?;
                let name = object.get("name").and_then(Value::as_str).unwrap_or("");
                object.insert("name".to_owned(), format!("{name}+{from}").into());
                Ok(value)
            },
        )
        .unwrap();
        assert_eq!(
            doc,
            Doc {
                format_version: 3,
                name: "a+1+2".into()
            }
        );
    }

    #[test]
    fn unregistered_migration_errors() {
        let result: Result<Doc, _> = load_versioned(r#"{"format_version": 0}"#, 1, no_migrations);
        assert!(matches!(result, Err(VersionError::NoMigration { from: 0 })));
    }

    #[test]
    fn non_object_root_is_refused() {
        let result: Result<Doc, _> = load_versioned("[1, 2]", 1, no_migrations);
        assert!(matches!(result, Err(VersionError::NotAnObject)));
    }
}
