//! Server-assigned identifiers: prefixed ULIDs.
//!
//! ULIDs sort by creation time, which keeps audit logs and cursor
//! pagination trivial. The prefix makes the resource kind obvious at a
//! glance and guards against cross-kind id confusion.

use ulid::Ulid;

/// Generate a prefixed id, e.g. `home_01J…`.
#[must_use]
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", Ulid::new())
}

/// Home id prefix.
pub const HOME: &str = "home";
/// Fleet id prefix.
pub const FLEET: &str = "flt";
/// Scenario id prefix.
pub const SCENARIO: &str = "scn";
/// Dispatch command id prefix.
pub const COMMAND: &str = "cmd";
/// Snapshot id prefix.
pub const SNAPSHOT: &str = "snap";
/// Backtest run id prefix.
pub const BACKTEST: &str = "bt";
