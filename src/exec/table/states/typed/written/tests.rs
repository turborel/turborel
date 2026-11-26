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
        Typed::Exact(e) => format!("exact {:?} {:?} {:?} {:?}", e.kind, e.units, e.counts, e.scale),
        Typed::Approx(a) => {
            let sums: Vec<(u64, u64)> = a.sums.iter().map(|w| w.parts()).map(|(b, s)| (b.to_bits(), s.to_bits())).collect();
            format!("approx {} {sums:?} {:?}", a.mean, a.counts)
        }
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
/// as written, bit for bit, the groups at the places in their order.
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
                units: vec![i128::MAX, -5, 0, i128::MIN],
                counts: vec![1, 2, 0, 9],
                scale: Some(2),
                result: call("SUM", Some(numeric)).result,
            }),
        ),
        (
            call("MAX", Some(integer.clone())),
            Typed::Exact(Exact { kind: ColumnarKind::Max, units: vec![0; 4], counts: vec![0; 4], scale: None, result: integer }),
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
