//! Unit tests of a frame of reference with a common divisor
//! (`encoded/stepped.rs`, RULES.md D1098).

use super::*;

/// The microseconds of a day, the step of a DATE as a record keeps it.
const DAY: i128 = 86_400_000_000;
/// The instant of the first day.
const FIRST: i128 = 725_846_400_000_000;
/// A column whose units reach every integer of 64 bits.
const BIG: DataType = DataType::Exact { precision: 18, scale: 0, spelled: Spelled::Bigint };

/// The values at positions `from` to `to` of the chunk of all of
/// `values`, compressed, as integers read at once, and each read alone.
fn integers(values: &[Value], (from, to): (usize, usize)) -> (Vec<i64>, Vec<i64>) {
    let fields = Fields::of(&BIG, values.iter().cloned()).unwrap();
    let bytes = fields.chunk(&BIG, 0, (fields.len(), true)).unwrap();
    let chunk = Chunk::parse(&bytes).unwrap();
    let (mut units, mut nulls) = (Vec::new(), Mask::default());
    chunk.ints((from, to), (&mut units, &mut nulls)).unwrap();
    (units, (from..to).map(|n| chunk.body.int_at(n, None).unwrap()).collect())
}

fn shown(values: &[Value]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

fn units(values: &[i128]) -> Vec<i64> {
    values.iter().map(|&n| i64::try_from(n).unwrap()).collect()
}

/// Verifies: [DESIGN] RULES.md D1098, steps: values a fixed step apart,
/// as the instants of dates are a day apart, are written as their counts
/// of steps from the least, in the bits those need, a null among them:
/// 3,000 days in 12 bits each where a frame of reference takes 48; read
/// back as written, whole, a run of them at once, and each alone.
#[test]
fn values_a_step_apart_take_the_bits_of_their_steps() {
    let days: Vec<i128> = (0..2000_i128).map(|n| FIRST.wrapping_add(n.wrapping_mul(7919).rem_euclid(3000).wrapping_mul(DAY))).collect();
    let dates: Vec<Value> = days.iter().enumerate().map(|(n, &day)| if n == 5 { Value::Null } else { int(day) }).collect();
    let (kind, len, back) = compressed(&BIG, &dates);
    assert_eq!(kind, STEPPED);
    assert!(len < 3300, "{len} bytes: 12 bits each of 2,000");
    assert_eq!(back, shown(&dates));
    let (at_once, alone) = integers(&dates, (100, 1900));
    let want = units(&days[100..1900]);
    assert_eq!(at_once, want);
    assert_eq!(alone, want);
}

/// Verifies: [DESIGN] RULES.md D1098, steps not taken: values whose
/// differences share no divisor above 1 are never written as steps, one
/// odd difference among even ones enough; nor are values that share one
/// where the step's 8 bytes outweigh the bits it saves, as of a few
/// values. MC/DC of the choice: a step above 1 that saves bytes (taken,
/// the test above) against a step of 1, and against one that saves none.
#[test]
fn values_of_no_common_step_are_a_frame() {
    let apart: Vec<Value> = (0..500_i128).map(|n| int(n.wrapping_mul(6))).chain([int(3001)]).collect();
    let (kind, _, back) = compressed(&BIG, &apart);
    assert_eq!((kind, back), (FRAME, shown(&apart)), "one odd difference");
    let primes: Vec<Value> = [2, 3, 5, 7, 11, 13, 17, 19, 23, 29].iter().cycle().take(400).map(|&n| int(n)).collect();
    assert_eq!(compressed(&BIG, &primes).0, FRAME, "no common divisor");
    let few: Vec<Value> = [0, 2, 4].map(int).to_vec();
    let (few_kind, _, few_back) = compressed(&BIG, &few);
    assert_eq!((few_kind, few_back), (FRAME, shown(&few)), "a step that saves nothing");
}

/// Verifies: [DESIGN] RULES.md D1098, steps of any integers: negative
/// values a step apart, and the least and the greatest integers of 64
/// bits in one chunk, whose difference no signed integer holds, are
/// written as steps and read back as written, whole, at once and alone;
/// no difference overflows.
#[test]
fn steps_hold_negative_values_and_the_extremes() {
    let below: Vec<i128> = (0..300_i128).map(|n| n.wrapping_mul(37).rem_euclid(300).wrapping_mul(7).wrapping_sub(1_000_000)).collect();
    let negative: Vec<Value> = below.iter().map(|&n| int(n)).collect();
    let (kind, _, back) = compressed(&BIG, &negative);
    assert_eq!((kind, back), (STEPPED, shown(&negative)));
    assert_eq!(integers(&negative, (0, 300)).0, units(&below));
    // A third of the span of 64 bits: the least, a step past it, and the
    // greatest, three steps past it.
    let step = i128::from(u64::MAX.wrapping_div(3));
    let ends = [i128::from(i64::MIN), i128::from(i64::MIN).wrapping_add(step), i128::from(i64::MAX)];
    let all: Vec<i128> = ends.iter().cycle().take(90).copied().collect();
    let extremes: Vec<Value> = all.iter().map(|&n| int(n)).collect();
    let (ends_kind, len, ends_back) = compressed(&BIG, &extremes);
    assert_eq!((ends_kind, ends_back), (STEPPED, shown(&extremes)));
    assert!(len < 70, "{len} bytes: 2 bits each");
    let (at_once, alone) = integers(&extremes, (0, 90));
    assert_eq!(at_once, units(&all));
    assert_eq!(alone, units(&all));
}

/// Verifies: [DESIGN] RULES.md D1098, R-ERR-2: a chunk of steps whose
/// step is 0, which none written has, is refused (SQLCODE -916).
#[test]
fn a_step_of_none_is_refused() {
    let values: Vec<Value> = (0..200_i128).map(|n| int(n.wrapping_mul(10))).collect();
    let fields = Fields::of(&BIG, values.into_iter()).unwrap();
    let mut bytes = fields.chunk(&BIG, 0, (fields.len(), true)).unwrap();
    assert_eq!(bytes.first(), Some(&STEPPED));
    let step = HEADER.wrapping_add(nulls_len(200)).wrapping_add(9);
    bytes[step..step.wrapping_add(8)].fill(0);
    assert_eq!(Chunk::parse(&bytes).err().map(|e| e.code), Some(Code::Corrupt));
}
