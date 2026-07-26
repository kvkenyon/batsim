//! Error types for the simulation core (thiserror, per project directive).

use thiserror::Error;

/// Errors from engine construction, composition, and stepping.
#[derive(Debug, Error)]
pub enum CoreError {
    /// Invalid static configuration (bad dt, unaligned epoch, impossible
    /// limits).
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    /// The registry could not provide a required entry.
    #[error("registry error: {0}")]
    Registry(#[from] batsim_registry::RegistryError),

    /// A HomeSystem composition failed validation.
    #[error("invalid home system: {0}")]
    InvalidSystem(String),

    /// A dispatch command cannot be applied (physically or logically).
    #[error("dispatch rejected: {0}")]
    Dispatch(String),

    /// Serialization of engine state failed.
    #[error("state serialization failed: {0}")]
    Serialization(String),
}
