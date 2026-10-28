//! Baselines: where existing code stood when RULES.md was adopted
//! (2026-10-23). They may only move toward the rules: shrink a file and lower
//! its entry, raise coverage and raise its floor, never the reverse
//! (RULES.md R-DEV-2, R-COV-2).

/// Files longer than `file_length::MAX_LINES` at adoption, with their length
/// then. Empty: every file is within the limit, and none may be added.
pub const FILE_LENGTH_BASELINE: &[(&str, usize)] = &[];

/// Region coverage may not fall below this percentage.
pub const COVERAGE_FLOOR_REGIONS: f64 = 94.0;
/// Line coverage may not fall below this percentage.
pub const COVERAGE_FLOOR_LINES: f64 = 96.2;
