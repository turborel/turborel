//! Unit tests of columns of states written to a spill: each kind read
//! back as written, bit for bit (RULES.md D1226).

use super::*;
use crate::functions::{ColumnarKind, Registry};
use crate::logical::Expr;
use crate::value::{DataType, Spelled};

/// A call of `name` of a column of type `arg`, or of rows if none.
fn call(name: &str, arg: Option<DataType>) -> AggCall {
    let func = Registry::with_set_functions().aggregate(name).unwrap();
    let result = func.0.result_type(arg.clone()).unwrap();
    AggCall { func, arg: arg.map(|_| Expr::Column(0)), distinct: false, result, filter: None, distribution: None, label: name.to_owned() }
}

/// Every bit of `typed`, as a string.
fn bits(typed: &Typed) -> String {
    match typed {
        Typed::Count(counts) => format!("count {counts:?}"),
        Typed::Exact(e) => format!("exact {:?} {:?} {:?} {:?}", e.kind, e.totals, e.counts, e.scale),
        Typed::Approx(a) => {
            let sums: Vec<(u64, u64)> = a.sums.iter().map(|w| w.parts()).map(|(b, s)| (b.to_bits(), s.to_bits())).collect();
            format!("approx {} {sums:?} {:?}", a.mean, a.counts)
        }
        Typed::Best(best) => format!("best {:?}", (0..best.len()).map(|g| best.finish(g)).collect::<Vec<_>>()),
    }
}

/// `typed` of `agg`, its groups at `places` written and read back.
fn round_trip(typed: &Typed, agg: &AggCall, places: &[u32]) -> Typed {
    let mut out = Vec::new();
    typed.put_groups(places, &mut out);
    let mut reader = Reader::new(&out, "states");
    let back = Typed::read(&mut reader, places.len(), agg).unwrap();
    reader.finish().unwrap();
    back
}

