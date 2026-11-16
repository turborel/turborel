//! Nearest refinement under deterministic transaction interleavings.
//! A scalar hook pauses the index traversal after its range certificate;
//! its distance result remains the declared geometry distance.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::common::{rows, serializable};
use crate::geometry::index::populated;
use turborel::error::{Code, Result};
use turborel::functions::ScalarFunction;
use turborel::geometry::{GeometryFunction, SpatialOperation};
use turborel::params::Params;
use turborel::value::{DataType, Value};

struct Hook {
    first: AtomicBool,
    calls: Arc<AtomicUsize>,
    action: Box<dyn Fn() + Send + Sync>,
}

impl ScalarFunction for Hook {
    fn name(&self) -> &'static str {
        "WATCH_DISTANCE"
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        GeometryFunction::Distance.return_type(args)
    }

    fn argument_type(&self, position: usize, args: &[Option<DataType>]) -> Option<DataType> {
        GeometryFunction::Distance.argument_type(position, args)
    }

    fn spatial_operation(&self) -> Option<SpatialOperation> {
        Some(SpatialOperation::Distance)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let first = self.first.swap(false, Ordering::Relaxed);
        if first {
            (self.action)();
        }
        GeometryFunction::Distance.call(args)
    }
}

/// Verifies: [DESIGN] D15, D29, D82: after certification, another
/// transaction moves a point, replaces a polygon by the same envelope,
/// changes NULL membership and splits pages with extreme new entries.
/// Refinement sees the reader's retained versions, each once, and never
/// evaluates an overflow on an invisible entry. No callback holds a latch.
#[test]
fn nearest_geometry_refines_the_snapshot_across_concurrent_splits() {
    let db = Arc::new(populated());
    let calls = Arc::new(AtomicUsize::new(0));
    let weak = Arc::downgrade(&db);
    db.register_scalar_function(Box::new(Hook {
        first: AtomicBool::new(true),
        calls: Arc::clone(&calls),
        action: Box::new(move || {
            let db = weak.upgrade().unwrap();
            let writer = db.session("A");
            writer.run("UPDATE SHAPES SET G = ST_POINT(10000, 10000, 4326) WHERE ID = 5").unwrap();
            writer.run("UPDATE SHAPES SET G = ST_GEOMFROMTEXT('POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))', 4326) WHERE ID = 2002").unwrap();
            writer.run("UPDATE SHAPES SET G = NULL WHERE ID = 4").unwrap();
            for start in (3000_i32..3400_i32).step_by(100) {
                let values: Vec<_> =
                    (start..start.saturating_add(100)).map(|id| format!("({id}, ST_POINT(1.7E308, 1.7E308, 4326))")).collect();
                writer.run(&format!("INSERT INTO SHAPES VALUES {}", values.join(","))).unwrap();
            }
            writer.commit().unwrap();
        }),
    }))
    .unwrap();
    let reader = db.session("A");
    reader.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    let ordinary = "SELECT ID, ST_DISTANCE(G, ST_POINT(5, 5, 4326)) AS D FROM SHAPES ORDER BY D, ID FETCH FIRST 10 ROWS ONLY";
    let expected = rows(&reader, ordinary).unwrap();
    let watched = ordinary.replace("ST_DISTANCE", "WATCH_DISTANCE");
    assert!(reader.physical_plan(&watched, &Params::new()).unwrap().contains("NearestScan"));
    assert_eq!(rows(&reader, &watched).unwrap(), expected);
    assert!(calls.load(Ordering::Relaxed) < 100, "the nearest path must actually prune");
    reader.commit().unwrap();
    assert_eq!(rows(&reader, ordinary).unwrap_err(), Code::Data);
}

/// Verifies: [DESIGN] D44, D82: cancellation during exact refinement
/// reaches the caller, and a later statement runs normally. A declared
/// distance function is recognized independently of its SQL name.
#[test]
fn nearest_geometry_refinement_observes_cancellation() {
    let db = populated();
    let a = db.session("A");
    let stop = a.canceller();
    db.register_scalar_function(Box::new(Hook {
        first: AtomicBool::new(true),
        calls: Arc::new(AtomicUsize::new(0)),
        action: Box::new(move || stop.cancel()),
    }))
    .unwrap();
    let query = "SELECT ID, WATCH_DISTANCE(G, ST_POINT(5, 5, 4326)) AS D FROM SHAPES ORDER BY D FETCH FIRST 1 ROW ONLY";
    assert!(a.physical_plan(query, &Params::new()).unwrap().contains("NearestScan"));
    assert_eq!(rows(&a, query).unwrap_err(), Code::Cancelled);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM SHAPES").unwrap(), ["2003"]);
}

/// Verifies: [DESIGN] D29, D82: a nearest read records its whole-table
/// predicate even when bounds stop the traversal. Two readers cannot
/// both commit inserts that would replace it with closer geometries.
#[test]
fn nearest_geometry_records_serializable_phantoms() {
    let db = populated();
    serializable(&db);
    let (a, b) = (db.session("A"), db.session("A"));
    let query = "SELECT ID, ST_DISTANCE(G, ST_POINT(5000, 5000, 4326)) AS D FROM SHAPES ORDER BY D FETCH FIRST 1 ROW ONLY";
    assert!(a.physical_plan(query, &Params::new()).unwrap().contains("NearestScan"));
    let first = rows(&a, query).unwrap();
    assert_eq!(first.len(), 1);
    assert!(first[0].starts_with("1999,"));
    assert_eq!(rows(&b, query).unwrap(), first);
    a.run("INSERT INTO SHAPES VALUES (4000, ST_POINT(5000, 5000, 4326))").unwrap();
    b.run("INSERT INTO SHAPES VALUES (4001, ST_POINT(5000, 5000, 4326))").unwrap();
    let commits = [a.commit(), b.commit()];
    assert!(commits.iter().any(Result::is_err));
    for error in commits.into_iter().filter_map(Result::err) {
        assert_eq!(error.code, Code::Conflict);
    }
}
