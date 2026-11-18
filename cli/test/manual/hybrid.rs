//! The examples of hybrid search, run as printed: the VECTOR type and its
//! functions registered, VECTOR_DISTANCE of a constant metric declaring
//! it, so FETCH APPROX of it reads an index of its metric.

use std::sync::Arc;

use turborel::db::Database;
use turborel::error::Result;
use turborel::functions::{ScalarFunction, ScalarRef};
use turborel::memory::MemoryBudget;
use turborel::params::ParameterType;
use turborel::value::{DataType, Value};
use turborel::vector::{VectorType, functions};

use super::Client;

/// A VECTOR function, VECTOR_DISTANCE declaring the metric its third
/// argument names when that is a constant (the vector design document
/// R13.1).
struct Declared(Arc<dyn ScalarFunction>);

impl ScalarFunction for Declared {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        self.0.return_type(args)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        self.0.call(args)
    }
    fn deterministic(&self) -> bool {
        self.0.deterministic()
    }
    fn specialize(&self, args: &[Option<&Value>], budget: &Arc<MemoryBudget>) -> Option<ScalarRef> {
        self.0.specialize(args, budget)
    }
    fn argument_type(&self, position: usize, args: &[Option<DataType>]) -> Option<DataType> {
        self.0.argument_type(position, args)
    }
    fn parameter_types(&self) -> Option<Vec<DataType>> {
        self.0.parameter_types()
    }
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        self.0.parameter_type(position, args)
    }
    fn vector_metric(&self, constants: &[Option<&Value>]) -> Option<(String, usize)> {
        let [_, _, Some(Value::Char(metric))] = constants else { return None };
        (self.0.name() == "VECTOR_DISTANCE").then(|| (metric.trim().to_ascii_uppercase(), 0))
    }
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D-APPROX: the examples of hybrid
/// search and of CREATE HYBRID VECTOR INDEX run as printed, as ANN, each
/// in a database of the VECTOR type and its functions, the second of a
/// table HELP.
#[test]
fn the_hybrid_search_examples_run_as_printed() {
    let db = Database::new();
    db.register_type(Box::new(VectorType)).unwrap();
    for f in functions() {
        db.register_scalar_function(Box::new(Declared(f))).unwrap();
    }
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/14-hybrid-search.md") > 0, "it shows its examples' output");
    let db = Database::new();
    db.register_type(Box::new(VectorType)).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE HELP (ID INTEGER PRIMARY KEY, BODY VARCHAR(200), EMB VECTOR(4))").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/statements/create-hybrid-vector-index.md") > 0, "it shows its examples' output");
}
