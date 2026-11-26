use super::*;
use crate::params::conformed;
use crate::value::Spelled;

/// A lane of `cells`, a null where `None`.
fn lane(cells: &[Option<&str>]) -> Lane {
    let mut lane = Lane { nulls: vec![0; cells.len().div_ceil(8)], ..Lane::default() };
    for (n, cell) in cells.iter().enumerate() {
        match cell {
            Some(text) => lane.bytes.extend_from_slice(text.as_bytes()),
            None => set_null(&mut lane.nulls, n),
        }
        lane.ends.push(u32::try_from(lane.bytes.len()).unwrap());
    }
    lane
}

/// The records the rows give of `cells`, each a row's character string
/// cast as a dynamic parameter of `t`, refused if the cast warns,
/// assigned to a column of `t` and encoded, or the first error: the
/// reference a lane's parse must equal, written apart from it.
fn by_rows(t: &DataType, cells: &[Option<&str>]) -> Result<Records> {
    let mut out = Records { nulls: vec![0; cells.len().div_ceil(8)], ..Records::default() };
    for (n, cell) in cells.iter().enumerate() {
        let value = match cell {
            None => Value::Null,
            Some(text) => match conformed(Value::Char(CharString::from((*text).to_owned())), t)? {
                (value, None) => assign(value, t, "ISO/IEC 9075:1992 §13.8 GR4b iii")?,
                (_, Some(_)) => return Err(SqlError::new(Code::Api, "[DESIGN] a value given with others warns as it is cast")),
            },
        };
        if value.is_null() {
            set_null(&mut out.nulls, n);
        } else {
            out.bytes.extend_from_slice(&t.encode_record(&value)?);
        }
        out.ends.push(u32::try_from(out.bytes.len()).unwrap_or(u32::MAX));
    }
    Ok(out)
}

/// The parse of `cells` as `t` equals the rows' records, or their error,
/// code, SQLSTATE and message.
fn same(t: &DataType, cells: &[Option<&str>]) {
    let parsed = parse(t, &lane(cells), cells.len());
    let rows = by_rows(t, cells);
    match (&parsed, &rows) {
        (Ok(p), Ok(r)) => assert_eq!(p, r, "{t} of {cells:?}"),
        (Err(p), Err(r)) => assert_eq!(format!("{p:?}"), format!("{r:?}"), "{t} of {cells:?}"),
        (p, r) => panic!("{t} of {cells:?}: parsed {p:?}, rows {r:?}"),
    }
}

/// Each of `texts` alone, then all of them as one lane, with a null.
fn each_and_all(t: &DataType, texts: &[&str]) {
    for text in texts {
        same(t, &[Some(text)]);
    }
    let mut cells: Vec<Option<&str>> = texts.iter().map(|s| Some(*s)).collect();
    cells.insert(cells.len().div_ceil(2), None);
    same(t, &cells);
}

/// Whether `text` parses as `t` to a record of its own, written straight.
fn fast(t: &DataType, text: &str) -> bool {
    Fast::of(t).write(text, &mut Vec::new())
}

fn exact(precision: u32, scale: u32) -> DataType {
    DataType::Exact { precision, scale, spelled: Spelled::Numeric }
}

const SMALLINT: DataType = DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Smallint };
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
const BIGINT: DataType = DataType::Exact { precision: 19, scale: 0, spelled: Spelled::Bigint };

/// Numerals of every kind the cast reads, and many it refuses.
const NUMERALS: [&str; 52] = [
    "0",
    "-0",
    "+0",
    "00",
    "-0.00",
    "1",
    "-1",
    "+1",
    "007",
    "-007",
    "+0012",
    "99999",
    "-99999",
    "100000",
    "-100000",
    "2147483647",
    "-2147483648",
    "9999999999",
    "10000000000",
    "1.5",
    "-1.5",
    "1.",
    ".5",
    "-.5",
    "1.25",
    "12.345",
    "0.001",
    " 12",
    "12 ",
    "  -3  ",
    "1 2",
    "",
    " ",
    "-",
    "+",
    ".",
    "-.",
    "1e3",
    "1E-2",
    "1_000",
    "١",
    "abc",
    "12a",
    "--1",
    "+-1",
    "1..2",
    "1.2.3",
    "\t1",
    "0x1F",
    "Infinity",
    "999999999999999999",
    "-999999999999999999",
];

