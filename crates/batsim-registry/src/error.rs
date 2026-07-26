//! Error types for registry loading and validation (thiserror, per project
//! directive).

use std::path::PathBuf;

use thiserror::Error;

/// One semantic or schema violation found in a catalog entry. Validation
/// collects all violations across all entries before failing (spec §1.3:
/// "a diagnostic listing every offending entry and field path").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Registry-relative path of the offending file, e.g.
    /// `batteries/tesla_powerwall_2.json`.
    pub path: String,
    /// JSON field path within the entry, e.g. `soc_window.min_soc_frac`.
    pub field: String,
    /// Human-readable description of the violation.
    pub message: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}: {}", self.path, self.field, self.message)
    }
}

/// Errors from loading or validating the device registry.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// A catalog file could not be read from disk.
    #[error("failed to read registry file {path}: {source}")]
    Io {
        /// File that failed.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// A catalog file is not valid JSON or does not match its entry type.
    #[error("failed to parse registry entry {path}: {source}")]
    Parse {
        /// Registry-relative path.
        path: String,
        /// Underlying serde error.
        source: serde_json::Error,
    },

    /// The catalog failed schema or semantic validation; every violation is
    /// enumerated.
    #[error("registry validation failed with {} violation(s):\n{}", .violations.len(), .violations.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"))]
    Validation {
        /// All violations found, across all entries.
        violations: Vec<Violation>,
    },

    /// `catalog.json` content hashes do not match the entry files
    /// (tamper detection, spec §4.6).
    #[error("registry integrity check failed: {0}")]
    Integrity(String),

    /// Two entries share the same `(kind, model_id)` key.
    #[error("duplicate registry entry {kind:?} `{model_id}`")]
    Duplicate {
        /// Entry kind.
        kind: crate::types::EntryKind,
        /// Duplicated identifier.
        model_id: String,
    },

    /// A requested `model_id` does not exist in the loaded registry.
    #[error("unknown {kind:?} model `{model_id}`")]
    UnknownModel {
        /// Entry kind that was queried.
        kind: crate::types::EntryKind,
        /// Identifier that was not found.
        model_id: String,
    },
}
