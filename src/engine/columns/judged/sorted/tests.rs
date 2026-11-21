//! Unit tests of conjuncts judged by the codes of a dictionary in order
//! (`sorted.rs`).

use super::super::super::{Chunk, Fields};
use super::*;
use crate::value::{CharUnits, CompOp, DataType};

/// A conjunct comparing column 0 with the string `c` by `op`.
fn compare(op: CompOp, c: &str) -> Conjunct {
    Conjunct::Condition(Condition { columns: vec![0], probe: Probe::Compare { op, value: Value::Char(c.to_owned()) } })
}

/// The conjuncts judged, of `constants`: every comparison of each, a LIKE
/// prefix as its bounds, IN with a null, NOT IN, an OR, IS NULL and IS
/// NOT NULL.
fn conjuncts(constants: &[&str]) -> Vec<Conjunct> {
    let mut out = Vec::new();
    for c in constants {
        for op in [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Gt, CompOp::Le, CompOp::Ge] {
            out.push(compare(op, c));
        }
    }
    out.push(compare(CompOp::Ge, "gr"));
    out.push(compare(CompOp::Lt, "gs"));
    let strings: Vec<Value> = constants.iter().map(|c| Value::Char((*c).to_owned())).collect();
    out.push(Conjunct::NotIn { column: 0, values: strings.clone() });
    out.push(Conjunct::In { column: 0, values: strings.into_iter().chain([Value::Null]).collect() });
    out.push(Conjunct::Any(vec![compare(CompOp::Lt, "blue"), compare(CompOp::Eq, "red")]));
    out.push(Conjunct::Null { column: 0, not: false });
    out.push(Conjunct::Null { column: 0, not: true });
    out
}

/// Verifies: [DESIGN] RULES.md D512, dictionaries in order: of texts of
/// VARCHAR and CHARACTER, nulls among them, one not ASCII, one with a
/// trailing space and the empty one, a chunk's dictionary is written in
/// order as strings compare and reads back each value; each conjunct
/// judged by its codes, of every row and of every third, keeps the rows
/// whose value it matches; a dictionary of numbers is not in order.
#[test]
fn dictionaries_in_order_judge_by_codes() {
    let words = ["red", "grün", "blue", "red ", "", "green", "gray", "zinc"];
    let types = [DataType::varying(20), DataType::Character { length: 6, units: CharUnits::Characters }];
    let values: Vec<Value> =
        (0..300).map(|n| if n % 13 == 0 { Value::Null } else { Value::Char(words[n * 7 % words.len()].to_owned()) }).collect();
    for t in types {
        let fields = Fields::of(&t, values.iter().cloned()).unwrap();
        let bytes = fields.chunk(&t, 0, (fields.len(), true)).unwrap();
        let chunk = Chunk::parse(&bytes).unwrap();
        let Body::Dictionary { sorted, offsets, entries, .. } = &chunk.body else { panic!("not a dictionary") };
        assert!(*sorted, "{t:?} in order");
        let at = |e: usize| read_word(offsets, e * WORD).unwrap();
        let texts: Vec<&str> =
            (0..offsets.len().checked_div(WORD).unwrap() - 1).map(|e| std::str::from_utf8(&entries[at(e)..at(e + 1)]).unwrap()).collect();
        assert!(texts.windows(2).all(|w| compare_strings(w[0], w[1]).is_le()), "{texts:?}");
        let read: Vec<Value> = (0..values.len()).map(|n| chunk.value(n, &t).unwrap()).collect();
        let back = Fields::of(&t, values.iter().cloned()).unwrap();
        let again = Fields::of(&t, read.iter().cloned()).unwrap();
        assert_eq!(back.0, again.0, "values read back");
        let all: Vec<u32> = (0..300).collect();
        let third: Vec<u32> = all.iter().copied().step_by(3).collect();
        for conjunct in conjuncts(&["red", "grün", "blue  ", "gr", "", "zz"]) {
            let pred = Pred { at: 0, column: 0, conjunct: conjunct.clone(), of_null: conjunct.matches_key(&[&Value::Null]) };
            for rows in [&all, &third] {
                let mut out = Vec::new();
                judged(&chunk, &pred, (0, 0), rows, &mut out).unwrap();
                let expected: Vec<u32> = rows.iter().copied().filter(|&r| conjunct.matches_key(&[&read[index(r)]])).collect();
                assert_eq!(out, expected, "{t:?}: {conjunct:?}");
            }
        }
    }
    let integer = DataType::Exact { precision: 18, scale: 0, spelled: crate::value::Spelled::Bigint };
    let numbers = Fields::of(
        &integer,
        (0_i32..300_i32).map(|n| Value::Exact(crate::value::Decimal::new(i128::from(n % 3_i32) * 1_000_000_007_i128, 0))),
    )
    .unwrap();
    let bytes = numbers.chunk(&integer, 0, (numbers.len(), false)).unwrap();
    if let Body::Dictionary { sorted, .. } = &Chunk::parse(&bytes).unwrap().body {
        assert!(!*sorted, "numbers as first seen");
    }
}