/// Verifies: [DESIGN] RULES.md D1148 [ISO/IEC 9075:1992 §6.10 GR3b,
/// §9.2 GR3k]: SMALLINT, INTEGER and BIGINT parse as their rows give
/// them, records or the first error: bounds of each width and past them,
/// zero and negative zero, leading zeros and a plus sign, spaces, empty
/// strings, nulls, exponents, underscores and what is no numeral.
#[test]
fn integers_parse_as_rows_give_them() {
    let (nines18, nines19, nines38, nines39) = ("9".repeat(18), "9".repeat(19), "9".repeat(38), "9".repeat(39));
    let (negative38, zeros) = (format!("-{nines38}"), format!("{}1", "0".repeat(50)));
    for t in [SMALLINT, INTEGER, BIGINT, exact(18, 0), exact(38, 0)] {
        each_and_all(&t, &NUMERALS);
        each_and_all(&t, &[&nines18, &nines19, &nines38, &nines39, &negative38, &zeros]);
    }
    assert!(fast(&INTEGER, "-0012"));
    assert!(fast(&INTEGER, " 9999999999 "));
    assert!(!fast(&INTEGER, "10000000000"), "past the precision, the row's error");
    assert!(!fast(&SMALLINT, "1.5"), "a fraction an integer truncates");
}

/// Verifies: [DESIGN] RULES.md D1148 [ISO/IEC 9075:1992 §6.10 GR3b,
/// §9.2 GR3k]: NUMERIC and DECIMAL of many precisions and scales parse as
/// their rows give them: fractions of fewer digits than the scale, of as
/// many and of more, which the cast truncates; whole digits up to the
/// precision's room and past it; the most precision, narrow and wide.
#[test]
fn decimals_parse_as_rows_give_them() {
    let wide = format!("{}.{}", "9".repeat(20), "9".repeat(18));
    let texts =
        ["123.45", "-123.45", "123.456", "123.4", "0.99", "-0.995", "999.99", "1000.00", "999.995", "00999.990", "+.01", wide.as_str()];
    for t in [exact(5, 2), exact(18, 2), exact(18, 18), exact(19, 4), exact(38, 18), exact(38, 0), exact(1, 1), exact(3, 0)] {
        each_and_all(&t, &NUMERALS);
        each_and_all(&t, &texts);
    }
    assert!(fast(&exact(5, 2), "-123.4"));
    assert!(!fast(&exact(5, 2), "123.456"), "more fraction digits than the scale fall back");
    assert!(!fast(&exact(5, 2), "1234.5"), "more whole digits than the precision leaves fall back");
    assert!(fast(&exact(38, 18), &wide));
}

/// Verifies: [DESIGN] RULES.md D1148 [ISO/IEC 9075:1992 §6.10 GR4b]:
/// REAL, DOUBLE PRECISION and FLOAT(p) parse as their rows give them, to
/// the bit: exact numerals rounded to single precision or not, long
/// ones, exponents and values past REAL's range, which the rows read.
#[test]
fn approximate_numbers_parse_as_rows_give_them() {
    let long = format!("0.{}", "1".repeat(37));
    let big = "9".repeat(38);
    let texts = ["0.1", "-0.1", "3.14159265358979323846", "16777217", "1e38", "3.5e38", "1e309", "-1E-400", &long, &big, "-0"];
    for precision in [1, 24, 25, 53] {
        let t = DataType::Approximate { precision, spelled: Spelled::Float };
        each_and_all(&t, &NUMERALS);
        each_and_all(&t, &texts);
        assert!(fast(&t, "0.1"));
        assert!(!fast(&t, "1e3"), "an exponent the row reads");
    }
}

