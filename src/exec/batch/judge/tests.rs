//! Unit tests of filters judged over vectors (`judge.rs`).

use super::*;
use crate::storage::Floats;
use crate::value::{ArithOp, Decimal};

fn int(n: i128) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n, 0)))
}

fn number(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

fn compare(a: Expr, op: CompOp, b: Expr) -> Cond {
    Cond::Compare(a, op, b)
}

fn and(a: Cond, b: Cond) -> Cond {
    Cond::And(Box::new(a), Box::new(b))
}

fn or(a: Cond, b: Cond) -> Cond {
    Cond::Or(Box::new(a), Box::new(b))
}

/// Verifies: [DESIGN] RULES.md D31, filters by batch: the conjuncts
/// comparing a column the batch holds with a constant, a constant first
/// turned around, testing it for null, or testing it IN or NOT IN a list
/// of constants are judged over vectors; the others, an OR of two columns,
/// and those of a column the batch lacks, are the rest; a filter that can
/// raise an error is all rest.
#[test]
fn conjuncts_judged_over_vectors() {
    let list = or(
        or(compare(Expr::Column(4), CompOp::Eq, int(1)), compare(int(2), CompOp::Eq, Expr::Column(4))),
        compare(Expr::Column(4), CompOp::Eq, int(3)),
    );
    let two = or(compare(Expr::Column(4), CompOp::Eq, int(1)), compare(Expr::Column(2), CompOp::Eq, int(2)));
    let cond = and(
        and(compare(Expr::Column(2), CompOp::Lt, int(5)), compare(int(3), CompOp::Lt, Expr::Column(4))),
        and(
            and(compare(Expr::Column(9), CompOp::Eq, int(1)), Cond::IsNull(Expr::Column(2), true)),
            and(and(list.clone(), Cond::Not(Box::new(list))), two),
        ),
    );
    let (judged, rest) = split(&cond, &[4, 2]);
    let shown: Vec<String> = judged.iter().map(|j| format!("{j:?}")).collect();
    assert_eq!(shown.len(), 5, "{shown:?}");
    assert!(shown[1].contains("Compare(0, Gt"), "turned around: {shown:?}");
    assert!(shown[2].starts_with("IsNull(1, true)"), "{shown:?}");
    assert!(shown[3].starts_with("In(0,") && shown[4].ends_with("true)"), "{shown:?}");
    assert!(matches!(rest, Some(Cond::And(..))), "the column the batch lacks, and the OR of two columns");
    let raising = and(
        compare(Expr::Column(2), CompOp::Lt, int(5)),
        compare(Expr::Arith(Box::new(int(1)), ArithOp::Div, Box::new(Expr::Column(2))), CompOp::Gt, int(0)),
    );
    let (none_judged, all_rest) = split(&raising, &[2]);
    assert!(none_judged.is_empty() && all_rest.is_some(), "it can raise");
    let (one, left) = split(&compare(Expr::Column(2), CompOp::Eq, int(1)), &[2]);
    assert_eq!((one.len(), left.is_none()), (1, true), "nothing left");
}

/// Verifies: [X3.135-1986 §5.13, §5.15, §5.18] what each conjunct judged
/// over vectors keeps, as a row's judgment does: IS NULL a null, never
/// unknown; IN a value equal to one of the list, not a null; NOT IN a
/// value equal to none, not a null, nor any value if the list holds a
/// null, every comparison then unknown or false.
#[test]
fn what_each_conjunct_keeps() {
    let list = vec![number(1), number(2)];
    assert!(Judged::IsNull(0, false).keeps(&Value::Null) && !Judged::IsNull(0, false).keeps(&number(1)));
    assert!(Judged::IsNull(0, true).keeps(&number(1)) && !Judged::IsNull(0, true).keeps(&Value::Null));
    let within = Judged::In(0, list.clone(), false);
    assert!(within.keeps(&number(2)) && !within.keeps(&number(3)) && !within.keeps(&Value::Null));
    let outside = Judged::In(0, list, true);
    assert!(outside.keeps(&number(3)) && !outside.keeps(&number(1)) && !outside.keeps(&Value::Null));
    let nulled = Judged::In(0, vec![number(1), Value::Null], true);
    assert!(!nulled.keeps(&number(3)), "a null in the list: unknown");
    assert!(Judged::Compare(0, CompOp::Ge, number(2)).keeps(&number(2)));
}

fn text(s: &str) -> Value {
    Value::Char(s.into())
}

/// Verifies: [X3.135-1986 §5.14 GR4, GR5] a LIKE of a constant pattern is
/// judged over vectors, after the other conjuncts judged so: its pattern
/// split once, it keeps what a row's LIKE keeps, of ASCII strings and of
/// others, a null never, nor any row if the pattern or the escape is
/// null; a pattern whose escape has no partitioning may raise an error,
/// so the filter is all rest.
#[test]
fn a_like_is_judged_over_vectors() {
    let like = |pattern: Value, escape: Option<Value>| Cond::Like(Expr::Column(3), Expr::Const(pattern), escape.map(Expr::Const));
    let cond = and(like(text("a%c_"), None), compare(Expr::Column(1), CompOp::Eq, int(1)));
    let (judged, rest) = split(&cond, &[1, 3]);
    assert!(rest.is_none());
    assert!(matches!(judged.as_slice(), [Judged::Compare(..), Judged::Like(1, Some(_))]), "the LIKE last: {judged:?}");
    for (value, keeps) in [("abcd", true), ("ac!", true), ("abc", false), ("ä…cd", false), ("aäcé", true), ("a\u{1F600}cz", true)] {
        assert_eq!(judged[1].keeps(&text(value)), keeps, "{value}");
        assert_eq!(crate::value::like(value, "a%c_", None).unwrap(), keeps, "{value}: as a row's LIKE");
    }
    assert!(!judged[1].keeps(&Value::Null), "unknown of a null");
    let (nulled, _) = split(&like(Value::Null, None), &[3]);
    assert!(matches!(nulled.as_slice(), [Judged::Like(0, None)]) && !nulled[0].keeps(&text("a")));
    let (escaped, _) = split(&like(text("a!%%"), Some(text("!"))), &[3]);
    assert!(escaped[0].keeps(&text("a%z")) && !escaped[0].keeps(&text("az")));
    let (none, all) = split(&like(text("a!x"), Some(text("!"))), &[3]);
    assert!(none.is_empty() && all.is_some(), "no partitioning: it may raise");
}

/// Verifies: [DESIGN] RULES.md D31 [X3.135-1986 §5.11 GR2, GR4, §5.13,
/// §5.15]: a conjunct judged over integers keeps the rows its judgment of
/// their values keeps: each comparison, with constants of fewer, as many
/// and more digits after the point, of either sign; IN and NOT IN, of a
/// list with a null; IS NULL and IS NOT NULL; nulls among the integers.
/// One with an approximate constant, or a LIKE, is not judged so; nor one
/// whose constant is too large to scale.
#[test]
fn conjuncts_judged_over_integers_keep_what_values_keep() {
    let exact = |units: i128, scale: u32| Value::Exact(Decimal::new(units, scale));
    let ints = crate::storage::Ints::new(
        vec![-150, 0, 151, 250, 0, 99_999, 150, -1],
        2,
        Some(vec![false, false, false, false, true, false, false, false]),
    );
    let values = ints.values().to_vec();
    let all: Vec<usize> = (0..values.len()).collect();
    let constants = [exact(15, 1), exact(1510, 3), exact(-2, 0), exact(0, 0), exact(1505, 3), exact(99_999, 2)];
    let mut judged = Vec::new();
    for c in &constants {
        for op in [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Le, CompOp::Gt, CompOp::Ge] {
            judged.push(Judged::Compare(0, op, c.clone()));
        }
    }
    let list = vec![exact(15, 1), exact(-15, 1), exact(25, 1)];
    let with_null = vec![exact(15, 1), Value::Null];
    for not in [false, true] {
        judged.push(Judged::In(0, list.clone(), not));
        judged.push(Judged::In(0, with_null.clone(), not));
        judged.push(Judged::IsNull(0, not));
    }
    for j in &judged {
        let by_values: Vec<usize> = all.iter().copied().filter(|&n| j.keeps(&values[n])).collect();
        assert_eq!(ints::kept(j, &ints, &all), Some(by_values), "{j:?}");
    }
    let some: Vec<usize> = vec![1, 3, 4];
    assert_eq!(ints::kept(&Judged::Compare(0, CompOp::Gt, exact(0, 0)), &ints, &some), Some(vec![3]), "of the rows kept before");
    assert_eq!(ints::kept(&Judged::Compare(0, CompOp::Eq, Value::Approx(1.5)), &ints, &all), None, "approximate");
    assert_eq!(ints::kept(&Judged::Compare(0, CompOp::Eq, exact(i128::MAX, 0)), &ints, &all), None, "too large to scale");
    assert_eq!(ints::kept(&Judged::In(0, vec![Value::Approx(1.5)], false), &ints, &all), None, "a list of an approximate");
    assert_eq!(ints::kept(&Judged::Like(0, None), &ints, &all), None, "LIKE");
}

/// Verifies: [DESIGN] RULES.md D31 [X3.135-1986 §5.11 GR5, §5.13, §5.14,
/// §5.15]: a conjunct judged over texts keeps the rows its judgment of
/// their values keeps: LIKE, of a pattern a null makes unknown too; each
/// comparison with a string, padded with spaces; IN and NOT IN, of a list
/// with a null; IS NULL and IS NOT NULL; nulls and strings not ASCII among
/// them; of the same as a dictionary's codes, alike. One with a number, or
/// a list holding one, is not judged so.
#[test]
fn conjuncts_judged_over_texts_keep_what_values_keep() {
    let words = ["ab", "ab ", "", "é", "b", "abc", "a"];
    let mut text = String::new();
    let mut ends = Vec::new();
    for w in words {
        text.push_str(w);
        ends.push(text.len());
    }
    ends.push(text.len());
    let mut nulls = vec![false; words.len()];
    nulls.push(true);
    let texts = crate::storage::Texts::new(text, ends, Some(nulls));
    let values = texts.values().to_vec();
    let all: Vec<usize> = (0..values.len()).collect();
    let char_of = |s: &str| Value::Char(s.to_owned());
    let mut judged = vec![
        Judged::Like(0, constant_pattern(&char_of("a%"), None).unwrap()),
        Judged::Like(0, constant_pattern(&char_of("_b"), None).unwrap()),
        Judged::Like(0, None),
    ];
    for c in ["ab", "a", "é", ""] {
        for op in [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Le, CompOp::Gt, CompOp::Ge] {
            judged.push(Judged::Compare(0, op, char_of(c)));
        }
    }
    for not in [false, true] {
        judged.push(Judged::In(0, vec![char_of("ab"), char_of("b")], not));
        judged.push(Judged::In(0, vec![char_of("é"), Value::Null], not));
        judged.push(Judged::IsNull(0, not));
    }
    for j in &judged {
        let by_values: Vec<usize> = all.iter().copied().filter(|&n| j.keeps(&values[n])).collect();
        assert_eq!(texts::kept(j, &texts, &all), Some(by_values), "{j:?}");
    }
    // The same strings as a dictionary's codes, two entries each used
    // twice, judged once of each entry.
    let (mut entry_text, mut entry_ends) = (String::new(), Vec::new());
    for w in ["ab", "ab ", "", "é", "b", "abc", "a", "zz"] {
        entry_text.push_str(w);
        entry_ends.push(entry_text.len());
    }
    let entry_of_row = vec![0, 1, 2, 3, 4, 5, 6, 0];
    let by_entry = crate::storage::Texts::coded(
        crate::storage::Coded { codes: entry_of_row, entries: crate::storage::Texts::new(entry_text, entry_ends, None) },
        texts.nulls.clone(),
    );
    for j in &judged {
        let by_values: Vec<usize> = all.iter().copied().filter(|&n| j.keeps(&values[n])).collect();
        assert_eq!(texts::kept(j, &by_entry, &all), Some(by_values), "{j:?}, coded");
    }
    assert_eq!(texts::kept(&Judged::Compare(0, CompOp::Eq, number(1)), &texts, &all), None, "a number");
    assert_eq!(texts::kept(&Judged::In(0, vec![number(1)], false), &texts, &all), None, "a list of a number");
}

/// Verifies: [DESIGN] RULES.md D31 [X3.135-1986 §5.11 GR2, GR4, §5.13,
/// §5.15]: a conjunct judged over approximate numbers (`judge/floats.rs`)
/// keeps the rows it keeps of their values: comparisons of every operator
/// with approximate and exact constants, -0 equal to 0, one with a string
/// true of none; IS NULL and IS NOT NULL; IN and NOT IN, a null among
/// them; LIKE not judged of them.
#[test]
fn conjuncts_judged_over_floats_keep_as_values() {
    let numbers = vec![1.5_f64, 0.0_f64, -2.25_f64, -0.0_f64, 3.0_f64, 7.0_f64];
    let floats = Floats::new(numbers, Some(vec![false, true, false, false, false, false]));
    let of_floats = Batch { columns: vec![Column::Floats(floats.clone())], rows: 6 };
    let of_values = Batch { columns: vec![Column::from(floats.values().to_vec())], rows: 6 };
    let mut judged = vec![
        Judged::IsNull(0, false),
        Judged::IsNull(0, true),
        Judged::In(0, vec![Value::Approx(1.5_f64), number(3), Value::Null], false),
        Judged::In(0, vec![Value::Approx(1.5_f64), number(3)], true),
        Judged::In(0, vec![Value::Approx(1.5_f64), Value::Null], true),
        Judged::Compare(0, CompOp::Eq, Value::Char("x".into())),
    ];
    for op in [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Le, CompOp::Gt, CompOp::Ge] {
        judged.extend([
            Judged::Compare(0, op, Value::Approx(0.0_f64)),
            Judged::Compare(0, op, number(3)),
            Judged::Compare(0, op, Value::Approx(1.5_f64)),
        ]);
    }
    let over: Vec<usize> = (0..6).collect();
    for j in &judged {
        assert_eq!(j.kept(&of_floats, &over), j.kept(&of_values, &over), "{j:?}");
    }
    assert_eq!(floats::kept(&Judged::Like(0, None), &floats, &over), None, "not of numbers");
}
