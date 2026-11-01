//! Unit tests of a batch's columns (`batch.rs`).

use super::*;

fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// `values` as they are written.
fn shown(values: &[Value]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

/// Verifies: [DESIGN] RULES.md D31, a batch's columns: integers give the
/// values of their units at their scale, nulls as nulls, made once; their
/// cells are read in place, a value's borrowed, an integer's its units,
/// past the end or of a column not there a null.
#[test]
fn columns_of_values_and_of_integers() {
    let ints = Ints::new(vec![150, -3, 0], 2, Some(vec![false, false, true]));
    assert_eq!(shown(ints.values()), ["1.50", "-0.03", "NULL"]);
    assert!(ints.is_null(2) && !ints.is_null(0) && !ints.is_null(9), "past the end, not null");
    let batch =
        Batch { columns: vec![Column::Ints(ints.clone()), Column::from(vec![Value::Char("a".into()), Value::Null, Value::Null])], rows: 3 };
    assert!(matches!(batch.cell(0, 0), Cell::Exact(150, 2)));
    assert!(matches!(batch.cell(0, 2), Cell::Null) && batch.cell(0, 2).is_null());
    assert!(matches!(batch.cell(1, 0), Cell::Value(Value::Char(_))) && !batch.cell(1, 0).is_null());
    assert!(batch.cell(1, 1).is_null(), "a value that is null");
    assert!(matches!(batch.cell(0, 7), Cell::Null) && matches!(batch.cell(5, 0), Cell::Null), "past the end, no column");
}

/// Verifies: [DESIGN] RULES.md D31, a batch's cells' values: an integer's
/// made of its units and scale, a value's cloned, a null's null; a value
/// of a vector of integers, and none of a column not there.
#[test]
fn cells_give_their_values() {
    let ints = Ints::new(vec![150, -3, 0], 2, Some(vec![false, false, true]));
    let batch = Batch { columns: vec![Column::Ints(ints), Column::from(vec![Value::Char("a".into()), Value::Null, Value::Null])], rows: 3 };
    assert_eq!(batch.cell(0, 1).to_value().to_string(), "-0.03");
    assert_eq!(batch.cell(1, 0).to_value().to_string(), "a");
    assert!(batch.cell(0, 2).to_value().is_null());
    assert_eq!(batch.value(0, 1).map(ToString::to_string), Some("-0.03".to_owned()));
    assert!(batch.values(9).is_empty(), "no column");
}

/// Verifies: [DESIGN] RULES.md D31, a batch's columns changed: a column of
/// integers changed becomes one of values, and a row pushed to a batch of
/// them is added to those values; a column's values owned are its own.
#[test]
fn columns_of_integers_changed_become_of_values() {
    let ints = Ints::new(vec![150, -3, 0], 2, Some(vec![false, false, true]));
    let mut batch =
        Batch { columns: vec![Column::Ints(ints.clone()), Column::from(vec![Value::Char("a".into()), Value::Null, Value::Null])], rows: 3 };
    batch.push(&vec![exact(7, 0), Value::Char("b".into())], &[0, 1]);
    assert!(matches!(batch.columns.first(), Some(Column::Values(_))), "of values once changed");
    assert_eq!(shown(batch.values(0)), ["1.50", "-0.03", "NULL", "7"]);
    assert_eq!(batch.rows, 4);
    assert_eq!(shown(&Column::Ints(ints).into_values()), ["1.50", "-0.03", "NULL"]);
    assert!(Column::default().into_values().is_empty());
    assert!(Ints::default().values().is_empty());
}

/// Verifies: [DESIGN] RULES.md D31, a batch's approximate numbers: their
/// values, nulls as nulls, made once; their cells their numbers, read in
/// place; changed, a column of them becomes one of values; owned, their
/// values are their own.
#[test]
fn columns_of_approximate_numbers() {
    let floats = Floats::new(vec![1.5_f64, -0.0_f64, 0.0_f64], Some(vec![false, false, true]));
    assert_eq!(shown(floats.values()), ["1.5E0", "-0E0", "NULL"]);
    assert_eq!((floats.get(0), floats.get(2), floats.get(9)), (Some(1.5_f64), None, None));
    let mut batch = Batch { columns: vec![Column::Floats(floats.clone())], rows: 3 };
    assert!(matches!(batch.cell(0, 0), Cell::Approx(f) if f.to_bits() == 1.5_f64.to_bits()) && !batch.cell(0, 0).is_null());
    assert!(batch.cell(0, 2).is_null());
    assert_eq!(batch.cell(0, 1).to_value().to_string(), "-0E0");
    batch.push(&vec![Value::Approx(2.0_f64)], &[0]);
    assert!(matches!(batch.columns.first(), Some(Column::Values(_))), "of values once changed");
    assert_eq!(shown(batch.values(0)), ["1.5E0", "-0E0", "NULL", "2E0"]);
    assert_eq!(shown(&Column::Floats(floats).into_values()), ["1.5E0", "-0E0", "NULL"]);
    assert!(Floats::default().values().is_empty());
}

/// Verifies: [DESIGN] RULES.md D31, a column gathered by row, as a join
/// pairs a batch's rows: each kind gathered as one of its kind, a row taken
/// again as often as picked, in the order picked, a null where none is
/// picked or the row's is null; texts of a store's dictionaries keep their
/// entries; a vector of nulls holds none, its cells null, its values
/// made once asked for, gathered as nulls.
#[test]
fn columns_gathered_by_row_keep_their_kind() {
    let picks = [Some(2_usize), None, Some(0_usize), Some(2_usize)];
    let ints = Column::Ints(Ints::new(vec![150_i64, -3_i64, 7_i64], 2_u32, Some(vec![false, true, false])));
    let Column::Ints(gathered) = ints.gathered(&picks) else { panic!("integers") };
    assert_eq!(shown(gathered.values()), ["0.07", "NULL", "1.50", "0.07"]);
    let Column::Ints(whole) = ints.gathered(&[Some(0_usize), Some(2_usize)]) else { panic!("integers") };
    assert!(whole.nulls.is_none(), "no row null, no nulls kept");
    let Column::Floats(floats) = Column::Floats(Floats::new(vec![1.5_f64, 2.5_f64, -1.0_f64], None)).gathered(&picks) else {
        panic!("numbers")
    };
    assert_eq!(shown(floats.values()), ["-1E0", "NULL", "1.5E0", "-1E0"]);
    let texts = Texts::new("abcde".to_owned(), vec![1_usize, 3_usize, 5_usize], Some(vec![false, false, true]));
    let Column::Texts(plain) = Column::Texts(texts).gathered(&[Some(1_usize), None, Some(2_usize), Some(0_usize)]) else { panic!("texts") };
    assert_eq!((plain.get(0_usize), plain.get(1_usize), plain.get(2_usize), plain.get(3_usize)), (Some("bc"), None, None, Some("a")));
    let entries = Texts::new("xy".to_owned(), vec![1_usize, 2_usize], None);
    let coded = Column::Texts(Texts::coded(Coded { codes: vec![1_u32, 0_u32, 1_u32], entries }, None));
    let Column::Texts(recoded) = coded.gathered(&picks) else { panic!("texts") };
    assert!(recoded.coded.is_some(), "entries kept");
    assert_eq!(shown(recoded.values()), ["y", "NULL", "y", "y"]);
    let values = Column::from(vec![Value::Char("v".into()), Value::Null, exact(5_i128, 0_u32)]);
    assert_eq!(shown(values.gathered(&picks).values()), ["5", "NULL", "v", "5"]);
    let nulls = Column::nulls(2_usize);
    let Column::Nulls(held) = &nulls else { panic!("nulls") };
    assert_eq!(held.rows, 2_usize, "none held");
    assert!(nulls.values().len() == 2_usize && nulls.values().iter().all(Value::is_null), "made as values once asked for");
    let null_picks = nulls.gathered(&picks);
    assert_eq!(shown(null_picks.values()), ["NULL"; 4]);
    let batch = Batch { columns: vec![null_picks], rows: 4 };
    assert!(batch.cell(0, 1).is_null() && batch.cell(0, 9).is_null());
    assert_eq!(nulls.into_values().len(), 2_usize);
}

/// Verifies: [DESIGN] RULES.md D31, integers known as runs: kept only
/// with ends that close each run, the last their count; each run's unit
/// and the rows of a list in increasing order in it, or none of a list
/// not so; each run's unit and its rows not null; of nulls among them,
/// their ends not given as those of runs each row of which is the run's.
#[test]
fn integers_known_as_runs() {
    let units = vec![5, 5, 5, 9, 9, -1];
    let runs = Ints::new(units.clone(), 0, None).with_runs(vec![3, 5, 6]);
    let rows = [0_usize, 2, 4, 5];
    let of: Vec<(i64, Vec<usize>)> = runs.runs_of(&rows).unwrap().map(|(u, r)| (u, r.to_vec())).collect();
    assert_eq!(of, [(5, vec![0, 2]), (9, vec![4]), (-1, vec![5])]);
    assert!(runs.runs_of(&[4, 0]).is_none(), "rows not in increasing order");
    assert_eq!(runs.run_lengths().unwrap().collect::<Vec<_>>(), [(5, 3), (9, 2), (-1, 1)]);
    let refused = |ints: Ints| ints.run_lengths().is_none();
    let with_nulls = Ints::new(units.clone(), 0, Some(vec![false, true, false, false, false, true])).with_runs(vec![3, 5, 6]);
    assert_eq!(with_nulls.run_lengths().unwrap().collect::<Vec<_>>(), [(5, 2), (9, 2), (-1, 0)], "nulls left out");
    assert!(with_nulls.runs().is_none() && runs.runs() == Some(&[3, 5, 6][..]), "runs of rows each its run's, only with no null");
    assert_eq!((with_nulls.held(&rows), runs.held(&rows)), (3, 4));
    assert!(refused(Ints::new(units.clone(), 0, None).with_runs(vec![3, 5])), "not to the end");
    assert!(refused(Ints::new(units.clone(), 0, None).with_runs(vec![3, 3, 6])), "an end not past the last");
    assert!(refused(Ints::new(units, 0, None)), "not known");
}

/// Verifies: [DESIGN] RULES.md D31, the rows of a scan's batches: the
/// first holds `FIRST_BATCH_ROWS`, each next twice the last's, up to
/// `BATCH_ROWS`, and then each that many.
#[test]
fn batches_grow_to_their_most() {
    let mut ramp = Ramp::default();
    let sizes: Vec<usize> = (0_u32..10).map(|_| ramp.next_rows()).collect();
    assert_eq!(sizes, [16, 32, 64, 128, 256, 512, 1024, 2048, 2048, 2048]);
    assert_eq!(sizes.first(), Some(&FIRST_BATCH_ROWS));
}

/// Verifies: [DESIGN] RULES.md D38: a batch is counted as holding its
/// vectors' bytes: integers' and numbers' 8 bytes each and their nulls,
/// values as rows count them, texts their bytes and ends, a store's
/// dictionary its codes and entries.
#[test]
fn batches_count_the_bytes_their_vectors_hold() {
    let ints = Column::Ints(Ints::new(vec![1_i64, 2_i64], 0_u32, Some(vec![false, true])));
    assert_eq!(Column::bytes(&ints), 18_usize);
    assert_eq!(Column::bytes(&Column::Floats(Floats::new(vec![1.0_f64], None))), 8_usize);
    let values = Column::Values(vec![Value::Char("abc".to_owned())]);
    assert_eq!(Column::bytes(&values), size_of::<Value>() + 3_usize);
    let texts = Texts::new("abcd".to_owned(), vec![2_usize, 4_usize], None);
    assert_eq!(Column::bytes(&Column::Texts(texts.clone())), 4_usize + 2_usize * size_of::<usize>());
    let coded = Texts::coded(Coded { codes: vec![0_u32, 1_u32, 1_u32], entries: texts }, None);
    assert_eq!(Column::bytes(&Column::Texts(coded)), 3_usize * 4_usize + 4_usize + 2_usize * size_of::<usize>());
    let batch = Batch { columns: vec![ints, values], rows: 2 };
    assert_eq!(batch.bytes(), 18_usize + size_of::<Value>() + 3_usize);
}

/// Two batches of a column of each kind: integers of one scale and of
/// two, numbers, texts (a store's dictionary's and plain), and nulls the
/// second lacks.
fn two_batches() -> Vec<(Batch, Vec<usize>)> {
    let ints = |units: Vec<i64>, scale: u32| Column::Ints(Ints::new(units, scale, None));
    let entries = Texts::new("xy".to_owned(), vec![1_usize, 2_usize], None);
    let coded = Column::Texts(Texts::coded(Coded { codes: vec![1_u32, 0_u32], entries }, None));
    let first = Batch {
        columns: vec![
            ints(vec![1, 2], 0),
            ints(vec![5, 6], 1),
            Column::Floats(Floats::new(vec![1.5_f64, 2.5_f64], Some(vec![false, true]))),
            coded,
            Column::nulls(2),
        ],
        rows: 2,
    };
    let second = Batch {
        columns: vec![
            Column::Ints(Ints::new(vec![7, 0], 0, Some(vec![false, true]))),
            ints(vec![8, 9], 2),
            Column::Floats(Floats::new(vec![-1.0_f64, 0.0_f64], None)),
            Column::Texts(Texts::new("ab".to_owned(), vec![1_usize, 2_usize], None)),
        ],
        rows: 2,
    };
    vec![(first, vec![1_usize, 0_usize]), (second, vec![0_usize, 1_usize])]
}

/// The kind of each vector of `batch`, and its values as written.
fn kinds_and_values(batch: &Batch) -> Vec<(&'static str, Vec<String>)> {
    let kind = |c: &Column| match c {
        Column::Ints(_) => "ints",
        Column::Floats(_) => "floats",
        Column::Texts(t) => {
            if t.coded.is_some() {
                "coded"
            } else {
                "texts"
            }
        }
        Column::Values(_) => "values",
        Column::Nulls(_) => "nulls",
    };
    batch.columns.iter().map(|c| (kind(c), shown(c.values()))).collect()
}

/// Verifies: [DESIGN] RULES.md D31, D38: batches' rows joined into one
/// batch, each at the places beside its batch, in order: integers of one
/// scale stay integers, of two become values; numbers and texts (a store's
/// dictionary's too) stay of their kind, their nulls kept; a column one
/// part lacks is null there; one part of all its rows is that batch, its
/// texts' dictionary kept; none is a batch of no rows.
#[test]
fn batches_joined_keep_their_kinds() {
    let joined = Batch::joined(two_batches());
    assert_eq!(joined.rows, 4_usize);
    let to = |kind: &'static str, values: [&str; 4]| (kind, values.map(String::from).to_vec());
    assert_eq!(
        kinds_and_values(&joined),
        [
            to("ints", ["2", "1", "7", "NULL"]),
            to("values", ["0.6", "0.5", "0.08", "0.09"]),
            to("floats", ["NULL", "1.5E0", "-1E0", "0E0"]),
            to("texts", ["x", "y", "a", "b"]),
            to("values", ["NULL"; 4]),
        ]
    );
    let whole = two_batches().into_iter().next().map(|(batch, _)| batch).unwrap();
    let same = Batch::joined(vec![(whole, vec![0_usize, 1_usize])]);
    assert_eq!(kinds_and_values(&same)[3].0, "coded", "as it is");
    assert_eq!(Batch::joined(Vec::new()).rows, 0_usize);
}
