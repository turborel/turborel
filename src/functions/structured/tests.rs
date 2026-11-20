//! Unit tests of the functions of structured types (`structured.rs`):
//! the types they report, and their refusals of a value the binder
//! never gives them.

use std::sync::Arc;

use super::*;
use crate::value::Decimal;

fn int() -> DataType {
    DataType::Exact { precision: 9, scale: 0, spelled: crate::value::Spelled::Integer }
}

fn point() -> Arc<UdtName> {
    Arc::new(UdtName::new("S", "POINT"))
}

/// An instance of POINT of one attribute, 7.
fn instance() -> Value {
    Value::Structured(Arc::new(Structured::new(point(), Arc::from(vec![Value::Exact(Decimal::new(7, 0))]))))
}

/// Verifies: [DESIGN] RULES.md D81, each function the binder makes of a
/// structured type reports the name its calls show and the type its
/// binding gave it: an observer and mutator their attribute's, the
/// constructor and an instance their type's, the type predicate Boolean
/// and SPECIFICTYPE a varying string of 776 characters.
#[test]
fn each_function_reports_its_bound_type() {
    let x = String::from("X");
    let functions: [(&dyn ScalarFunction, &str, DataType); 9] = [
        (&Observer { name: x.clone(), index: 0, result: int() }, "X", int()),
        (&Mutator { name: x, index: 0, attribute: int(), result: int() }, "X", int()),
        (&Constructed { of: point(), result: int() }, "POINT", int()),
        (&Treat { types: vec![UdtName::new("S", "POINT")], result: int() }, TREAT, int()),
        (&TreatReference { result: int(), same: true }, TREAT, int()),
        (&TypeTest { types: Vec::new() }, TYPE_PREDICATE, DataType::Boolean),
        (&SpecificType { catalog: "C".to_owned() }, SPECIFICTYPE, DataType::Varying { length: 776, units: CharUnits::Characters }),
        (&Keyed { result: int() }, KEYED, int()),
        (&Instanced { of: point(), result: int() }, "POINT", int()),
    ];
    for (f, name, t) in functions {
        assert_eq!((f.name(), f.return_type(&[]).unwrap()), (name, t), "{name}");
    }
}

/// Verifies: [DESIGN] RULES.md D81, the functions of a structured type
/// are given only what binding checked: an observer or mutator of an
/// attribute past the instance's, a value not of a structured type, an
/// instance made of a value not a row, or TREAT of a reference of other
/// than three arguments is refused with SQLCODE -910, a fault of the
/// binding, never taken for data.
#[test]
fn a_value_binding_never_gives_is_refused() {
    let observer = Observer { name: "Y".to_owned(), index: 3, result: int() };
    let e = observer.call(&[instance()]).unwrap_err();
    assert!(e.sqlcode() == -910_i32 && e.to_string().contains("no attribute Y"), "{e}");
    let mutator = Mutator { name: "Y".to_owned(), index: 3, attribute: int(), result: int() };
    assert_eq!(mutator.call(&[instance(), Value::Exact(Decimal::new(1, 0))]).unwrap_err().sqlcode(), -910_i32);
    assert_eq!(observer.call(&[Value::Exact(Decimal::new(1, 0))]).unwrap_err().sqlcode(), -910_i32);
    let instanced = Instanced { of: point(), result: int() };
    assert_eq!(instanced.call(&[Value::Bool(true)]).unwrap_err().sqlcode(), -910_i32);
    assert!(instanced.call(&[]).unwrap().is_null() && instanced.call(&[Value::Null]).unwrap().is_null());
    let reference = TreatReference { result: int(), same: true };
    assert_eq!(reference.call(&[Value::Null]).unwrap_err().sqlcode(), -910_i32);
}
