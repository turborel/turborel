//! Refusals and resource limits of geometry interchange.

use super::*;

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.67] D85:
/// malformed numeric literals, arity, separators and trailing data are
/// data errors. Ring cardinality and closure, and both finite-coordinate
/// conditions, fail independently; every type and coordinate dimension
/// is read, as its linear form of x and y.
#[test]
fn malformed_text_never_becomes_a_partial_geometry() {
    for text in [
        "",
        "POINT ()",
        "POINT (1)",
        "POINT (1 2 3)",
        "POINT (1 2, 3 4)",
        "POINT (NaN 2)",
        "POINT (1 inf)",
        "POINT (1e999 0)",
        "POINT (0 -1e999)",
        "POINT (1e 2)",
        "POINT (. 2)",
        "POINT (1 2) x",
        "POINT EMPTY junk",
        "LINESTRING (1 2)",
        "LINESTRING (1 2,)",
        "POLYGON (EMPTY)",
        "POLYGON ((0 0, 1 0, 0 0))",
        "POLYGON ((0 0, 1 0, 1 1, 0 1))",
        "GEOMETRYCOLLECTION ()",
        "POINTZ (1 2 3)",
    ] {
        assert_eq!(Geometry::from_wkt(text, 0).unwrap_err().code, Code::Data, "{text}");
    }
    for text in ["CIRCULARSTRING (0 0, 1 1, 2 2)", "POINT Z (1 2 3)", "POINT M EMPTY", "POINT ZM EMPTY"] {
        assert!(Geometry::from_wkt(text, 0).is_ok(), "{text}");
    }
    assert_eq!(Geometry::from_wkt("POINT Z (1 2 3)", 0).unwrap().to_wkt(), "POINT (1 2)");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.68] D85:
/// truncation at every byte but after the header, which leaves the value
/// empty, trailing bytes, invalid byte order, overclaimed counts and
/// mismatched homogeneous components are rejected; EWKB flags and codes
/// of no type are refused; Z and M codes are read.
#[test]
fn malformed_binary_counts_and_tags_are_rejected() {
    let g = Geometry::from_wkt("POLYGON ((0 0, 2 0, 2 2, 0 0))", 0).unwrap();
    let bytes = g.to_wkb();
    for end in (0..bytes.len()).filter(|end| *end != HEADER) {
        assert_eq!(Geometry::from_wkb(&bytes[..end], 0).unwrap_err().code, Code::Data, "{end}");
    }
    assert!(Geometry::from_wkb(&bytes[..HEADER], 0).unwrap().is_empty());
    let mut extra = bytes.clone();
    extra.push(0);
    assert_eq!(Geometry::from_wkb(&extra, 0).unwrap_err().code, Code::Data);
    extra[0] = 2;
    assert_eq!(Geometry::from_wkb(&extra, 0).unwrap_err().code, Code::Data);
    for (code, known) in [(8_u32, true), (1001, true), (2001, true), (3001, true), (99, false), (0x2000_0001, false)] {
        let mut header = vec![1];
        header.extend_from_slice(&code.to_le_bytes());
        assert_eq!(
            Geometry::from_wkb(&header, 0).map_err(|e| e.code).map(|read| read.is_empty()),
            if known { Ok(true) } else { Err(Code::Data) }
        );
    }
    let mut count = vec![1, 2, 0, 0, 0];
    count.extend_from_slice(&10_u32.to_le_bytes());
    assert_eq!(Geometry::from_wkb(&count, 0).unwrap_err().code, Code::Data);
    let mut wrong = vec![1, 4, 0, 0, 0, 1, 0, 0, 0];
    wrong.extend_from_slice(&Geometry::from_wkt("LINESTRING EMPTY", 0).unwrap().to_wkb());
    assert_eq!(Geometry::from_wkb(&wrong, 0).unwrap_err().code, Code::Data);
}

/// The bytes of a component's byte order and code.
const HEADER: usize = 5;

/// Verifies: [EXTENSION] D85: only two NaNs in a point denote EMPTY;
/// either NaN alone, either infinity, and NaNs in a line are invalid.
#[test]
fn empty_point_nan_convention_does_not_admit_nonfinite_coordinates() {
    for (xy, empty) in [
        ([f64::NAN, f64::NAN], true),
        ([f64::NAN, 0.0_f64], false),
        ([0.0_f64, f64::NAN], false),
        ([f64::INFINITY, 0.0_f64], false),
        ([0.0_f64, f64::NEG_INFINITY], false),
    ] {
        let mut bytes = vec![1, 1, 0, 0, 0];
        for v in xy {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let parsed = Geometry::from_wkb(&bytes, 0);
        if empty {
            assert!(parsed.unwrap().is_empty());
        } else {
            assert_eq!(parsed.unwrap_err().code, Code::Data);
        }
    }
    let mut line = vec![1, 2, 0, 0, 0, 2, 0, 0, 0];
    for v in [f64::NAN, f64::NAN, 0.0_f64, 0.0_f64] {
        line.extend_from_slice(&v.to_le_bytes());
    }
    assert_eq!(Geometry::from_wkb(&line, 0).unwrap_err().code, Code::Data);
}

/// Verifies: [DESIGN] D85, R-CF-6: WKT/WKB nesting and component budgets
/// agree; oversized text and hostile binary counts stop before allocation.
#[test]
fn representation_limits_are_checked_before_unbounded_work() {
    let mut text = "POINT EMPTY".to_owned();
    for _ in 1..MAX_DEPTH {
        text = format!("GEOMETRYCOLLECTION ({text})");
    }
    let g = Geometry::from_wkt(&text, 0).unwrap();
    assert_eq!(Geometry::from_wkb(&g.to_wkb(), 0).unwrap().to_wkt(), text);
    let mut bytes = vec![1, 7, 0, 0, 0, 1, 0, 0, 0];
    bytes.extend_from_slice(&g.to_wkb());
    assert_eq!(Geometry::from_wkb(&bytes, 0).unwrap_err().code, Code::Unsupported);
    assert_eq!(Geometry::from_wkt(&format!("GEOMETRYCOLLECTION ({text})"), 0).unwrap_err().code, Code::Unsupported);
    assert_eq!(Geometry::from_wkt(&" ".repeat(MAX_BYTES.saturating_add(1)), 0).unwrap_err().code, Code::Unsupported);
    let count = [1, 2, 0, 0, 0, 255, 255, 255, 255];
    assert_eq!(Geometry::from_wkb(&count, 0).unwrap_err().code, Code::Unsupported);
    let line = format!("LINESTRING ({})", vec!["0 0"; MAX_PARTS.saturating_sub(1)].join(","));
    let long_line = Geometry::from_wkt(&line, 0).unwrap();
    assert!(Geometry::from_wkb(&long_line.to_wkb(), 0).is_ok());
    let empty_points = format!("MULTIPOINT ({})", vec!["EMPTY"; MAX_PARTS.saturating_sub(1)].join(","));
    let collection = Geometry::from_wkt(&empty_points, 0).unwrap();
    assert!(Geometry::from_wkb(&collection.to_wkb(), 0).unwrap().is_empty());
    let too_many = format!("MULTIPOINT ({})", vec!["(0 0)"; MAX_PARTS].join(","));
    assert_eq!(Geometry::from_wkt(&too_many, 0).unwrap_err().code, Code::Unsupported);
}
