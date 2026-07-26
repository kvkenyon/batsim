//! The simulation world and time-control engine (spec B.1; F1).
//!
//! Owns the home arena (insertion-ordered `Vec`, never `HashMap`
//! iteration), the [`crate::time::SimClock`], the master seed, and the
//! execution modes: `step(n)`, `run_until(t)`, and speed-multiplier pacing.
//! All modes execute identical per-tick code; acceleration changes pacing
//! only, never numerics (B.1.3).
//!
//! Filled in during engine integration.
