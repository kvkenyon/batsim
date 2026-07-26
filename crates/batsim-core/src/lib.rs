//! batsim-core: the deterministic home-energy simulation engine.
//!
//! A pure, synchronous, allocation-frugal library: no tokio, no
//! network, no `std::time::{SystemTime, Instant}` in simulation paths. All
//! time is virtual ([`time::SimClock`]); all randomness flows through the
//! seeded ChaCha stream-splitting subsystem ([`rng`]).
//!
//! Current scope: the engine and tick loop, the determinism gate, the
//! split-efficiency SOC model, Thevenin voltage sag, chemistry modules,
//! the inverter conversion model, load synthesis, the PV model, dispatch,
//! and coupling-aware topology routing, against the batsim-registry
//! catalog. A lumped thermal model, degradation tracking, outages, and
//! vendor telemetry noise classes are planned future work and are
//! deliberately absent; where the battery and thermal physics need a cell
//! temperature, the ambient feed stands in directly.

pub mod battery;
pub mod chemistry;
pub mod dispatch;
pub mod engine;
pub mod error;
pub mod home;
pub mod inverter;
pub mod load;
mod math;
pub mod pv;
pub mod rng;
pub mod telemetry;
pub mod time;
pub mod topology;

pub use error::CoreError;
