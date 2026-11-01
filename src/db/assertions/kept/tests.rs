//! Unit tests of `db/assertions/kept.rs`: totals added and taken, the way
//! a change moves a form's value, what a read of the totals records of it,
//! and a row of totals read back as written (RULES.md R-TEST-1, R-TEST-2).

use super::*;
use crate::functions::Registry;
use crate::logical::AggCall;

const EXACT: DataType = DataType::Exact { precision: 18, scale: 2, spelled: Spelled::Numeric };

fn totals(counted: i128, nulled: i128, sum: Option<(i128, u32)>) -> Totals {
    Totals { counted, nulled, sum: sum.map(|(units, scale)| Decimal::new(units, scale)) }
}

/// `SUM(x) op k`, or AVG, of an exact `k`.
fn totalled(avg: bool, op: CompOp, k: Decimal) -> Totalled {
    let functions = Registry::with_set_functions();
    let func = functions.aggregate(if avg { "AVG" } else { "SUM" }).unwrap();
    let agg = AggCall { func, arg: Some(Expr::Column(0)), distinct: false, result: EXACT, label: "F".into() };
    Totalled { agg, x: Expr::Column(0), avg, op, k: Expr::Const(Value::Exact(k)) }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR2b iv]; [DESIGN]
/// RULES.md D51: totals add and take back at any scale, exactly; one
/// changing nothing is zero; past 128 bits, SQLCODE -900, as SUM's own.
#[test]
fn totals_add_and_take_back() {
    let a = totals(2, 1, Some((150, 2)));
    let b = totals(1, 0, Some((25, 1)));
    let both = a.plus(b).unwrap();
    assert_eq!((both.counted, both.nulled, both.sum.map(|d| (d.units, d.scale))), (3, 1, Some((400, 2))));
    let back = both.minus(b).unwrap();
    assert_eq!((back.counted, back.nulled, back.sum.map(|d| d.to_f64())), (2, 1, Some(1.5_f64)));
    assert!(back.minus(a).unwrap().is_zero(), "taken back whole");
    assert!(Totals::default().is_zero() && !totals(0, 1, None).is_zero() && !totals(0, 0, Some((1, 0))).is_zero());
    let huge = totals(i128::MAX, 0, None);
    assert_eq!(huge.plus(totals(1, 0, None)).err().map(|e| e.code), Some(Code::Unsupported));
    let wide = totals(0, 0, Some((i128::MAX, 0)));
    assert_eq!(wide.plus(wide).err().map(|e| e.code), Some(Code::Unsupported));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §6.5 GR2b ii, iv]; [DESIGN]
/// RULES.md D51: a change moves SUM's value as its sum's sign, and AVG's
/// as the sign of its sum less the bound times its count, at the larger
/// scale, or past 128 bits as approximate numbers, whose sign is then sure.
#[test]
fn a_change_moves_a_value_by_its_sign() {
    let sum = totalled(false, CompOp::Le, Decimal::new(10, 0));
    assert_eq!(moves(&sum, &totals(1, 0, Some((5, 0)))), Move::Up);
    assert_eq!(moves(&sum, &totals(1, 0, Some((-5, 0)))), Move::Down);
    assert_eq!(moves(&sum, &totals(1, 0, None)), Move::Still);
    let avg = totalled(true, CompOp::Le, Decimal::new(25, 1));
    // Two values of 3: 6 - 2.5 * 2 = 1, up; one of 2: 2 - 2.5, down.
    assert_eq!(moves(&avg, &totals(2, 0, Some((6, 0)))), Move::Up);
    assert_eq!(moves(&avg, &totals(1, 0, Some((2, 0)))), Move::Down);
    assert_eq!(moves(&avg, &totals(2, 0, Some((5, 0)))), Move::Still);
    // A value of 3 taken: -3 + 2.5, down.
    assert_eq!(moves(&avg, &totals(-1, 0, Some((-3, 0)))), Move::Down);
    let far = totalled(true, CompOp::Le, Decimal::new(i128::MAX.checked_div(10).unwrap(), 0));
    assert_eq!(moves(&far, &totals(1_000, 0, Some((0, 0)))), Move::Down, "past 128 bits");
}

