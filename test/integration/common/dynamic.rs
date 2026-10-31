//! Helpers of the tests of dynamic SQL in SQL text [ISO/IEC 9075:1992
//! §17]: a program's parameters, and statements run with them. Split from
//! common.rs (RULES.md R-SIZE-2).

use turborel::db::{Completion, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::{DataType, Spelled, Value};

use super::number;

/// The parameters of a program that runs dynamic SQL in SQL text [ISO/IEC
/// 9075:1992 §17] on `setup`'s tables: TEXT, a statement's text,
/// VARCHAR(500); SN, CN and DN, names of a statement, a cursor and a
/// descriptor area, VARCHAR(128); K, ID, N and VAL, INTEGER, K 1; NM,
/// CHAR(6), with its indicator NI; and TXT, CHARACTER(128), of a field's
/// text.
pub fn dynamic_host() -> Params {
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    let name = DataType::Varying { length: 128 };
    Params::new()
        .declare("TEXT", DataType::Varying { length: 500 }, Value::Null)
        .declare("SN", name, Value::Null)
        .declare("CN", name, Value::Null)
        .declare("DN", name, Value::Null)
        .declare("K", integer, number(1))
        .declare("ID", integer, Value::Null)
        .declare("N", integer, Value::Null)
        .declare("VAL", integer, Value::Null)
        .declare("NM", DataType::Character { length: 6 }, Value::Null)
        .indicator("NI", 0)
        .declare("TXT", DataType::Character { length: 128 }, Value::Null)
}

/// `sql` run with `p`: its completion, or its error's SQLSTATE.
pub fn run_with(s: &Session, sql: &str, p: &mut Params) -> Result<Completion, &'static str> {
    s.execute(sql, p).map_err(|e| e.sqlstate().as_str())
}

/// The SQLCODE `sql`, run with `p`, fails with.
pub fn code_with(s: &Session, sql: &str, p: &mut Params) -> Code {
    s.execute(sql, p).map_err(|e| e.code).expect_err(sql)
}

/// `p`, of `dynamic_host`, its TEXT set to `text`.
pub fn set_text(p: &mut Params, text: &str) {
    p.set("TEXT", Value::Char(text.to_owned()));
}
