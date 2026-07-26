//! Seeded RNG subsystem.
//!
//! All randomness in the engine flows through ChaCha stream splitting:
//!
//! ```text
//! stream_key(entity_id, purpose, tick) = ChaCha8Rng::seed_from_u64(
//!     hash64(master_seed, entity_id, purpose_tag, tick))
//! ```
//!
//! `hash64` is `xxh3_64` over the concatenated little-endian fields (fixed
//! here; one mixing function is picked once and frozen in code). Substreams
//! are stateless functions of `(seed, entity, purpose, tick)`, so parallel
//! scheduling, snapshot/resume, and replay cannot perturb results.

use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha8Rng;
use xxhash_rust::xxh3::xxh3_64;

/// Purpose tags for RNG substreams.
///
/// Never reorder or reuse tags; appended-only evolution keeps replay
/// compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum RngPurpose {
    /// Load-profile stochastic layers (appliance arrivals, AR(1) residual).
    LoadNoise = 1,
    /// PV cloud-variability overlay.
    PvCloud = 2,
    /// Telemetry measurement noise (reserved for planned future work).
    TelemetryNoise = 3,
    /// Dispatch execution latency/jitter draws.
    DispatchJitter = 4,
    /// Stochastic outage triggers (reserved for planned future work).
    OutageTrigger = 5,
    /// One-time per-home load phase/cycle offsets drawn at init.
    LoadPhase = 6,
    /// One-time per-home PV geometry jitter drawn at init.
    PvPhase = 7,
}

/// Compose a device-level entity id from a home index and a device slot.
///
/// Home-level streams use `entity_home(home_idx)`; per-device streams use
/// `entity_device(home_idx, slot)` with `slot` the device's stable index
/// within the home (batteries 1.., PV 0x100, load 0x101, etc. — slot
/// assignments are fixed at the call site and documented there).
#[must_use]
pub const fn entity_device(home_idx: u64, slot: u64) -> u64 {
    debug_assert!(slot < 0x1000);
    (home_idx << 12) | slot
}

/// The home-level entity id (slot 0).
#[must_use]
pub const fn entity_home(home_idx: u64) -> u64 {
    home_idx << 12
}

/// Fixed non-cryptographic mixing function: `xxh3_64` over the
/// concatenated little-endian fields.
#[must_use]
pub fn hash64(master_seed: u64, entity_id: u64, purpose: RngPurpose, tick: u64) -> u64 {
    let mut buf = [0u8; 32];
    buf[0..8].copy_from_slice(&master_seed.to_le_bytes());
    buf[8..16].copy_from_slice(&entity_id.to_le_bytes());
    buf[16..24].copy_from_slice(&(purpose as u64).to_le_bytes());
    buf[24..32].copy_from_slice(&tick.to_le_bytes());
    xxh3_64(&buf)
}

/// Construct the ChaCha8 substream for one `(entity, purpose, tick)`.
///
/// Seeding cost is tens of ns; callers construct streams per
/// tick on the stack — no RNG state is ever serialized.
#[must_use]
pub fn substream(master_seed: u64, entity_id: u64, purpose: RngPurpose, tick: u64) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(hash64(master_seed, entity_id, purpose, tick))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use rand_chacha::rand_core::RngCore;

    use super::*;

    #[test]
    fn substreams_are_deterministic_and_independent() {
        let a1 = substream(42, entity_device(7, 1), RngPurpose::LoadNoise, 100).next_u64();
        let a2 = substream(42, entity_device(7, 1), RngPurpose::LoadNoise, 100).next_u64();
        assert_eq!(a1, a2, "same key -> same stream");

        let b = substream(42, entity_device(7, 1), RngPurpose::LoadNoise, 101).next_u64();
        assert_ne!(a1, b, "tick changes stream");
        let c = substream(42, entity_device(7, 2), RngPurpose::LoadNoise, 100).next_u64();
        assert_ne!(a1, c, "entity changes stream");
        let d = substream(42, entity_device(7, 1), RngPurpose::PvCloud, 100).next_u64();
        assert_ne!(a1, d, "purpose changes stream");
        let e = substream(43, entity_device(7, 1), RngPurpose::LoadNoise, 100).next_u64();
        assert_ne!(a1, e, "master seed changes stream");
    }

    #[test]
    fn entity_slots_do_not_collide() {
        assert_ne!(entity_home(0), entity_device(0, 1));
        assert_eq!(entity_device(5, 0), entity_home(5));
        assert_eq!(entity_device(1, 0xFFF), (1 << 12) | 0xFFF);
    }
}