/// Verifies: [DESIGN] RULES.md D1148 [ISO/IEC 9075:1992 §6.10 GR5c,
/// GR6c, §9.2 GR3b, GR3e]: CHARACTER and CHARACTER VARYING, of a length
/// in characters or octets, parse as their rows give them: shorter,
/// exactly the length, longer by spaces alone, which the row cuts, and
/// longer by other characters, which warns, so the lane is refused as a
/// row with others is; trailing and leading spaces, empty strings and
/// characters of many octets.
#[test]
fn strings_parse_as_rows_give_them() {
    let texts = ["", " ", "a", "ab", "abc", "abcd", "abc ", "abcd  ", "  ab", "a  b", "é", "éé", "ééé", "éééé", "日本", "ab\u{0}"];
    for units in [CharUnits::Characters, CharUnits::Octets] {
        for length in [1, 3, 4, 10] {
            for t in [DataType::Character { length, units }, DataType::Varying { length, units }] {
                each_and_all(&t, &texts);
            }
        }
    }
    let char3 = DataType::character(3);
    assert!(fast(&char3, "ab "));
    assert!(!fast(&char3, "abcd"), "longer than the length, the row cuts or warns");
    let mut out = Vec::new();
    assert!(Fast::of(&char3).write("a  ", &mut out));
    assert_eq!(out, b"a", "a CHARACTER's record has no trailing spaces");
    assert!(!fast(&DataType::Varying { length: 3, units: CharUnits::Octets }, "éé"));
}

/// Verifies: [DESIGN] RULES.md D1148 [ISO/IEC 9075:1992 §6.10 GR9a,
/// GR10a, GR11a, §9.2 GR3l]: DATE, TIME and TIMESTAMP, with time zone or
/// not, of many precisions, parse as their rows give them: dates at the
/// year range's bounds, leap days and dates that are none, fractions of
/// more digits than the precision, displacements, spaces and odd forms.
#[test]
fn datetimes_parse_as_rows_give_them() {
    let dates = [
        "2024-02-29",
        "2023-02-29",
        "1900-02-29",
        "2000-02-29",
        "0001-01-01",
        "9999-12-31",
        "0000-12-31",
        "10000-01-01",
        "2024-13-01",
        "2024-00-10",
        "2024-04-31",
        " 2024-01-05 ",
        "2024-1-5",
        "2024/01/05",
        "",
        "today",
    ];
    each_and_all(&DataType::Date, &dates);
    let times =
        ["00:00:00", "23:59:59", "24:00:00", "12:30:45.123456789", "12:30:45.5", "12:30", "1:02:03", "12:30:45+05:30", "12:30:45-12:59"];
    let stamps = [
        "2024-02-29 23:59:59.999999",
        "9999-12-31 23:59:59.9999999",
        "0001-01-01 00:00:00",
        "0001-01-01 00:00:00+01:00",
        "2024-03-10 02:30:00",
        "2024-03-10T02:30:00",
        "2024-03-10  02:30:00",
        "2024-02-30 00:00:00",
        "9999-12-31 23:59:59-02:00",
        " 2024-01-05 10:00:00 ",
    ];
    for zoned in [false, true] {
        for precision in [0, 3, 6] {
            each_and_all(&DataType::Time { precision, zoned }, &times);
            each_and_all(&DataType::Timestamp { precision, zoned }, &stamps);
            each_and_all(&DataType::Timestamp { precision, zoned }, &dates);
        }
    }
    assert!(fast(&DataType::Date, "2024-02-29"));
    assert!(!fast(&DataType::Date, "2023-02-29"), "no such day, the row's error");
}

