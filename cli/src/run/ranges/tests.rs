use std::fmt::Write as _;
use std::io::Cursor;

use super::*;

/// How CSV is written, with a header if `header`.
fn csv(header: bool) -> Written {
    Written { quoted: true, header }
}

/// `text` split into `jobs` ranges as `written` says.
fn split_of(text: &str, jobs: usize, written: Written) -> Result<Split, String> {
    split(&mut Cursor::new(text.as_bytes()), u64::try_from(text.len()).unwrap_or(u64::MAX), jobs, written)
}

/// Whether a range of `split` of `text` begins within a quoted field, by
/// the quotes of each range.
fn straddled(text: &str, split: &Split) -> bool {
    let counts: Vec<u64> = split.ranges.iter().map(|r| quotes(&mut Cursor::new(text.as_bytes()), *r).unwrap()).collect();
    straddles(&counts)
}

/// The lines and records before each range of `split` of `text`.
fn befores(text: &str, split: &Split, written: Written) -> Vec<Before> {
    let starts: Vec<u64> = split.ranges.iter().map(|r| r.start).collect();
    before(&mut text.as_bytes(), &starts, written).unwrap()
}

/// Checks that `split` of `text` has its ranges contiguous, none empty,
/// from the first record to the end, each but the first beginning just
/// after a line break, the lines and records before each counted: so
/// together they are the file's records, each once.
fn covers(text: &str, split: &Split, written: Written) {
    let bytes = text.as_bytes();
    let first = split.ranges.first().map_or(bytes.len(), |r| usize::try_from(r.start).unwrap());
    assert_eq!(&bytes[..first], &split.header[..], "the header comes first, then the first range");
    let mut at = first;
    for (range, counted) in split.ranges.iter().zip(befores(text, split, written)) {
        let (start, end) = (usize::try_from(range.start).unwrap(), usize::try_from(range.end).unwrap());
        assert_eq!(start, at, "contiguous: {split:?}");
        assert!(end > start, "none empty: {split:?}");
        assert!(start.checked_sub(1).is_none_or(|p| bytes[p] == b'\n'), "{start} begins a line: {split:?}");
        let before = &bytes[..start];
        let lines = before.iter().fold(0_usize, |n, b| n.saturating_add(usize::from(*b == b'\n')));
        assert_eq!(counted.lines, u64::try_from(lines).unwrap(), "lines before {start}");
        let mut inside = false;
        let ends = before
            .iter()
            .filter(|&&b| {
                inside ^= written.quoted && b == b'"';
                b == b'\n' && !inside
            })
            .count();
        assert_eq!(counted.records, u64::try_from(ends.saturating_sub(usize::from(written.header))).unwrap(), "records before {start}");
        at = end;
    }
    assert_eq!(at, bytes.len(), "to the end");
}

/// Verifies: [DESIGN] RULES.md D1148: a file is split into at most as
/// many ranges as jobs, contiguous and none empty, the first at the first
/// record, the others each just after a line break at or past an equal
/// share, the last to the end: together the file, each byte once, with
/// the lines and records before each counted. A file of fewer lines than
/// jobs has a range a line; one job has the file whole; a last line
/// without its line break is the last range's.
#[test]
fn ranges_begin_at_lines_and_cover_the_file_once() {
    let rows = (0..50_usize).fold(String::new(), |mut rows, n| {
        writeln!(rows, "{n},name {n},{}", "x".repeat(n % 7)).unwrap();
        rows
    });
    for text in [rows.as_str(), "1,a\n2,b\n3,c", "1\n", "", "1,a\r\n2,b\r\n3,c\r\n"] {
        for jobs in 1..=7 {
            let split = split_of(text, jobs, csv(false)).unwrap();
            assert!(split.ranges.len() <= jobs);
            covers(text, &split, csv(false));
        }
    }
    let whole = split_of(&rows, 1, csv(false)).unwrap();
    assert_eq!(whole.ranges, [Range { start: 0, end: u64::try_from(rows.len()).unwrap() }]);
    let four = split_of(&rows, 4, csv(false)).unwrap();
    assert_eq!(four.ranges.len(), 4, "a share each");
    let lines = split_of("1\n2\n3\n", 7, csv(false)).unwrap();
    assert_eq!(lines.ranges.iter().map(|r| (r.start, r.end)).collect::<Vec<_>>(), [(0, 2), (2, 4), (4, 6)]);
}

/// Verifies: [DESIGN] RULES.md D1148 [RFC 4180 §2 rule 3]: a header is
/// kept apart, its line break with it, even one holding a quoted line
/// break, and the first range begins after it; its lines are counted
/// before every range, and it is no record of data.
#[test]
fn a_header_is_kept_apart() {
    let text = "K,\"S\nT\"\n1,a\n2,b\n3,c\n4,d\n";
    for jobs in 1..=4 {
        let split = split_of(text, jobs, csv(true)).unwrap();
        assert_eq!(split.header, b"K,\"S\nT\"\n");
        assert_eq!((split.header_lines, split.ranges[0].start, befores(text, &split, csv(true))[0].records), (2, 8, 0));
        covers(text, &split, csv(true));
    }
    let only = split_of("K,S\n", 3, csv(true)).unwrap();
    assert!(only.ranges.is_empty(), "no records, no range");
}

/// Verifies: [DESIGN] RULES.md D1148 [RFC 4180 §2 rule 6]: a CSV range that
/// would begin within a quoted field, after a line break it holds, is
/// found by the quotes before it, odd in number, and refused saying to
/// import with one job; a quoted line break before
/// where a range begins is no such, and its record is counted once; of
/// tab-separated text a quote encloses nothing, so it splits.
#[test]
fn a_record_split_between_jobs_is_found() {
    let long = format!("1,\"{}\"\n2,b\n", "line\n".repeat(20));
    assert!(straddled(&long, &split_of(&long, 2, csv(false)).unwrap()));
    assert!(STRADDLES.ends_with("import it with --jobs 1"));
    let tabs = Written { quoted: false, header: false };
    let tabbed = split_of(&long, 2, tabs).unwrap();
    covers(&long, &tabbed, tabs);
    assert!(
        !straddles(&[2, 1]) && straddles(&[1, 1]) && straddles(&[3, 2, 1]) && !straddles(&[2, 7]),
        "the last range's quotes come before none"
    );
    let early = format!("1,\"a\nb\"\n{}", "2,ccccccccccccccccc\n".repeat(4));
    let after = split_of(&early, 2, csv(false)).unwrap();
    assert_eq!(after.ranges.len(), 2);
    assert!(!straddled(&early, &after));
    let counted = befores(&early, &after, csv(false))[1];
    assert_eq!((counted.lines, counted.records), (4, 3), "the quoted line break is no record's end");
}

/// Verifies: [DESIGN] RULES.md D1148: quotes are counted whole however
/// many a block holds, every byte a quote among them, and across reads.
#[test]
fn quotes_are_counted_whole() {
    for (len, every) in [(0_usize, 1_usize), (254, 1), (255, 1), (256, 1), (70_000, 1), (70_000, 3), (1_000, 1_001)] {
        let text: String = (0..len).map(|n| if n % every == 0 { '"' } else { 'x' }).collect();
        let expected = u64::try_from(len.div_ceil(every)).unwrap();
        assert_eq!(quotes_in(text.as_bytes()), expected, "{len} bytes, a quote every {every}");
        let whole = Range { start: 0, end: u64::try_from(len).unwrap() };
        assert_eq!(quotes(&mut Cursor::new(text.as_bytes()), whole).unwrap(), expected, "across reads");
    }
}