/// Verifies: [DESIGN] RULES.md D29, D51: a change recorded as read is the
/// one that may break the form with another moving it so: SUM's either
/// way, its sum in range [§6.5 GR2b iv]; AVG's only toward the bound
/// [GR2b ii]; of equal or unequal, every row of the form; of none moved,
/// nothing.
#[test]
fn a_move_records_what_may_break_it_with_it() {
    let k = Decimal::new(10, 0);
    let cases = [
        (false, CompOp::Le, Move::Up, Register::Moving(Move::Up)),
        (false, CompOp::Le, Move::Down, Register::Moving(Move::Down)),
        (false, CompOp::Gt, Move::Up, Register::Moving(Move::Up)),
        (true, CompOp::Le, Move::Up, Register::Moving(Move::Up)),
        (true, CompOp::Lt, Move::Down, Register::Nothing),
        (true, CompOp::Ge, Move::Down, Register::Moving(Move::Down)),
        (true, CompOp::Gt, Move::Up, Register::Nothing),
        (false, CompOp::Eq, Move::Down, Register::Form),
        (true, CompOp::Ne, Move::Up, Register::Form),
        (false, CompOp::Eq, Move::Still, Register::Nothing),
    ];
    for (n, (avg, op, mv, registered)) in cases.into_iter().enumerate() {
        let t = totalled(avg, op, k);
        assert_eq!(Register::of(t.op, !t.avg, mv), registered, "case {n}");
    }
    assert!(Register::Nothing.condition(3).is_none() && Register::Moving(Move::Still).condition(3).is_none());
    let row = |form: i128, mv: i128| vec![number(form), number(0), number(0), Value::Char("0".into()), number(0), number(mv)];
    let up = Register::Moving(Move::Up).condition(3).unwrap();
    assert!(up.conjuncts().iter().all(|c| c.matches(&row(3, 1))), "of the form, moving up");
    assert!(!up.conjuncts().iter().all(|c| c.matches(&row(3, -1))) && !up.conjuncts().iter().all(|c| c.matches(&row(2, 1))));
    let down = Register::Moving(Move::Down).condition(3).unwrap();
    assert!(down.conjuncts().iter().all(|c| c.matches(&row(3, -1))) && !down.conjuncts().iter().all(|c| c.matches(&row(3, 0))));
    let form = Register::Form.condition(3).unwrap();
    assert!(form.conjuncts().iter().all(|c| c.matches(&row(3, 0))) && !form.conjuncts().iter().all(|c| c.matches(&row(4, 0))));
}

/// Verifies: [DESIGN] RULES.md D51: a row of totals is read back as it is
/// written, its sum's 128 bits exactly; one of another shape is refused
/// with SQLCODE -916; each assertion's table has a name of its own.
#[test]
fn a_row_of_totals_is_read_back() {
    let written = totals(-4, 2, Some((i128::MIN + 1, 7)));
    let row = kept_row(5, &written, Move::Down);
    assert_eq!(row.len(), WIDTH);
    let (form, read) = parse_row(&row).unwrap();
    assert_eq!((form, read.counted, read.nulled, read.sum.map(|d| (d.units, d.scale))), (5, -4, 2, Some((i128::MIN + 1, 7))));
    let mut bad = row.clone();
    bad[UNITS] = Value::Char("ten".into());
    assert_eq!(parse_row(&bad).err().map(|e| e.code), Some(Code::Corrupt));
    bad = row;
    bad[COUNTED] = Value::Exact(Decimal::new(15, 1));
    assert_eq!(parse_row(&bad).err().map(|e| e.code), Some(Code::Corrupt));
    let named = |auth: &str, ident: &str| kept_name(&QualifiedName { auth: auth.into(), ident: ident.into() });
    assert_ne!(named("A_B", "C"), named("A", "B_C"));
    assert_ne!(named("AB", "C"), named("A", "BC"));
    assert_eq!(named("A", "X").auth, DEFINITION_SCHEMA);
    let desc = kept_desc(named("A", "X"), 3);
    assert_eq!((desc.columns.len(), desc.generation, desc.engine.as_str()), (WIDTH, 3_u32, ROW_STORE));
}
