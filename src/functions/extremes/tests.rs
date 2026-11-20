//! Unit tests of MAX and MIN by a collation (`extremes.rs`): their
//! states merged, and their values given by vector.

use super::*;
use crate::value::collation::{CASE_INSENSITIVE, Pad, builtin};

fn text(s: &str) -> Value {
    Value::Char(s.to_owned())
}

fn insensitive(max: bool) -> CollatedExtreme {
    let collation = builtin().into_iter().find(|c| c.name() == CASE_INSENSITIVE).unwrap();
    CollatedExtreme { max, collation: CollationRef { collation, pad: Pad::Space } }
}

/// Verifies: [DESIGN] RULES.md D53, MAX
/// and MIN of a collated column order by the collation: of 'b', 'C' and
/// 'a' case-insensitive MAX is 'C', MIN 'a'; one accumulator merging the
/// state of another fed the rest gives the same, wherever the values are
/// split; nulls and the rows not asked of a vector are passed over; a
/// vector of integers or doubles adds nothing; a state not one written is
/// refused as corrupt.
#[test]
fn collated_extremes_merge_and_take_vectors() {
    let values = [text("b"), Value::Null, text("C"), text("a")];
    for (max, want) in [(true, "C"), (false, "a")] {
        let f = insensitive(max);
        assert_eq!(f.name(), if max { "MAX" } else { "MIN" });
        let t = DataType::varying(5);
        for at in 0..=values.len() {
            let (mut before, mut after) = (f.accumulator(t.clone()), f.accumulator(t.clone()));
            before.update_many(&values[..at], None).unwrap();
            after.update_many(&values[at..], None).unwrap();
            let mut state = Vec::new();
            assert!(after.write_state(&mut state));
            before.merge(&state).unwrap();
            assert_eq!(before.finish().unwrap().to_string(), want, "{max} at {at}");
        }
        let mut some = f.accumulator(t.clone());
        some.update_many(&values, Some(&[0, 1])).unwrap();
        some.update_ints(&Ints::new(vec![9], 0, None), None).unwrap();
        some.update_floats(&Floats::new(vec![9.0_f64], None), None).unwrap();
        assert_eq!(some.finish().unwrap().to_string(), "b");
        assert_eq!(some.merge(&[0xff_u8]).unwrap_err().code, Code::Corrupt);
    }
    assert_eq!(insensitive(true).result_type(None).unwrap_err().code, Code::SyntaxRule);
}
