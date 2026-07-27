//! batsim-server: the HTTP API shell over the simulation engine.
//!
//! A thin, replaceable layer: handlers validate requests, translate them
//! into engine calls over a command channel, and serialize results. No
//! physics lives here. The binary produced from this crate is `batsim`.

pub mod compose;
pub mod config;
pub mod engine;
pub mod ids;
pub mod model;
pub mod price;
pub mod problem;
pub mod routes;
pub mod state;
pub mod telemetry;

mod openapi;

pub use config::Config;
pub use openapi::openapi_document;
pub use routes::build_router;
pub use state::AppState;
