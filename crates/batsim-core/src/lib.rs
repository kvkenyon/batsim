//! batsim-core: the Part B simulation engine.
//!
//! A pure, synchronous, allocation-frugal library (spec C.2.2): no tokio, no
//! network, no `std::time::{SystemTime, Instant}` in simulation paths. All
//! time is virtual ([`time::SimClock`]); all randomness flows through the
//! seeded ChaCha stream-splitting subsystem ([`rng`], spec B.1.4).
//!
//! M1 scope (spec 0.2): F1-F6, F9, F10, F15, F16 against the batsim-registry
//! catalog. Thermal lumped model (F7), degradation (F8), outages (F11), and
//! vendor telemetry noise classes (F12) are M4 and are deliberately absent;
//! where B.2/B.4 physics needs a cell temperature, the ambient feed stands
//! in directly.

pub mod battery;
pub mod chemistry;
pub mod dispatch;
pub mod engine;
pub mod error;
pub mod home;
pub mod inverter;
pub mod load;
pub mod pv;
pub mod rng;
pub mod telemetry;
pub mod time;
pub mod topology;

pub use error::CoreError;
