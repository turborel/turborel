//! The modes of a prepared CALL's dynamic parameters [ISO/IEC 9075-2:2023
//! §20.10 GR7e] (RULES.md D977): each `?` described with the mode and the
//! ordinal position of the parameter whose argument holds it, and the
//! specific name of the procedure the CALL invokes, through the API and
//! DESCRIBE alike; of an overloaded procedure, the one of as many
//! parameters as arguments; DEFAULT of an argument, of no `?`.

use turborel::catalog::ParameterMode;
use turborel::db::{Database, Prepared, Session};
use turborel::error::Code;
use turborel::value::{Decimal, Value};

use crate::common::{code_with as code, dynamic_host, run_with as run, set_text as text};

/// Schema A: T (K, N) of rows (1, 10) and (2, 20); MIXED (OUT, IN, INOUT),
/// specific MIXED,
/// and FIND of two parameters (IN, OUT) and of three (IN, OUT, OUT),
/// specific FIND_TWO and FIND_THREE; and a session of A.
fn defined() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    a.run(
        "CREATE PROCEDURE MIXED (OUT TOTAL INTEGER, IN TIMES INTEGER, INOUT TAG VARCHAR(10)) SPECIFIC MIXED READS SQL DATA \
         SELECT SUM(N) * TIMES, TAG || 'X' INTO TOTAL, TAG FROM T",
    )
    .unwrap();
    a.run("CREATE PROCEDURE FIND (IN W INTEGER, OUT G INTEGER) SPECIFIC FIND_TWO READS SQL DATA SELECT N INTO G FROM T WHERE K = W")
        .unwrap();
    a.run(
        "CREATE PROCEDURE FIND (IN W INTEGER, OUT G INTEGER, OUT H INTEGER) SPECIFIC FIND_THREE READS SQL DATA \
         SELECT N, N * 2 INTO G, H FROM T WHERE K = W",
    )
    .unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The exact number `n`.
fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// Each of `p`'s `?`s: its parameter's mode and ordinal position.
fn modes(p: &Prepared) -> Vec<(ParameterMode, Option<u32>)> {
    p.parameters().iter().map(|d| (d.mode, d.ordinal)).collect()
}

