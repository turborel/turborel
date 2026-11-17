//! Preparation remains effective after a statement learns its physical
//! plan, without overriding disabled or absent optimizer rules (D45, D85).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use turborel::db::Database;
use turborel::error::{Code, Result, SqlError};
use turborel::functions::{ScalarFunction, ScalarRef};
use turborel::memory::{MemoryBudget, Reservation};
use turborel::optimizer::Optimizer;
use turborel::value::{DataType, Decimal, Spelled, Value};

const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

#[derive(Default)]
struct Counts {
    ordinary: AtomicUsize,
    prepared: AtomicUsize,
}

/// Instrumentation counts the implementation used, without changing the
/// SQL result: the function always returns its second argument.
struct Second {
    counts: Arc<Counts>,
    held: Option<Reservation>,
}

impl ScalarFunction for Second {
    fn name(&self) -> &'static str {
        "PREPARED_SECOND"
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(INTEGER)
    }

    fn argument_type(&self, _position: usize, _args: &[Option<DataType>]) -> Option<DataType> {
        Some(INTEGER)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        let counter = if self.held.is_some() { &self.counts.prepared } else { &self.counts.ordinary };
        counter.fetch_add(1, Ordering::Relaxed);
        args.get(1).cloned().ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] the test function needs two arguments"))
    }

    fn specialize(&self, args: &[Option<&Value>], budget: &Arc<MemoryBudget>) -> Option<ScalarRef> {
        if self.held.is_some() || args.get(1).copied().flatten().is_none() {
            return None;
        }
        let held = budget.reserve(size_of::<Self>(), "a test scalar preparation").ok()?;
        Some(ScalarRef(Arc::new(Self { counts: Arc::clone(&self.counts), held: Some(held) })))
    }
}

/// Verifies: [DESIGN] D45, D85: preparation uses each execution's values
/// after plan learning, in projections and filters. This regresses the
/// loss of preparation when cached slots were merely filled. Disabling
/// constant folding or removing its rule independently prevents it.
#[test]
fn reused_plans_keep_scalar_preparation_and_respect_its_switch() {
    for mode in ["ON", "OFF", "ABSENT"] {
        let db = Database::new();
        let counts = Arc::new(Counts::default());
        db.register_scalar_function(Box::new(Second { counts: Arc::clone(&counts), held: None })).unwrap();
        db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
        if mode == "ABSENT" {
            db.with_optimizer(|optimizer| *optimizer = Optimizer::empty()).unwrap();
        } else {
            db.set_setting("OPTIMIZER.CONSTANT_FOLDING", &Value::Char(mode.into())).unwrap();
        }
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER)").unwrap();
        let a = db.session("A");
        a.run("INSERT INTO T VALUES (1), (2), (3), (4)").unwrap();
        a.commit().unwrap();
        for sql in [
            "SELECT PREPARED_SECOND(ID, ?) FROM T",
            "SELECT ID FROM T WHERE PREPARED_SECOND(ID, ?) > 0",
            "SELECT PREPARED_SECOND(ID, CAST(? AS INTEGER) + 0) FROM T",
            "SELECT ID FROM T WHERE PREPARED_SECOND(ID, CAST(? AS INTEGER) + 0) > 0",
        ] {
            let query = a.prepare(sql).unwrap();
            for n in 1_i32..=16 {
                let value = Value::Exact(Decimal::new(i128::from(n), 0));
                let mut cursor = a.open_prepared(&query, &[value]).unwrap();
                let mut found = Vec::new();
                while let Some(row) = cursor.fetch().unwrap() {
                    found.push(row[0].to_string());
                }
                assert_eq!(found.len(), 4);
                if sql.starts_with("SELECT PREPARED") {
                    assert_eq!(found, vec![n.to_string(); 4]);
                }
            }
            assert_eq!(query.plans_made(), 1);
        }
        let expected = if mode == "ON" { (0, 256) } else { (256, 0) };
        assert_eq!((counts.ordinary.load(Ordering::Relaxed), counts.prepared.load(Ordering::Relaxed)), expected, "{mode}");
    }
}
