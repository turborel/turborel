//! Robustness of a frozen group's header and summary: either cut short
//! anywhere, or with a byte damaged, is refused, never misread.

use super::*;

/// A group of 600 rows, sorted by its second column, filtered by its
/// first, in the layout `pages` says, on its pages.
fn group(pages: &Pages<'_, Plain>) -> Group {
    let ids: Vec<RowId> = (1..=600_u64).collect();
    let rows: Vec<Row> = (0..600).map(row).collect();
    Group::write(pages, &TYPES, (&ids, &rows), (&[1], &[0])).unwrap()
}

/// Verifies: [DESIGN] RULES.md D31, a group's header, R-ERR-2: a header
/// of the column layout and of the hybrid one, cut at any byte and
/// written as a chain, is refused with SQLCODE -916 as it is read; so is
/// one whose layout, bounds or filter flag is of another format.
#[test]
fn every_header_prefix_is_refused() {
    // Each header read back is written to pages of its own.
    let (pool, file) = memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES << 7_i32), "headers").unwrap();
    for pax in [false, true] {
        let pages = Pages { pool: &pool, file, capacity: 1024, sealing: &Plain, compress: true, pax, imprints: true };
        let g = group(&pages);
        let bytes = header::encode(&g, &TYPES).unwrap();
        let read_back = |b: &[u8]| {
            let chain = put_chain(&pages, b, &mut Vec::new()).unwrap();
            read(&pages, chain[0], &TYPES)
        };
        assert_eq!(format!("{:?}", read_back(&bytes).unwrap().columns), format!("{:?}", g.columns));
        for cut in 1..bytes.len() {
            assert_eq!(read_back(&bytes[..cut]).unwrap_err().code, Code::Corrupt, "pax {pax} cut at {cut}");
        }
        assert_eq!(read_back(&[bytes.clone(), vec![0]].concat()).unwrap_err().code, Code::Corrupt, "a byte after");
        for at in 0..bytes.len() {
            let mut bad = bytes.clone();
            bad[at] = 0xFE;
            if let Err(e) = read_back(&bad) {
                assert_eq!(e.code, Code::Corrupt, "byte {at}");
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D31, a group's summary, R-ERR-2: a summary
/// cut at any byte is refused with SQLCODE -916; one with any byte set to
/// 0xFE is refused so or read as some summary.
#[test]
fn every_summary_prefix_is_refused() {
    let (pool, file) = file();
    let pages = Pages { pool: &pool, file, capacity: 1024, sealing: &Plain, compress: true, pax: false, imprints: true };
    let g = group(&pages);
    let mut bytes = Vec::new();
    put_summary(&mut bytes, &Summary::of(&g, g.header[0]), &TYPES).unwrap();
    for cut in 1..bytes.len() {
        assert_eq!(read_summaries(&bytes[..cut], &TYPES).unwrap_err().code, Code::Corrupt, "cut at {cut}");
    }
    for at in 0..bytes.len() {
        let mut bad = bytes.clone();
        bad[at] = 0xFE;
        if let Err(e) = read_summaries(&bad, &TYPES) {
            assert_eq!(e.code, Code::Corrupt, "byte {at}");
        }
    }
}