/// Verifies: [ISO/IEC 9075-2:2023 §20.10 GR7e, §20.7 GR7a] RULES.md D977:
/// each `?` of a prepared CALL is described with its parameter's mode and
/// ordinal position, several of an IN parameter's expression each, those
/// of named arguments in the order written; an execution gives values
/// for the IN and INOUT ones and gives back the OUT and INOUT ones'; the
/// procedure is told by its specific name; a `?` of another statement is
/// IN, of no parameter.
#[test]
fn each_dynamic_parameter_of_a_call_has_its_mode_20_10() {
    let (_db, a) = defined();
    let mixed = a.prepare("CALL MIXED(?, CAST(? AS INTEGER) + ?, ?)").unwrap();
    let (in_, out, both) = (ParameterMode::In, ParameterMode::Out, ParameterMode::InOut);
    assert_eq!(modes(&mixed), [(out, Some(1)), (in_, Some(2)), (in_, Some(2)), (both, Some(3))]);
    assert_eq!(mixed.inputs().len(), 3, "the OUT one's is no input");
    assert_eq!(mixed.procedure().map(|n| (n.auth.as_str(), n.ident.as_str())), Some(("A", "MIXED")));
    let out_values = a.execute_prepared_out(&mixed, &[int(1), int(1), Value::Char("A".into())]).unwrap().1;
    assert_eq!(out_values.iter().map(ToString::to_string).collect::<Vec<_>>(), ["60", "AX"]);
    let named = a.prepare("CALL MIXED(TAG => ?, TIMES => ?, TOTAL => ?)").unwrap();
    assert_eq!(modes(&named), [(both, Some(3)), (in_, Some(2)), (out, Some(1))], "in the order written");
    let fixed = a.prepare("CALL MIXED(?, 2, ?)").unwrap();
    assert_eq!((modes(&fixed), fixed.inputs().len()), (vec![(out, Some(1)), (both, Some(3))], 1), "of a constant none");
    let query = a.prepare("SELECT N FROM T WHERE K = ?").unwrap();
    assert_eq!((modes(&query), query.procedure()), (vec![(in_, None)], None));
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.18 SR9b, §11.60 SR20a, §20.10 GR7e
/// vi] RULES.md D977: of procedures of one name, a CALL invokes the one of
/// as many parameters as it has arguments, which describes its `?`s and
/// is told by its specific name; DEFAULT of an IN or INOUT parameter's
/// argument holds no `?` (Features T522 and T523), and a CALL of fewer
/// arguments than parameters is refused (-103), as no parameter has a
/// default.
#[test]
fn an_overloaded_procedure_and_a_default_argument_20_10() {
    let (_db, a) = defined();
    let two = a.prepare("CALL FIND(?, ?)").unwrap();
    let three = a.prepare("CALL FIND(?, ?, ?)").unwrap();
    assert_eq!(two.procedure().map(|n| n.ident.as_str()), Some("FIND_TWO"));
    assert_eq!(three.procedure().map(|n| n.ident.as_str()), Some("FIND_THREE"));
    let out = ParameterMode::Out;
    assert_eq!(modes(&three), [(ParameterMode::In, Some(1)), (out, Some(2)), (out, Some(3))]);
    let values = a.execute_prepared_out(&three, &[int(2)]).unwrap().1;
    assert_eq!(values.iter().map(ToString::to_string).collect::<Vec<_>>(), ["20", "40"]);
    let defaulted = a.prepare("CALL FIND(DEFAULT, ?)").unwrap();
    assert_eq!((modes(&defaulted), defaulted.procedure().map(|n| n.ident.as_str())), (vec![(out, Some(2))], Some("FIND_TWO")), "T522");
    let both = a.prepare("CALL MIXED(?, 1, DEFAULT)").unwrap();
    assert_eq!(modes(&both), [(out, Some(1))], "T523");
    let refused = |sql: &str| a.prepare(sql).map(drop).unwrap_err().sqlcode();
    assert_eq!(refused("CALL FIND(?)"), -103_i32, "no parameter has a default");
}

/// Verifies: [ISO/IEC 9075-2:2023 §20.10 GR7e iv to vi, Table 33, §20.5
/// SR3a] RULES.md D977: DESCRIBE INPUT and DESCRIBE OUTPUT of a CALL set
/// each item's PARAMETER_MODE to its parameter's mode's code (1 IN, 2
/// INOUT, 4 OUT), PARAMETER_ORDINAL_POSITION to its position and the
/// PARAMETER_SPECIFIC items to the procedure's specific name; of another
/// statement they are 0 and empty; SET DESCRIPTOR sets none of them
/// (SQLCODE -103).
#[test]
fn describe_sets_each_parameters_mode_20_10() {
    let (_db, a) = defined();
    let mut p = dynamic_host();
    let item = |p: &mut turborel::params::Params, n: usize| {
        let get =
            format!("GET DESCRIPTOR 'D' VALUE {n} N = PARAMETER_MODE, VAL = PARAMETER_ORDINAL_POSITION, SN = PARAMETER_SPECIFIC_NAME");
        run(&a, &get, p).unwrap();
        run(&a, &format!("GET DESCRIPTOR 'D' VALUE {n} CN = PARAMETER_SPECIFIC_SCHEMA, DN = PARAMETER_SPECIFIC_CATALOG"), p).unwrap();
        ["N", "VAL", "SN", "CN"].map(|v| p.value(v).to_string())
    };
    text(&mut p, "CALL MIXED(?, ?, ?)");
    run(&a, "PREPARE C FROM TEXT", &mut p).unwrap();
    run(&a, "ALLOCATE DESCRIPTOR 'D'", &mut p).unwrap();
    run(&a, "DESCRIBE INPUT C USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    assert_eq!(item(&mut p, 1), ["1", "2", "MIXED", "A"]);
    assert_eq!(item(&mut p, 2), ["2", "3", "MIXED", "A"]);
    assert!(!p.value("DN").to_string().is_empty(), "the catalog's name");
    run(&a, "DESCRIBE OUTPUT C USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    assert_eq!(item(&mut p, 1), ["4", "1", "MIXED", "A"]);
    assert_eq!(item(&mut p, 2), ["2", "3", "MIXED", "A"]);
    text(&mut p, "SELECT N FROM T WHERE K = ?");
    run(&a, "PREPARE Q FROM TEXT", &mut p).unwrap();
    run(&a, "DESCRIBE INPUT Q USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    assert_eq!(item(&mut p, 1), ["0", "0", "", ""]);
    for set in ["PARAMETER_MODE = 1", "PARAMETER_ORDINAL_POSITION = 1", "PARAMETER_SPECIFIC_NAME = 'P'"] {
        assert_eq!(code(&a, &format!("SET DESCRIPTOR 'D' VALUE 1 {set}"), &mut p), Code::SyntaxRule, "{set}");
    }
}