/// Verifies: [DESIGN] (RULES.md D1226) counts, exact sums of 128 bits
/// with their scale and of no scale yet, and approximate sums and means
/// of both parts, -0 and values below 2^-900 among them, are read back
/// as written, bit for bit, the groups at the places in their order; the
/// groups of a wide column that 64 bits hold are read back narrow, as the
/// narrow column of the same totals is (RULES.md D1308).
#[test]
fn every_column_of_states_is_read_back_as_written() {
    let double = DataType::Approximate { precision: crate::value::DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    let numeric = DataType::Exact { precision: 30, scale: 2, spelled: Spelled::Numeric };
    let wide = |b: f64, s: f64| Wide::of_parts((b, s));
    let columns = [
        (call("COUNT", None), Typed::Count(vec![0, 3, i64::MAX, 7])),
        (
            call("SUM", Some(numeric.clone())),
            Typed::Exact(Exact {
                kind: ColumnarKind::Sum,
                totals: Totals::Wide(vec![i128::MAX, -5, 0, i128::MIN]),
                counts: vec![1, 2, 0, 9],
                scale: Some(2),
                result: call("SUM", Some(numeric)).result,
            }),
        ),
        (
            call("MAX", Some(integer.clone())),
            Typed::Exact(Exact {
                kind: ColumnarKind::Max,
                totals: Totals::Narrow(vec![0; 4]),
                counts: vec![0; 4],
                scale: None,
                result: integer,
            }),
        ),
        (
            call("SUM", Some(double.clone())),
            Typed::Approx(Approx {
                mean: false,
                sums: vec![wide(-0.0_f64, -0.0_f64), wide(1.5_f64, 1e-300_f64), wide(f64::MAX, -0.0_f64), wide(-2.0_f64, 0.0_f64)],
                counts: vec![0, 4, 1, 2],
                result: call("SUM", Some(double.clone())).result,
            }),
        ),
        (
            call("AVG", Some(double.clone())),
            Typed::Approx(Approx {
                mean: true,
                sums: vec![wide(3.0_f64, -1e-310_f64); 4],
                counts: vec![3; 4],
                result: call("AVG", Some(double)).result,
            }),
        ),
    ];
    for (agg, typed) in &columns {
        assert_eq!(bits(&round_trip(typed, agg, &[0, 1, 2, 3])), bits(typed), "{}", agg.label);
        let twice = round_trip(&round_trip(typed, agg, &[3, 1]), agg, &[1]);
        assert_eq!(bits(&twice), bits(&round_trip(typed, agg, &[1])), "{}: the groups at the places, in their order", agg.label);
    }
}

/// Verifies: [DESIGN] (RULES.md D1186, D1226) the column of a part of a
/// set function, which is its function's column, is read back from a
/// spill by the function the part is of: a part of COUNT and of a sum of
/// integers, bit for bit, where reading it as the part's own function,
/// which keeps no column, would refuse it.
#[test]
fn a_parts_column_is_read_back_as_its_functions() {
    use std::sync::Arc;
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    let part = |of: AggCall| {
        let func = crate::functions::AggregateRef(Arc::new(crate::functions::Partial { func: of.func.clone(), result: of.result.clone() }));
        AggCall { func, result: crate::functions::PART_TYPE, ..of }
    };
    let sum = call("SUM", Some(integer));
    let totals = Totals::Narrow(vec![7, -5, 0]);
    let summed = Exact { kind: ColumnarKind::Sum, totals, counts: vec![1, 2, 0], scale: Some(0), result: sum.result.clone() };
    for (agg, typed) in [(part(call("COUNT", None)), Typed::Count(vec![0, 3, 7])), (part(sum), Typed::Exact(summed))] {
        assert_eq!(bits(&round_trip(&typed, &agg, &[2, 0, 1])), bits(&typed.gathered(&[2, 0, 1])), "{}", agg.label);
    }
}

/// Verifies: [DESIGN] (RULES.md D1308, D1226) a column of exact totals is
/// written to a spill as the same bytes whichever form holds its totals,
/// 16 bytes a total; read back it holds the totals written, narrow where
/// 64 bits hold every one of the groups written, the least and greatest
/// integers of 64 bits among them, and wide where one of them does not
/// fit, on either side; and bytes that end within the totals are refused
/// (SQLCODE -916).
#[test]
fn exact_totals_are_written_alike_in_either_form_and_read_back_narrow_where_they_fit() {
    let whole = DataType::Exact { precision: 19, scale: 0, spelled: Spelled::Numeric };
    let sum = call("SUM", Some(whole));
    let column = |totals: Totals| {
        Typed::Exact(Exact { kind: ColumnarKind::Sum, totals, counts: vec![1, 2, 3, 4], scale: Some(0), result: sum.result.clone() })
    };
    let fit = [i64::MAX, -5, 0, i64::MIN];
    let (narrow, wide) = (column(Totals::Narrow(fit.to_vec())), column(Totals::Wide(fit.iter().map(|&t| i128::from(t)).collect())));
    let written = |typed: &Typed, places: &[u32]| {
        let mut out = Vec::new();
        typed.put_groups(places, &mut out);
        out
    };
    assert_eq!(written(&narrow, &[0, 1, 2, 3]), written(&wide, &[0, 1, 2, 3]), "the same bytes of either form");
    assert_eq!(written(&narrow, &[0, 1, 2, 3]).len(), 1 + 1 + 4 + 4 * 16 + 4 * 8, "16 bytes a total");
    for typed in [&narrow, &wide] {
        assert_eq!(bits(&round_trip(typed, &sum, &[3, 0, 1, 2])), bits(&narrow.gathered(&[3, 0, 1, 2])), "read back narrow");
    }
    for past in [i128::from(i64::MAX) + 1, i128::from(i64::MIN) - 1] {
        let one_past = column(Totals::Wide(vec![7, past, -9, 0]));
        assert_eq!(bits(&round_trip(&one_past, &sum, &[0, 1, 2, 3])), bits(&one_past), "a total past 64 bits read back wide");
        let without = bits(&round_trip(&one_past, &sum, &[2, 0]));
        assert!(without.contains("Narrow([-9, 7])"), "narrow, of the groups that fit alone: {without}");
    }
    let bytes = written(&narrow, &[0, 1, 2, 3]);
    let mut short = Reader::new(&bytes[..bytes.len() - 4 * 8 - 1], "states");
    assert_eq!(Typed::read(&mut short, 4, &sum).err().map(|e| e.sqlcode()), Some(-916_i32));
}
