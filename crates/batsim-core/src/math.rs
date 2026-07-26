//! Bit-exact transcendental math for simulation paths.
//!
//! Rust's `f64` transcendental methods delegate to the platform math
//! library, whose results may differ between platforms (and between libm
//! versions on the same platform). The golden traces hash every tick, so a
//! one-ulp difference across platforms fails the comparison. Every
//! transcendental evaluated on a simulation path MUST go through these
//! wrappers, which route to the `libm` crate: a pure-Rust implementation
//! with identical results on every target.
//!
//! Operations IEEE 754 requires to be correctly rounded (`sqrt`, `ceil`,
//! `floor`, `round`, `abs`, `rem_euclid`) are already bit-exact everywhere
//! and keep using the intrinsic methods.

/// Sine.
pub(crate) fn sin(x: f64) -> f64 {
    libm::sin(x)
}

/// Cosine.
pub(crate) fn cos(x: f64) -> f64 {
    libm::cos(x)
}

/// Tangent.
pub(crate) fn tan(x: f64) -> f64 {
    libm::tan(x)
}

/// Arc sine.
pub(crate) fn asin(x: f64) -> f64 {
    libm::asin(x)
}

/// Arc cosine.
pub(crate) fn acos(x: f64) -> f64 {
    libm::acos(x)
}

/// Four-quadrant arc tangent of `y / x`.
pub(crate) fn atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}

/// Natural exponential.
pub(crate) fn exp(x: f64) -> f64 {
    libm::exp(x)
}

/// Natural logarithm.
pub(crate) fn ln(x: f64) -> f64 {
    libm::log(x)
}

/// `base` raised to the power `exponent`.
pub(crate) fn powf(base: f64, exponent: f64) -> f64 {
    libm::pow(base, exponent)
}
