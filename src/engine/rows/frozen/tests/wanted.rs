//! Unit tests of a table's headers read for the columns a scan reads
//! (`frozen.rs` `group_of`, RULES.md D1173).

use super::*;

/// Of each group of `table`, the columns of `[0, 1]` its header as held
/// holds, none if it is not held.
fn held(table: &RowTable) -> Vec<Option<Vec<bool>>> {
    let directory = table.frozen.directory().unwrap();
    directory.groups.iter().map(|e| e.slot.peek().map(|g| (0..TYPES.len()).map(|c| g.holds(Some(&[c]))).collect())).collect()
}

/// Verifies: [DESIGN] RULES.md D1173: of a table opened again, a scan by
/// batches of one column holds each group's header read for it alone,
/// fewer bytes than read whole; a scan of the other then holds it read for
/// both, as is one read for all, and gives what the table holds; a row
/// fetched of a column its header was not read for reads it again; and a
/// header read for some columns is no hindrance to freeing the groups.
#[test]
fn headers_are_read_for_the_columns_read() {
    let (_, files) = files();
    let (table, ids) = table(&files, 0, 5000);
    ended(&table, 1);
    let cold = TableFiles { headers: Arc::default(), ..files.clone() };
    let opened = Arc::new(RowTable::open(&cold, table.meta(), &TYPES, options(0)).unwrap());
    let batched = |c: usize| {
        let mut out: Vec<String> = Vec::new();
        for batch in Arc::clone(&opened).scan_batches(NO_TRANSACTION, &[c], None, &[]) {
            let batch = batch.unwrap();
            // The batch's one column, the one asked for.
            out.extend((0..batch.rows).map(|r| batch.cell(0, r).to_value().to_string()));
        }
        out
    };
    assert_eq!(batched(0).len(), 5000);
    assert_eq!(held(&opened), [Some(vec![true, false]), Some(vec![true, false])], "the first column alone");
    let (some, _) = cold.headers.held();
    let texts = batched(1);
    assert_eq!(texts.len(), 5000);
    let text = |n: i128| row(n)[1].to_string();
    assert!(texts.contains(&text(4321)) && texts.contains(&text(0)));
    assert_eq!(held(&opened), [Some(vec![true, true]), Some(vec![true, true])], "both, the first kept");
    assert!(cold.headers.held().0 > some, "more bytes held of both");
    let fresh = TableFiles { headers: Arc::default(), ..files.clone() };
    let only = Arc::new(RowTable::open(&fresh, table.meta(), &TYPES, options(0)).unwrap());
    assert_eq!(Arc::clone(&only).scan_batches(NO_TRANSACTION, &[0], None, &[]).map(|b| b.unwrap().rows).sum::<usize>(), 5000);
    let row = only.frozen_row(ids[2500], Some(&[1])).unwrap().unwrap();
    assert_eq!(row[1].to_string(), text(2500));
    assert_eq!(held(&only)[1], Some(vec![true, true]), "read again for the column fetched");
    only.retire_all().unwrap();
}

/// Verifies: [DESIGN] RULES.md D1314 the listing of a table's bytes by
/// column reads every group's header for every column and holds none of
/// them. A table opened again and scanned of one column holds each
/// header read for that column alone, before the listing and after it,
/// the cache's bytes and headers the same, though the listing told the
/// other column's chunks too. A table none of whose headers is held
/// holds none after it. And a header held for every column is taken as
/// it is held, the listing the same.
#[test]
fn a_listing_of_a_tables_bytes_holds_no_header() {
    let (_, files) = files();
    let (table, _) = table(&files, 0, 5000);
    ended(&table, 1);
    let cold = TableFiles { headers: Arc::default(), ..files.clone() };
    let opened = Arc::new(RowTable::open(&cold, table.meta(), &TYPES, options(0)).unwrap());
    assert_eq!(Arc::clone(&opened).scan_batches(NO_TRANSACTION, &[0], None, &[]).map(|b| b.unwrap().rows).sum::<usize>(), 5000);
    let before = (held(&opened), cold.headers.held());
    assert_eq!(before.0, [Some(vec![true, false]), Some(vec![true, false])], "the first column alone");
    let listed = opened.measured(true).unwrap();
    assert!(listed.columns.iter().any(|c| c.column == 1 && c.chunks.is_some()), "the other column's chunks told");
    assert_eq!((held(&opened), cold.headers.held()), before, "what was held is held, and no more");
    let fresh = TableFiles { headers: Arc::default(), ..files.clone() };
    let only = RowTable::open(&fresh, table.meta(), &TYPES, options(0)).unwrap();
    assert_eq!(only.measured(true).unwrap().columns, listed.columns);
    assert_eq!(fresh.headers.held(), (0, 0), "none held, none read to be held");
    let whole = files.headers.held();
    assert_eq!(whole.1, 2, "the headers written, held for every column");
    assert_eq!(table.measured(true).unwrap().columns, listed.columns);
    assert_eq!(files.headers.held(), whole);
}
