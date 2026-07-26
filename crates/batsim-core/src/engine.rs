//! The simulation world and time-control engine (spec B.1; F1).
//!
//! Owns the home arena (insertion-ordered `Vec`, never `HashMap`
//! iteration, B.1.4), the [`SimClock`], and the master seed. Execution
//! modes: `step(n)`, `run_until(t)`, and speed-multiplier pacing
//! (realtime / fast-forward / unbounded, B.1.3). All modes execute
//! identical per-tick code; acceleration changes pacing only, never
//! numerics.
//!
//! Determinism (B.1.4): per-tick work is a pure function of
//! `(state, tick)`. The optional rayon step partitions the arena into
//! fixed chunks computed once; fleet aggregates are combined in index
//! order (f64 addition is not associative — no `par_iter().sum()`).

use serde::{Deserialize, Serialize};

use crate::dispatch::ScheduledDispatch;
use crate::error::CoreError;
use crate::home::Home;
use crate::time::SimClock;

/// Ambient temperature feed (M1). The Texas TMY/NSRDB hourly feed with
/// Catmull-Rom interpolation (B.4.2) arrives with the scenario pipeline;
/// M1 provides deterministic synthetic feeds with the same pure-function
/// contract.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum AmbientFeed {
    /// Constant temperature (degC).
    Constant(f64),
    /// Diurnal sinusoid: `mean + amplitude * sin(2 pi (hour - 15) / 24)`,
    /// peaking at 15:00 local (UTC-6 Texas, documented simplification).
    DiurnalSine {
        /// Daily mean temperature (degC).
        mean_c: f64,
        /// Diurnal half-swing (degC).
        amplitude_c: f64,
    },
}

impl AmbientFeed {
    /// Temperature at a unix time (degC). Pure function.
    #[must_use]
    pub fn at(&self, unix_time_s: u64) -> f64 {
        match *self {
            Self::Constant(t) => t,
            Self::DiurnalSine {
                mean_c,
                amplitude_c,
            } => {
                // UTC-6 fixed offset (Texas; DST ignored, documented).
                let hour = (unix_time_s as f64 / 3600.0 - 6.0).rem_euclid(24.0);
                let phase = std::f64::consts::TAU * (hour - 15.0) / 24.0;
                mean_c + amplitude_c * phase.sin()
            }
        }
    }
}

/// Execution speed (B.1.3). Switchable only while paused — in this
/// synchronous library, pacing applies between `run_paced` calls.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Speed {
    /// One tick per `dt` of wall time (1x).
    Realtime,
    /// Up to N x realtime.
    FastForward(f64),
    /// As fast as compute allows (batch default; no pacing code).
    Unbounded,
}

/// The simulation world: homes + clock + seed + ambient feed.
#[derive(Debug, Serialize, Deserialize)]
pub struct SimWorld {
    homes: Vec<Home>,
    clock: SimClock,
    master_seed: u64,
    ambient: AmbientFeed,
}

impl SimWorld {
    /// Create a world. `master_seed` keys every RNG substream (B.1.4).
    ///
    /// # Errors
    /// Propagates [`SimClock::new`] errors.
    pub fn new(clock: SimClock, master_seed: u64, ambient: AmbientFeed) -> Result<Self, CoreError> {
        Ok(Self {
            homes: Vec::new(),
            clock,
            master_seed,
            ambient,
        })
    }

    /// The master seed (run-manifest recording, B.1.4).
    #[must_use]
    pub const fn master_seed(&self) -> u64 {
        self.master_seed
    }

    /// The virtual clock.
    #[must_use]
    pub const fn clock(&self) -> &SimClock {
        &self.clock
    }

    /// The ambient feed.
    #[must_use]
    pub const fn ambient(&self) -> &AmbientFeed {
        &self.ambient
    }

    /// Add a home; returns its stable arena index (RNG entity key base).
    pub fn add_home(&mut self, home: Home) -> u64 {
        let idx = self.homes.len() as u64;
        self.homes.push(home);
        idx
    }

    /// Number of homes.
    #[must_use]
    pub fn home_count(&self) -> usize {
        self.homes.len()
    }

    /// Read a home by arena index.
    #[must_use]
    pub fn home(&self, idx: usize) -> Option<&Home> {
        self.homes.get(idx)
    }

