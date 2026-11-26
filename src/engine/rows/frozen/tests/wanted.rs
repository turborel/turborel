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
