//! An inverted index's approximate search of a call of DOT (the vector
//! design document §9.2, §9.4, R13.1, R13.2; RULES.md D-APPROX): which
//! calls it serves, its candidates nearest first by the negated dot
//! product, a radius, an in-filter, and what it declines.

use super::*;
use crate::error::SqlError;
use crate::functions::ScalarRef;
use crate::index::{Approximation, FilterHint};
use crate::vector::{Layout as VectorLayout, Shape, typed, value_of};

/// The dimensions of the column.
const DIMS: u32 = 100;

/// VECTOR(`dims`, FLOAT32, `layout`).
fn column(dims: Option<u32>, layout: VectorLayout) -> DataType {
    typed(Shape { dims, element: Some(Element::Float32), layout: Some(layout) }).unwrap()
}

/// An index of a fixed sparse column, rows 1 to 6 holding dimension 1 at
/// weight `row` and row 7 dimension 50 alone.
fn filled() -> (SparseMethod, Arc<dyn Index>) {
    let method = SparseMethod::in_memory();
    let index: Arc<dyn Index> = Arc::from(method.create(&[column(Some(DIMS), VectorLayout::Sparse)]).unwrap());
    for row in 1..=6_u16 {
        index.insert(key(&sparse(&[(1, f32::from(row)), (2, 1.0)])), u64::from(row)).unwrap();
    }
    index.insert(key(&sparse(&[(50, 9.0)])), 7).unwrap();
    (method, index)
}

/// VECTOR_DISTANCE of `metric`, approximately for `first` rows, of `query`.
fn query(metric: &str, value: Value, approximation: Approximation) -> NearestQuery {
    let function = crate::vector::functions().into_iter().find(|f| f.name() == "VECTOR_DISTANCE").unwrap();
    NearestQuery {
        function: ScalarRef(function),
        column: 0,
        argument: 0,
        value,
        descending: false,
        statistics: None,
        metric: Some(metric.to_owned()),
        approximate: Some(approximation),
    }
}

fn asked(first: usize, radius: Option<f64>, filter: FilterHint) -> Approximation {
    Approximation { first, search_list: None, beam_width: None, probes: None, accuracy: None, widen: 1, radius, filter }
}

/// The candidates, (distance, row), of `q`, or none if declined.
fn found(index: &Arc<dyn Index>, q: &NearestQuery, filter: Option<&mut dyn RowFilter>) -> Option<Vec<(f64, RowId)>> {
    let budget = MemoryBudget::new(usize::MAX);
    let entries = Arc::clone(index).approximate(q, &budget, filter).unwrap()?;
    Some(entries.map(|e| e.map(|(d, _, r)| (d, r)).unwrap()).collect())
}

/// The query vector of dimension 1 at 1.
fn on_one() -> Value {
    value_of(sparse(&[(1, 1.0)]).image())
}

/// Admits odd rows; or fails on every row.
struct Odd(bool);

impl RowFilter for Odd {
    fn admits(&mut self, _key: &Key, row: RowId) -> Result<bool> {
        if self.0 {
            return Err(SqlError::new(Code::Data, "[DESIGN] refused"));
        }
        Ok(row % 2 == 1)
    }
}

/// Verifies: [EXTENSION] vector_standard R13.1, R10.3 The method serves
/// approximately a call of a vector metric asked approximately, ascending,
/// on one column of SPARSE layout fixing its dimension count and format,
/// and orders by DOT; it serves no exact search, no score, and no column
/// of open dimensions or DENSE layout.
#[test]
fn it_serves_an_approximate_search_of_a_fixed_sparse_column() {
    let method = SparseMethod::in_memory();
    let fixed = column(Some(DIMS), VectorLayout::Sparse);
    let q = query("DOT", on_one(), asked(3, None, FilterHint::None));
    assert_eq!(method.supports_nearest(&q, std::slice::from_ref(&fixed)), NearestSupport::Approximate);
    assert_eq!(method.metric(&desc(Vec::new())).as_deref(), Some("DOT"));
    let exact = NearestQuery { approximate: None, ..q.clone() };
    assert_eq!(method.supports_nearest(&exact, std::slice::from_ref(&fixed)), NearestSupport::No);
    let score = NearestQuery { descending: true, ..q.clone() };
    assert_eq!(method.supports_nearest(&score, std::slice::from_ref(&fixed)), NearestSupport::No);
    assert_eq!(method.supports_nearest(&q, &[column(None, VectorLayout::Sparse)]), NearestSupport::No);
    assert_eq!(method.supports_nearest(&q, &[fixed.clone(), fixed]), NearestSupport::No);
    assert_eq!(method.create(&[column(Some(DIMS), VectorLayout::Dense)]).err().unwrap().code, Code::Unsupported);
}

