//! Unit tests of the conjuncts a column store's scan judges (`judged.rs`).

use std::sync::Arc;

use super::super::encoded::Body;
use super::super::{Chunk, Fields};
use super::*;
use crate::storage::Coded;
use crate::value::Spelled;
use crate::value::datetime::Moment;

const TEXT: DataType = DataType::varying(100);

/// A conjunct comparing column 0 with `value` by `op`.
fn compare(op: CompOp, value: Value) -> Conjunct {
    Conjunct::Condition(Condition { columns: vec![0], probe: Probe::Compare { op, value } })
}

/// The conjuncts each vector is judged by, of `constants`, one of each
/// kind: every comparison, an OR of one and IS NULL, IS NULL and IS NOT
/// NULL, NOT IN, and IN with a null among its constants.
fn conjuncts(constants: &[Value]) -> Vec<Conjunct> {
    let mut out = Vec::new();
    for c in constants {
        for op in [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Gt, CompOp::Le, CompOp::Ge] {
            out.push(compare(op, c.clone()));
        }
        out.push(Conjunct::Any(vec![compare(CompOp::Lt, c.clone()), Conjunct::Null { column: 0, not: false }]));
    }
    out.push(Conjunct::Null { column: 0, not: false });
    out.push(Conjunct::Null { column: 0, not: true });
    let mut listed = constants.to_vec();
    out.push(Conjunct::NotIn { column: 0, values: listed.clone() });
    listed.push(Value::Null);
    out.push(Conjunct::In { column: 0, values: listed });
    out
}

/// Asserts each of `conjuncts` keeps of `column`, of every row and of
/// every third, the rows whose value of `values` it matches, as
/// `matches_key` judges it.
fn judged_as_values(column: &Column, values: &[Value], conjuncts: &[Conjunct], what: &str) {
    let all: Vec<u32> = (0..values.len()).map(|r| u32::try_from(r).unwrap()).collect();
    let given: Vec<u32> = all.iter().copied().step_by(3).collect();
    for conjunct in conjuncts {
        let pred = Pred { at: 0, column: 0, conjunct: conjunct.clone(), of_null: conjunct.matches_key(&[&Value::Null]) };
        let expected =
            |rows: &[u32]| -> Vec<u32> { rows.iter().copied().filter(|&r| conjunct.matches_key(&[&values[index(r)]])).collect() };
        assert_eq!(pred.kept(column, None, &crate::kernels::Portable), Some(expected(&all)), "{what}: {conjunct:?}");
        assert_eq!(pred.kept(column, Some(&given), &crate::kernels::Portable), Some(expected(&given)), "{what}, every third: {conjunct:?}");
    }
}

/// Where each text of `values` ends, a null of none.
fn text_ends(values: &[Value]) -> Vec<usize> {
    let mut end = 0_usize;
    values
        .iter()
        .map(|v| {
            if let Value::Char(s) = v {
                end = end.saturating_add(s.len());
            }
            end
        })
        .collect()
}

