//! Truth telemetry records and meter counters (spec B.9.1; M1 scope).
//!
//! M1 emits the lossless per-tick `debug_truth` stream only (B.9.2):
//! vendor noise classes, quantization, and rate decimation are F12 (M4).
//! Meter points follow the A.3 topology diagrams: MAIN, PV_AC, BATT_AC,
//! BACKUP_PANEL.
//!
//! Filled in during engine integration.