/// Verifies: [EXTENSION] vector_standard §9.2, R6.8, R13.2 The candidates
/// are the rows of the greatest dot product, nearest first, each distance
/// the dot product negated; a post-filter's share asks for more rows; a
/// dense query vector is read by its dimensions not zero.
#[test]
fn candidates_are_the_greatest_dot_products_negated() {
    let (_, index) = filled();
    let top = found(&index, &query("DOT", on_one(), asked(2, None, FilterHint::None)), None).unwrap();
    assert_eq!(top, [(-6.0_f64, 6), (-5.0_f64, 5)]);
    let post = found(&index, &query("dot", on_one(), asked(2, None, FilterHint::PostFilter(0.5))), None).unwrap();
    assert_eq!(post.iter().map(|(_, r)| *r).collect::<Vec<_>>(), [6, 5, 4, 3]);
    let mut dense = vec![0.0_f64; usize::try_from(DIMS).unwrap()];
    dense[1] = 1.0_f64;
    let dense = crate::vector::Vector::dense(Element::Float32, dense).unwrap().value().unwrap();
    let one = found(&index, &query("DOT", dense, asked(1, None, FilterHint::None)), None).unwrap();
    assert_eq!(one, [(-6.0_f64, 6)]);
}

/// Verifies: [EXTENSION] vector_standard §9.4 R9.1, R13.2 A radius keeps
/// every row of a distance at most it, of a dot product at least its
/// negation; an in-filter is asked of the rows found and passes over
/// those it refuses; its error ends the search.
#[test]
fn a_radius_and_an_in_filter_narrow_the_candidates() {
    let (_, index) = filled();
    let within = found(&index, &query("DOT", on_one(), asked(1, Some(-4.0_f64), FilterHint::None)), None).unwrap();
    assert_eq!(within.iter().map(|(_, r)| *r).collect::<Vec<_>>(), [6, 5, 4]);
    let q = query("DOT", on_one(), asked(2, None, FilterHint::InFilter));
    let odd = found(&index, &q, Some(&mut Odd(false))).unwrap();
    assert_eq!(odd, [(-5.0_f64, 5), (-3.0_f64, 3)]);
    let budget = MemoryBudget::new(usize::MAX);
    let failed = Arc::clone(&index).approximate(&q, &budget, Some(&mut Odd(true)));
    assert_eq!(failed.err().map(|e| e.code), Some(Code::Data));
}

/// Verifies: [EXTENSION] RULES.md D-APPROX, vector_standard R6.4, R13.1
/// The search declines, for the executor to read the table, a call of
/// another metric, a query vector of other dimensions or format, a null,
/// an exact search, and an index of a column of open dimensions.
#[test]
fn it_declines_what_the_call_could_raise_on() {
    let (_, index) = filled();
    let first = asked(2, None, FilterHint::None);
    assert!(found(&index, &query("COSINE", on_one(), first), None).is_none());
    let wide = value_of(SparseVector::new(DIMS + 1, [(1, 1.0)]).unwrap().image());
    assert!(found(&index, &query("DOT", wide, first), None).is_none());
    let int8 = crate::vector::Vector::sparse(Element::Int8, DIMS, vec![(1, 1.0_f64)]).unwrap().value().unwrap();
    assert!(found(&index, &query("DOT", int8, first), None).is_none());
    assert!(found(&index, &query("DOT", Value::Null, first), None).is_none());
    let exact = NearestQuery { approximate: None, ..query("DOT", on_one(), first) };
    assert!(found(&index, &exact, None).is_none());
    let method = SparseMethod::in_memory();
    let open: Arc<dyn Index> = Arc::from(method.create(&[column(None, VectorLayout::Sparse)]).unwrap());
    open.insert(key(&sparse(&[(1, 1.0)])), 1).unwrap();
    assert!(found(&open, &query("DOT", on_one(), first), None).is_none());
}