/// Verifies: [DESIGN] RULES.md D360, conjuncts judged over vectors:
/// integers of a scale, nulls among them, judged by the kernel, the
/// constant folded to their scale, of constants of a finer scale and past
/// their range too, and of runs, none null, once a run; approximate numbers, of an
/// exact constant too; datetimes; each keeps what each value's own
/// judging keeps.
#[test]
fn numbers_keep_what_their_values_keep() {
    let nulls = Mask::from_bools(&(0_i32..200_i32).map(|n| n % 13_i32 == 0_i32).collect::<Vec<_>>());
    let units: Vec<i64> = (0..200_i64).map(|n| (n * 37) % 101 - 50).collect();
    let ints = || Ints::new(units.clone(), 2, Some(nulls.clone()));
    let values = Column::Ints(ints()).values().to_vec();
    let exact = [
        Value::Exact(Decimal::new(-3, 1)),
        Value::Exact(Decimal::new(25, 2)),
        Value::Exact(Decimal::new(1255, 3)),
        Value::Exact(Decimal::new(i128::from(i64::MAX) * 4, 0)),
    ];
    judged_as_values(&Column::Ints(ints()), &values, &conjuncts(&exact), "integers");
    let sorted: Vec<i64> = (0..200_i64).map(|n| n.checked_div(20).unwrap()).collect();
    let ends: Vec<usize> = (1..=10).map(|n| n * 20).collect();
    let runs = Ints::new(sorted.clone(), 0, None).with_runs(ends);
    assert!(runs.runs().is_some());
    let run_values = Column::Ints(Ints::new(sorted, 0, None)).values().to_vec();
    judged_as_values(&Column::Ints(runs), &run_values, &conjuncts(&[Value::Exact(Decimal::new(4, 0))]), "runs");
    let numbers: Vec<f64> = (0_i32..200_i32).map(|n| f64::from(n) / 8.0_f64 - 3.0_f64).collect();
    let floats = || Floats::new(numbers.clone(), Some(nulls.clone()));
    let float_values = Column::Floats(floats()).values().to_vec();
    let approximate = [Value::Approx(2.5), Value::Exact(Decimal::new(25, 1))];
    judged_as_values(&Column::Floats(floats()), &float_values, &conjuncts(&approximate), "floats");
    let datetimes = || Datetimes::new((0..200).map(|n| n * 86_400_000_000).collect(), None, (Moment::Date, false, 0), Some(nulls.clone()));
    let date_values = Column::Datetimes(datetimes()).values().to_vec();
    let day = date_values[100].clone();
    judged_as_values(&Column::Datetimes(datetimes()), &date_values, &conjuncts(&[day]), "datetimes");
}

/// Verifies: [DESIGN] RULES.md D360, texts judged over vectors: texts,
/// nulls among them, one not ASCII, one with a trailing space, the empty
/// one, and a dictionary's codes of them, each entry judged once, keep
/// what each value's own judging keeps, padded with spaces, of constants
/// found, padded and of a prefix.
#[test]
fn texts_keep_what_their_values_keep() {
    let nulls = Mask::from_bools(&(0_i32..200_i32).map(|n| n % 13_i32 == 0_i32).collect::<Vec<_>>());
    let words = ["red", "grün", "blue", "red ", ""];
    let text_values: Vec<Value> =
        (0..200).map(|n| if n % 13 == 0 { Value::Null } else { Value::Char(words[n % words.len()].into()) }).collect();
    let text: String = text_values.iter().filter_map(|v| if let Value::Char(s) = v { Some(s.as_str()) } else { None }).collect();
    let texts = Column::Texts(Texts::new(text, text_ends(&text_values), Some(nulls.clone())));
    let strings = [Value::Char("red".into()), Value::Char("blue  ".into()), Value::Char("gr".into())];
    judged_as_values(&texts, &text_values, &conjuncts(&strings), "texts");
    let entries = Arc::new(Texts::new(words.concat(), text_ends(&words.map(|w| Value::Char(w.into()))), None));
    let coded = Coded { codes: (0..200).map(|n| u32::try_from(n % words.len()).unwrap()).collect(), entries };
    let dictionary = Column::Texts(Texts::coded(coded, Some(nulls)));
    judged_as_values(&dictionary, &text_values, &conjuncts(&strings), "a dictionary");
}