    /// Read a home mutably by arena index.
    pub fn home_mut(&mut self, idx: usize) -> Option<&mut Home> {
        self.homes.get_mut(idx)
    }

    /// Queue a dispatch to one home (applied at its tick top).
    ///
    /// # Errors
    /// [`CoreError::Dispatch`] when `home_idx` is out of range.
    pub fn dispatch(&mut self, home_idx: usize, cmd: ScheduledDispatch) -> Result<(), CoreError> {
        let home = self
            .homes
            .get_mut(home_idx)
            .ok_or_else(|| CoreError::Dispatch(format!("no home at index {home_idx}")))?;
        home.schedule(cmd);
        Ok(())
    }

    /// Advance one tick: step every home in arena order (B.1.5 pipeline
    /// per home). Single-threaded; the reference implementation.
    pub fn step(&mut self) {
        let (tick, unix, dt) = (self.clock.tick(), self.clock.unix_time(), self.clock.dt_s());
        let t_amb = self.ambient.at(unix);
        for home in &mut self.homes {
            home.step(tick, unix, dt, t_amb);
        }
        self.clock.advance();
    }

    /// Advance one tick with rayon parallelism. Produces bit-identical
    /// state to [`SimWorld::step`] (B.1.4): homes are independent within a
    /// tick, each home's RNG streams are keyed by `(seed, entity, tick)`,
    /// and no cross-home reduction feeds back into state.
    pub fn step_parallel(&mut self) {
        use rayon::prelude::*;
        let (tick, unix, dt) = (self.clock.tick(), self.clock.unix_time(), self.clock.dt_s());
        let t_amb = self.ambient.at(unix);
        // Fixed chunk size computed once (B.10.4) so partitioning is
        // deterministic regardless of thread scheduling.
        let n_threads = rayon::current_num_threads().max(1);
        let chunk = (self.homes.len() / (4 * n_threads)).max(1);
        self.homes.par_chunks_mut(chunk).for_each(|chunk_homes| {
            for home in chunk_homes {
                home.step(tick, unix, dt, t_amb);
            }
        });
        self.clock.advance();
    }

    /// Advance `n` ticks (unbounded; B.1.3 fast-forward infinity).
    pub fn step_n(&mut self, n: u64) {
        for _ in 0..n {
            self.step();
        }
    }

    /// Advance `n` ticks using the parallel stepper; identical state to
    /// [`SimWorld::step_n`].
    pub fn step_n_parallel(&mut self, n: u64) {
        for _ in 0..n {
            self.step_parallel();
        }
    }

    /// Advance until `t_sim >= t_target_s` (B.1.3 run-until; the primary
    /// scenario-replay mode).
    pub fn run_until(&mut self, t_target_s: u64) {
        while self.clock.t_sim() < t_target_s {
            self.step();
        }
    }

    /// Advance at a paced speed for `ticks` ticks (B.1.3). Pacing reads
    /// wall time ONLY to sleep between ticks; it never enters simulation
    /// state, so results are bit-identical to [`SimWorld::step_n`]
    /// (B.1.3 contract). Overruns are counted, not caught up
    /// (`RealtimeOverrun` semantics: pacing skew is recorded, not
    /// silently absorbed).
    ///
    /// Returns the number of pacing overruns (compute longer than the
    /// slice allows).
    ///
    /// # Errors
    /// [`CoreError::InvalidConfig`] when a `FastForward` multiplier is not
    /// finite and positive.
    pub fn run_paced(&mut self, ticks: u64, speed: Speed) -> Result<u64, CoreError> {
        if let Speed::FastForward(n) = speed {
            if !n.is_finite() || n <= 0.0 {
                return Err(CoreError::InvalidConfig(format!(
                    "FastForward multiplier must be finite and > 0, got {n}"
                )));
            }
        }
        let mut overruns = 0u64;
        let dt = self.clock.dt_s();
        for _ in 0..ticks {
            let start = std::time::Instant::now();
            self.step();
            let budget = match speed {
                Speed::Unbounded => continue,
                Speed::Realtime => std::time::Duration::from_secs_f64(f64::from(dt)),
                Speed::FastForward(n) => {
                    std::time::Duration::from_secs_f64(f64::from(dt) / n)
                }
            };
            let elapsed = start.elapsed();
            if elapsed < budget {
                std::thread::sleep(budget - elapsed);
            } else {
                overruns += 1;
            }
        }
        Ok(overruns)
    }
}
