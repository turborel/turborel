//! Unit tests of a table function's invocation (`builtin.rs`): the type
//! it reports, the rows it reads and makes, and its value as a function.

use super::*;
use crate::value::Decimal;

/// A table function of columns A and B, making the rows it is given as
/// its first argument's elements, each of `width` values, then a row of
/// each row of the query it reads.
#[derive(Debug)]
struct Echo {
    width: usize,
}

fn int() -> DataType {
    DataType::Exact { precision: 9, scale: 0, spelled: crate::value::Spelled::Integer }
}

fn number(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

impl TableFunction for Echo {
    fn name(&self) -> &'static str {
        "ECHO"
    }
    fn parameters(&self, _: usize) -> Option<Vec<DataType>> {
        Some(vec![int()])
    }
    fn columns(&self) -> Vec<(String, DataType)> {
        vec![("A".to_owned(), int()), ("B".to_owned(), int())]
    }
    fn rows(&self, args: &[Value], read: &[Vec<Vec<Value>>]) -> Result<Vec<Vec<Value>>> {
        let mut made: Vec<Vec<Value>> = args.iter().map(|v| vec![v.clone(); self.width]).collect();
        made.extend(read.iter().flatten().map(|r| r.iter().cycle().take(self.width).cloned().collect()));
        Ok(made)
    }
}

/// Verifies: [DESIGN] RULES.md D125, a table function invoked: its value
/// is the multiset of its rows, each a row of its columns, typed so; the
/// rows of a query it reads are given it each a row, a value of one
/// column its own row, a null query none; as a function it is the first
/// column of its first row, the null value of no row; it is not
/// deterministic, and is not a function by default.
#[test]
fn a_table_function_gives_its_rows_as_a_multiset() {
    let call = TableCall { function: Arc::new(Echo { width: 2 }), arguments: 1 };
    assert_eq!(call.name(), "ECHO");
    let DataType::Multiset(m) = call.return_type(&[]).unwrap() else { panic!("a multiset") };
    assert_eq!(m.element, row_type(&Echo { width: 2 }));
    let read = Value::Multiset(Arc::from(vec![Value::Row(Arc::from(vec![number(2), number(3)])), number(4)]));
    let Value::Multiset(rows) = call.call(&[number(1), read]).unwrap() else { panic!("a multiset") };
    let shown: Vec<String> = rows.iter().map(ToString::to_string).collect();
    assert_eq!(shown.len(), 3, "{shown:?}");
    assert!(shown[1].contains('2') && shown[1].contains('3') && shown[2].contains('4'), "{shown:?}");
    let Value::Multiset(none) = call.call(&[number(1), Value::Null]).unwrap() else { panic!("a multiset") };
    assert_eq!(none.len(), 1);
    assert!(!call.deterministic() && !Echo { width: 2 }.scalar());
    let value = ValueCall(TableCall { function: Arc::new(Echo { width: 2 }), arguments: 1 });
    assert_eq!((value.name(), value.return_type(&[]).unwrap()), ("ECHO", int()));
    assert_eq!(value.call(&[number(7)]).unwrap().to_string(), "7");
    assert!(value.call(&[]).unwrap().is_null() && !value.deterministic());
}

/// Verifies: [DESIGN] RULES.md D125, a table function is held to its
/// columns: a row of another width is refused with SQLCODE -910, and the
/// rows of a query given it as anything but a multiset are corrupt.
#[test]
fn a_table_function_is_held_to_its_columns() {
    let call = TableCall { function: Arc::new(Echo { width: 3 }), arguments: 1 };
    let e = call.call(&[number(1)]).unwrap_err();
    assert!(e.code == Code::Api && e.to_string().contains("3 columns, not 2"), "{e}");
    let bad = TableCall { function: Arc::new(Echo { width: 2 }), arguments: 0 };
    assert_eq!(bad.call(&[number(1)]).unwrap_err().code, Code::Corrupt);
}