/// Verifies: [DESIGN] RULES.md D360, texts coded by symbols compared in
/// place: each text of a chunk coded by symbols, some ending in spaces
/// and some in a tab, orders against constants as the decoded text does,
/// padded with spaces, of constants found in it, the same padded, one not
/// found, a prefix, one ordered below a space, the empty string; codes
/// ending in an escape compare as none.
#[test]
fn texts_coded_by_symbols_compare_padded() {
    let texts: Vec<Value> = (0..300_u32)
        .map(|k| {
            if k % 23 == 5 {
                Value::Null
            } else {
                Value::Char(format!("customer{k}@mail{}.example.com{}", k % 7, ["", "  ", "\t"][usize::try_from(k % 3).unwrap()]).into())
            }
        })
        .collect();
    let fields = Fields::of(&TEXT, texts.iter().cloned()).unwrap();
    let bytes = fields.chunk(&TEXT, 0, (fields.len(), true)).unwrap();
    let chunk = Chunk::parse(&bytes).unwrap();
    let Body::Fsst(coded) = &chunk.body else { panic!("not coded by symbols") };
    let constants = [
        "customer10@mail3.example.com",
        "customer11@mail4.example.com",
        "customer7@mail0.example.com",
        "customer7@mail0.example.com  ",
        "customer8@mail1.example.co",
        "customer",
        "customer7@mail0.example.com\t",
        "",
        "zz",
    ];
    for (n, v) in texts.iter().enumerate() {
        let Value::Char(s) = v else { continue };
        for c in constants {
            assert_eq!(coded.compare_padded(n, c.as_bytes()), Some(compare_strings(s, c)), "{s:?} against {c:?}");
        }
    }
    let mut bad = bytes.clone();
    let last = bad.len() - 1;
    bad[last] = 255;
    let broken = Chunk::parse(&bad).unwrap();
    let Body::Fsst(broken) = &broken.body else { panic!("not coded by symbols") };
    assert_eq!(broken.compare_padded(299, b"customer299@mail5.example.com\t"), None, "an escape at the end");
}

/// Verifies: [DESIGN] RULES.md D360, the conjuncts the scan judges: of one
/// column of a type it decodes into a vector, a comparison, NOT IN; not
/// one of two columns, nor one of a type it does not judge.
#[test]
fn the_scan_judges_conjuncts_of_one_column() {
    let integer = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
    let types = [integer, TEXT, DataType::Json];
    let two = Conjunct::Condition(Condition { columns: vec![0, 1], probe: Probe::Compare { op: CompOp::Eq, value: Value::Null } });
    let of_json = Conjunct::Null { column: 2, not: false };
    let judged =
        [compare(CompOp::Lt, Value::Exact(Decimal::new(3, 0))), Conjunct::NotIn { column: 1, values: vec![Value::Char("a".into())] }];
    let all = Conjunction(judged.into_iter().chain([two, of_json]).collect());
    let judgment = Judgment::of(Some(&all), &types);
    assert_eq!(judgment.preds.iter().map(|p| p.column).collect::<Vec<_>>(), vec![0, 1]);
    assert!(Judgment::of(None, &types).is_empty());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.2] (RULES.md D1183) a