/// Verifies: [DESIGN] RULES.md D1148 [ISO/IEC 9075-2:2023 §6.13 GR21a]:
/// BOOLEAN parses as its rows give it: TRUE and FALSE in any case and
/// spaced, UNKNOWN a null, and what is no truth value refused.
#[test]
fn truth_values_parse_as_rows_give_them() {
    each_and_all(&DataType::Boolean, &["TRUE", "true", "False", " FALSE ", "unknown", "UNKNOWN", "", "yes", "1", "t", "TRUEX", "ＴＲＵＥ"]);
    let parsed = parse(&DataType::Boolean, &lane(&[Some("Unknown"), Some("true")]), 2).unwrap();
    assert!(parsed.is_null(0), "UNKNOWN is the null value");
    assert_eq!(parsed.record(1), Some(&[TRUE_BYTE][..]));
}

/// Verifies: [DESIGN] RULES.md D1148: a type with no fast path, DECFLOAT,
/// parses every cell as its row gives it.
#[test]
fn types_with_no_fast_path_parse_as_rows_give_them() {
    each_and_all(&DataType::DecFloat { precision: 16 }, &NUMERALS);
    assert!(!fast(&DataType::DecFloat { precision: 16 }, "1"));
}

/// Verifies: [DESIGN] RULES.md D1148: a lane of another number of cells
/// than the rows is refused, as `Lane::check` refuses it; a lane of no
/// rows parses to no records.
#[test]
fn a_lane_of_another_number_of_cells_is_refused() {
    assert!(parse(&INTEGER, &lane(&[Some("1"), Some("2")]), 3).is_err());
    assert!(parse(&INTEGER, &lane(&[]), 0).unwrap().is_empty());
}

/// A deterministic sequence of numbers, each from the one before.
fn next(x: &mut u64) -> u64 {
    *x ^= x.wrapping_shl(13);
    *x ^= x.wrapping_shr(7);
    *x ^= x.wrapping_shl(17);
    *x
}

/// The text of a generated numeral, from `r`: a sign, up to 23 whole
/// digits, up to 5 of a fraction, some spaced; none, a null, for some.
fn numeral_of(r: u64) -> Option<String> {
    let digit = |k: u64| char::from(b'0'.wrapping_add(u8::try_from(k % 10).unwrap()));
    let whole: String = (0..r % 24).map(|k| digit(r.wrapping_shr(u32::try_from(k.wrapping_mul(2)).unwrap()))).collect();
    let fraction: String = (0..r.wrapping_shr(40) % 6).map(|k| digit(r.wrapping_shr(u32::try_from(k).unwrap()))).collect();
    let sign = ["", "-", "+", "", ""][usize::try_from(r.wrapping_shr(50) % 5).unwrap()];
    match r.wrapping_shr(56) % 8 {
        0 => None,
        1 => Some(format!(" {sign}{whole}.{fraction} ")),
        2 | 3 => Some(format!("{sign}{whole}.{fraction}")),
        _ => Some(format!("{sign}{whole}")),
    }
}

/// Verifies: [DESIGN] RULES.md D1148: a generated sweep of integers and
/// decimals, of every digit count, sign, leading zeros and fraction
/// length, some with spaces and some null, parses as whole lanes to the
/// rows' records, or the rows' first error, for integer and decimal types
/// narrow and wide and an approximate one.
#[test]
fn a_generated_sweep_parses_as_rows_give_it() {
    let mut x = 0x2545_F491_4F6C_DD1D_u64;
    let texts: Vec<Option<String>> = (0..3000_u32).map(|_| numeral_of(next(&mut x))).collect();
    let float = DataType::Approximate { precision: 53, spelled: Spelled::Float };
    for t in [SMALLINT, INTEGER, BIGINT, exact(9, 3), exact(18, 5), exact(30, 5), float] {
        // Lanes of the cells the type takes, so no lane is refused at its
        // first cell alone; then every cell alone.
        let taken: Vec<Option<&str>> =
            texts.iter().map(Option::as_deref).filter(|c| c.is_none_or(|s| by_rows(&t, &[Some(s)]).is_ok())).collect();
        assert!(taken.len() > 300, "{t} takes a part of the sweep");
        for chunk in taken.chunks(97) {
            same(&t, chunk);
        }
        for cell in &texts {
            same(&t, &[cell.as_deref()]);
        }
    }
}
