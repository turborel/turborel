//! An index probe prepares its fixed outer operand without replacing the
//! complete SQL condition or retaining the previous probe's operand.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{populated, windows};
use crate::common::rows;
use turborel::error::Result;
use turborel::functions::{ScalarFunction, ScalarRef};
use turborel::geometry::{GeometryFunction, SpatialOperation};
use turborel::memory::MemoryBudget;
use turborel::params::Params;
use turborel::value::{DataType, Value};

const NAME: &str = "COUNTED_INTERSECTS";

#[derive(Default)]
struct Counts {
    ordinary: AtomicUsize,
    prepared: AtomicUsize,
}

struct Counted {
    inner: ScalarRef,
    counts: Arc<Counts>,
    prepared: bool,
}

impl ScalarFunction for Counted {
    fn name(&self) -> &'static str {
        NAME
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        self.inner.0.return_type(args)
    }

    fn spatial_operation(&self) -> Option<SpatialOperation> {
        self.inner.0.spatial_operation()
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        let count = if self.prepared { &self.counts.prepared } else { &self.counts.ordinary };
        count.fetch_add(1, Ordering::Relaxed);
        self.inner.0.call(args)
    }

    fn specialize(&self, args: &[Option<&Value>], budget: &Arc<MemoryBudget>) -> Option<ScalarRef> {
        let inner = self.inner.0.specialize(args, budget)?;
        Some(ScalarRef(Arc::new(Self { inner, counts: Arc::clone(&self.counts), prepared: true })))
    }
}

/// Verifies: [DESIGN] D15, D82: each outer row supplies the prepared
/// operand for its own probe, in either argument position. All candidate
/// evaluations use preparation only when constant folding is enabled.
/// Holes, empty/NULL operands, projected columns and residual comparisons
/// give the same results with the index removed. Disabling preparation
/// alone regresses the instrumented calls, even if SQL results still match.
#[test]
fn index_join_prepares_each_outer_operand_and_respects_its_switch() {
    for mode in ["ON", "OFF"] {
        let db = populated();
        windows(&db, "GEOMETRY(4326)");
        let counts = Arc::new(Counts::default());
        let function = Counted { inner: ScalarRef(Arc::new(GeometryFunction::Intersects)), counts: Arc::clone(&counts), prepared: false };
        db.register_scalar_function(Box::new(function)).unwrap();
        db.set_setting("OPTIMIZER.CONSTANT_FOLDING", &Value::Char(mode.into())).unwrap();
        let a = db.session("A");
        let queries = [
            "SELECT P.ID, W.ID FROM WINDOWS W JOIN SHAPES P ON COUNTED_INTERSECTS(P.G, W.G) = 1 ORDER BY P.ID, W.ID",
            "SELECT P.ID, W.ID FROM SHAPES P JOIN WINDOWS W ON 1 = COUNTED_INTERSECTS(W.G, P.G) AND P.ID < W.ID ORDER BY P.ID, W.ID",
        ];
        let mut results = Vec::new();
        for query in queries {
            assert!(a.physical_plan(query, &Params::new()).unwrap().contains("IndexNestedLoop"));
            results.push(rows(&a, query).unwrap());
        }
        assert_eq!(counts.ordinary.load(Ordering::Relaxed) == 0, mode == "ON");
        assert_eq!(counts.prepared.load(Ordering::Relaxed) > 0, mode == "ON");
        assert_eq!(results[0], ["0,10", "1,10", "2,10", "3,10", "5,11", "2002,10"]);
        assert_eq!(results[1], ["0,10", "1,10", "2,10", "3,10", "5,11"]);
        a.commit().unwrap();
        db.drop_index("SHAPES_G").unwrap();
        for (query, expected) in queries.into_iter().zip(results) {
            assert_eq!(rows(&a, query).unwrap(), expected);
        }
    }
}