/// conjunct compared unpadded is judged by the scan over texts and a
/// dictionary's entries by their octets, a prefix the less and a trailing
/// space apart, alone and in an OR with a padded comparison; and no
/// dictionary's order nor sketch takes such a constant.
#[test]
fn a_conjunct_unpadded_is_judged_by_octets() {
    let told = |op: CompOp, text: &str| Conjunct::Unpadded { column: 0, op, octets: text.as_bytes().to_vec() };
    assert!(judged_conjunct(&told(CompOp::Eq, "a")) && judged_conjunct(&Conjunct::Any(vec![told(CompOp::Eq, "a")])));
    let words = ["a", "a ", "a\t", "", "ab"];
    let values: Vec<Value> = (0..60).map(|n| if n % 7 == 6 { Value::Null } else { Value::Char(words[n % words.len()].into()) }).collect();
    let nulls = Mask::from_bools(&(0_usize..60).map(|n| n % 7 == 6).collect::<Vec<_>>());
    let text: String = values.iter().filter_map(|v| if let Value::Char(s) = v { Some(s.as_str()) } else { None }).collect();
    let plain = Column::Texts(Texts::new(text, text_ends(&values), Some(nulls.clone())));
    let entries = Arc::new(Texts::new(words.concat(), text_ends(&words.map(|w| Value::Char(w.into()))), None));
    let coded = Coded { codes: (0..60).map(|n| u32::try_from(n % words.len()).unwrap()).collect(), entries };
    let dictionary = Column::Texts(Texts::coded(coded, Some(nulls)));
    let rows_where = |keeps: &dyn Fn(&str) -> bool| -> Vec<u32> {
        (0..60_u32).filter(|&r| if let Value::Char(s) = &values[index(r)] { keeps(s) } else { false }).collect()
    };
    let either = Conjunct::Any(vec![told(CompOp::Eq, "a "), compare(CompOp::Eq, Value::Char("ab  ".into()))]);
    let cases: [(Conjunct, Vec<u32>); 6] = [
        (told(CompOp::Eq, "a"), rows_where(&|s| s == "a")),
        (told(CompOp::Ne, ""), rows_where(&|s| !s.is_empty())),
        (told(CompOp::Lt, "a "), rows_where(&|s| s.as_bytes() < b"a ".as_slice())),
        (told(CompOp::Ge, "a\t"), rows_where(&|s| s.as_bytes() >= b"a\t".as_slice())),
        (told(CompOp::Le, "a"), rows_where(&|s| s.is_empty() || s == "a")),
        (either, rows_where(&|s| s == "a " || s == "ab")),
    ];
    for (conjunct, wanted) in &cases {
        let pred = Pred { at: 0, column: 0, conjunct: conjunct.clone(), of_null: conjunct.matches_key(&[&Value::Null]) };
        assert!(!pred.of_null);
        for (what, column) in [("texts", &plain), ("a dictionary", &dictionary)] {
            assert_eq!(pred.kept(column, None, &crate::kernels::Portable).as_ref(), Some(wanted), "{what}: {conjunct:?}");
        }
        let mut constants = Vec::new();
        assert_eq!(sorted::constants_of(conjunct, &mut constants), None, "no order of a dictionary: {conjunct:?}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.2] (RULES.md D1183) a
/// chunk coded by symbols compares each text with a constant's octets in
/// place, unpadded, as the decoded text's octets order against it: of
/// constants found, the same with spaces or a tab after, a prefix, the
/// empty string and one past every text; codes ending in an escape
/// compare as none.
#[test]
fn texts_coded_by_symbols_compare_unpadded() {
    let texts: Vec<Value> = (0..300_u32)
        .map(|k| Value::Char(format!("customer{k}@mail{}.example.com{}", k % 7, ["", "  ", "\t"][index(k % 3)]).into()))
        .collect();
    let fields = Fields::of(&TEXT, texts.iter().cloned()).unwrap();
    let bytes = fields.chunk(&TEXT, 0, (fields.len(), true)).unwrap();
    let chunk = Chunk::parse(&bytes).unwrap();
    let Body::Fsst(symbols) = &chunk.body else { panic!("not coded by symbols") };
    let constants = [
        "customer10@mail3.example.com",
        "customer7@mail0.example.com  ",
        "customer7@mail0.example.com\t",
        "customer",
        "",
        "zz",
        "customer8@mail1.example.co",
    ];
    for (n, v) in texts.iter().enumerate() {
        let Value::Char(s) = v else { continue };
        for c in constants {
            assert_eq!(symbols.compare_unpadded(n, c.as_bytes()), Some(s.as_bytes().cmp(c.as_bytes())), "{s:?} against {c:?}");
        }
    }
    let mut bad = bytes.clone();
    let last = bad.len() - 1;
    bad[last] = 255;
    let broken = Chunk::parse(&bad).unwrap();
    let Body::Fsst(broken) = &broken.body else { panic!("not coded by symbols") };
    assert_eq!(broken.compare_unpadded(299, b"customer299@mail5.example.com\t"), None, "an escape at the end");
}
