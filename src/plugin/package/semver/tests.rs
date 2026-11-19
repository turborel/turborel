//! Unit tests of semantic versions and their ranges.

use super::*;

fn v(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn holds(range: &str, version: &str) -> bool {
    Range::parse(range).unwrap().matches(&v(version))
}

/// Verifies: [EXTENSION] design document §13.2: a version is
/// MAJOR.MINOR.PATCH with a prerelease and build metadata, ordered as
/// semantic versioning orders: a prerelease before its release, numeric
/// identifiers before text, build metadata passed over; a leading zero or
/// a missing part is refused.
#[test]
fn versions_parse_and_order_as_semantic_versions() {
    assert_eq!(v("1.2.3+build.5"), Version::new(1, 2, 3));
    assert_eq!(v("1.0.0-alpha.1").to_string(), "1.0.0-alpha.1");
    let ordered = [
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-alpha.beta",
        "1.0.0-beta",
        "1.0.0-beta.2",
        "1.0.0-beta.11",
        "1.0.0-rc.1",
        "1.0.0",
        "1.0.1",
        "1.1.0",
        "2.0.0",
    ];
    for pair in ordered.windows(2) {
        assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        assert!(v(pair[1]) > v(pair[0]), "{} > {}", pair[1], pair[0]);
    }
    for bad in ["1.2", "1.2.3.4", "01.2.3", "1.2.x", "a.b.c", "1.2.3-", "1.2.3-a..b", ""] {
        assert!(Version::parse(bad).is_err(), "{bad}");
    }
}

/// Verifies: [EXTENSION] design document §13.2: ranges npm-style: caret
/// keeps the leftmost part that is not zero, tilde the minor version, x
/// and partial versions any of the versions written, comparators joined
/// by spaces or commas all met, `||` alternatives, and hyphen ranges
/// inclusive.
#[test]
fn ranges_are_npm_style() {
    let cases = [
        ("^1.2.3", "1.2.3", true),
        ("^1.2.3", "1.9.0", true),
        ("^1.2.3", "2.0.0", false),
        ("^1.2.3", "1.2.2", false),
        ("^0.2.3", "0.2.9", true),
        ("^0.2.3", "0.3.0", false),
        ("^0.0.3", "0.0.4", false),
        ("~1.2.3", "1.2.9", true),
        ("~1.2.3", "1.3.0", false),
        ("~1", "1.9.9", true),
        ("1.x", "1.4.0", true),
        ("1.x", "2.0.0", false),
        ("1.2.x", "1.2.7", true),
        ("1.2", "1.3.0", false),
        ("*", "9.9.9", true),
        ("x", "0.0.1", true),
        ("", "3.0.0", true),
        ("=1.2.3", "1.2.3", true),
        ("1.2.3", "1.2.4", false),
        (">=1.2.0, <2.0.0", "1.5.0", true),
        (">=1.2.0 <2.0.0", "2.0.0", false),
        (">= 1.2.0", "1.2.0", true),
        (">1.2", "1.2.9", false),
        (">1.2", "1.3.0", true),
        ("<=1.2", "1.2.9", true),
        ("<1.2", "1.1.9", true),
        ("<1.2", "1.2.0", false),
        ("^1.0.0 || ^3.0.0", "3.1.0", true),
        ("^1.0.0 || ^3.0.0", "2.1.0", false),
        ("1.2.3 - 2.3.4", "2.3.4", true),
        ("1.2.3 - 2.3", "2.3.9", true),
        ("1.2.3 - 2.3", "2.4.0", false),
    ];
    for (range, version, expected) in cases {
        assert_eq!(holds(range, version), expected, "{range} of {version}");
    }
    for bad in [">=", "^x.1", "1.x.3", "abc", "1.2.3.4"] {
        assert!(Range::parse(bad).is_err(), "{bad}");
    }
}

/// Verifies: [EXTENSION] design document §13.2: a prerelease meets a range
/// only when the range names a prerelease of its MAJOR.MINOR.PATCH, so
/// one is taken only when asked for.
#[test]
fn prereleases_come_only_when_asked() {
    assert!(!holds("^1.0.0", "1.1.0-beta.1"));
    assert!(!holds("*", "2.0.0-rc.1"));
    assert!(!holds("<2.0.0", "2.0.0-rc.1"));
    assert!(holds(">=1.1.0-beta.1", "1.1.0-beta.2"));
    assert!(holds("^1.1.0-beta.1", "1.1.0"));
    assert!(!holds(">=1.1.0-beta.1", "1.2.0-beta.1"));
    assert!(holds("=2.0.0-rc.1", "2.0.0-rc.1"));
}
