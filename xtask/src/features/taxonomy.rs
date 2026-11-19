//! The parts, features and subfeatures of ISO/IEC 9075, read from the
//! standards' texts in a directory: the parts ISO/IEC 9075-2:2023 names
//! as the editions it is to be used with, and the rows of the feature
//! taxonomy tables of ISO/IEC 9075-2:2023 (Annexes D and H) and of the
//! 2006 drafts of ISO/IEC 9075-11 (Annex E), 9075-4 and 9075-14. None of
//! these texts defines a package, so there are none.
//!
//! The texts are laid out as printed, so a table's row is a line of its
//! counter, the feature's identifier and the start of its name; the rest
//! of a long name is on the lines below, at the same column, before any
//! blank line; a table of three columns has the description to the right,
//! two spaces or a dash away. The counters run from 1 without a gap, which
//! is checked, so no row is passed over.

use std::collections::BTreeMap;
use std::path::Path;

use super::{Element, SUB_MARK, feature, part};
use crate::error::{Error, Result};
use crate::unicode::read;

/// ISO/IEC 9075-2:2023, which names the parts and the newest features.
const PART_2: &str = "iso-iec-9075-2-2023.exact.txt";
/// The texts and the titles of their feature taxonomy tables, ISO/IEC
/// 9075-2:2023's first, so a feature two of them list is named as the
/// newest names it.
const TABLES: [(&str, &[&str]); 4] = [
    (PART_2, &["Table D.1 — Feature taxonomy for optional features", "Table H.1 — Feature taxonomy and definition for mandatory features"]),
    (
        "5CD2-11-Schemata-2006-01.exact.txt",
        &["Table 2 — Feature taxonomy and definition for mandatory features", "Table 3 — Feature taxonomy for optional features"],
    ),
    ("5CD2-04-PSM-2006-01.exact.txt", &["Table 5 — Feature taxonomy for optional features"]),
    ("5CD2-14-XML-2006-01.exact.txt", &["Table 16 — Feature taxonomy for optional features"]),
];
/// What begins a line that ends a table, at its start once trimmed.
const ENDINGS: [&str; 4] = ["Table ", "Annex ", "Bibliography", "Index"];
/// What begins a heading line repeated on each page of a table.
const PAGE_HEADINGS: [&str; 2] = ["Feature", "ID"];
/// What separates a name from a description to its right.
const GAPS: [&str; 2] = ["  ", " — "];
/// The compounds a name breaks after their hyphen, which is kept: any
/// other hyphen ending a line breaks a word, and goes.
const COMPOUNDS: [&str; 1] = ["variable-length"];
/// How far from its row's name a line's text may start and still continue it.
const NEAR: usize = 2;
/// The digits of a feature's identifier after its letter.
const ID_DIGITS: usize = 3;
/// The digits of a subfeature's identifier after its feature's.
const SUB_DIGITS: usize = 2;
/// How the parts' list names each, `— ISO/IEC 9075-1, sixth edition;`,
/// and how a reference names a part's title: `SQL — Part 1: Framework
/// (SQL/Framework)`.
const EDITION_ITEM: &str = "— ISO/IEC 9075-";
const EDITION: &str = " edition";
const PART_TITLE: &str = "SQL — Part ";
/// The part whose text this is, which its list leaves out.
const THIS_PART: &str = "2";

/// The parts, features and subfeatures of the texts in `from`.
pub fn read_all(from: &Path) -> Result<Vec<Element>> {
    let part_2 = read(from, PART_2)?;
    let mut elements: Vec<Element> = parts(&part_2)?.iter().map(|(n, name)| part(n, name)).collect();
    let mut named: BTreeMap<String, String> = BTreeMap::new();
    for (file, titles) in TABLES {
        let text = if file == PART_2 { part_2.clone() } else { read(from, file)? };
        for title in titles {
            for (id, name) in table(&text, title)? {
                named.entry(id).or_insert(name);
            }
        }
    }
    for (id, name) in &named {
        let parent = match id.split_once(SUB_MARK) {
            Some((of, _)) => Some(named.get(of).ok_or_else(|| Error::new(format!("subfeature {id} has no feature {of}")))?.as_str()),
            None => None,
        };
        elements.push(feature(id, name, parent));
    }
    Ok(elements)
}

/// A row of a table being read: its counter, identifier, name so far, the
/// column its name starts at, and the column its description starts at in
/// a table of three columns.
struct Row {
    counter: usize,
    id: String,
    name: String,
    column: usize,
    description: Option<usize>,
}

/// The identifiers and names of the rows of the table titled `title` in
/// `text`.
///
/// # Errors
///
/// There is no such table, or a row's counter does not follow the last.
fn table(text: &str, title: &str) -> Result<Vec<(String, String)>> {
    let mut lines = text.lines().skip_while(|l| l.trim() != title);
    if lines.next().is_none() {
        return Err(Error::new(format!("no '{title}'")));
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut open = false;
    for line in lines {
        if let Some(row) = row(line) {
            let wanted = rows.len().saturating_add(1);
            if row.counter != wanted {
                return Err(Error::new(format!("'{title}': row {} ({}) where {wanted} was wanted", row.counter, row.id)));
            }
            rows.push(row);
            open = true;
        } else if ends(line) {
            break;
        } else if line.trim().is_empty() {
            open = false;
        } else if let Some(last) = rows.last_mut().filter(|_| open) {
            continued(last, line);
        }
    }
    Ok(rows.into_iter().map(|r| (r.id, r.name)).collect())
}

/// The row `line` begins, if it is one: a counter, an identifier and a name.
fn row(line: &str) -> Option<Row> {
    let (counter, rest) = line.trim_start().split_once(' ')?;
    let (id, rest) = rest.trim_start().split_once(' ')?;
    let counter: usize = counter.parse().ok()?;
    if !is_feature_id(id) {
        return None;
    }
    let text = rest.trim_start();
    let column = line.chars().count().saturating_sub(text.chars().count());
    let name = first_cell(text);
    let after = text.get(name.len()..).unwrap_or_default().trim_start();
    let description = (!after.is_empty()).then(|| line.chars().count().saturating_sub(after.chars().count()));
    Some(Row { counter, id: id.to_owned(), name: name.to_owned(), column, description })
}

/// Whether `id` is a feature's identifier, `T031`, or a subfeature's,
/// `E011-01`.
fn is_feature_id(id: &str) -> bool {
    let digits = |s: &str, n: usize| s.len() == n && s.chars().all(|c| c.is_ascii_digit());
    let (feature, sub) = id.split_once(SUB_MARK).map_or((id, None), |(f, s)| (f, Some(s)));
    let mut chars = feature.chars();
    let letter = chars.next().is_some_and(|c| c.is_ascii_uppercase());
    letter && digits(chars.as_str(), ID_DIGITS) && sub.is_none_or(|s| digits(s, SUB_DIGITS))
}

/// Whether `line` ends a table: a heading, or text at the margin that is
/// no row.
fn ends(line: &str) -> bool {
    let trimmed = line.trim();
    let at_margin = !trimmed.is_empty() && !line.starts_with(' ');
    at_margin || ENDINGS.iter().any(|e| trimmed.starts_with(e))
}

/// Adds to `row`'s name what `line` holds of it: its first cell, if it
/// starts where the name does and is no page's heading.
fn continued(row: &mut Row, line: &str) {
    let text = line.trim_start();
    let column = line.chars().count().saturating_sub(text.chars().count());
    if column.abs_diff(row.column) > NEAR || PAGE_HEADINGS.iter().any(|h| text.starts_with(h)) {
        return;
    }
    let cut = row.description.map_or(text, |d| before(text, d.saturating_sub(column)));
    let more = first_cell(cut);
    let hyphenated = format!("{}{more}", row.name);
    let compound = COMPOUNDS.iter().any(|c| hyphenated.contains(c));
    row.name = match row.name.strip_suffix('-') {
        // A word broken at the line's end, as `func-` `tion`, but a
        // compound broken after its hyphen.
        Some(start) => {
            if compound {
                hyphenated
            } else {
                format!("{start}{more}")
            }
        }
        None => format!("{} {more}", row.name),
    };
}

/// `text` before its `at`th character, where a description to the right
/// of a name starts. Text is justified, so a line of both may hold a word
/// across that column with one space each side: the name ends at the
/// nearer of the word's ends, its end if neither is nearer.
fn before(text: &str, at: usize) -> &str {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let in_word = |k: usize| chars.get(k).is_some_and(|(_, c)| *c != ' ');
    let mut start = at;
    while start > 0 && in_word(start) && in_word(start.saturating_sub(1)) {
        start = start.saturating_sub(1);
    }
    let mut end = at;
    while start != at && in_word(end) {
        end = end.saturating_add(1);
    }
    let cut = if at.saturating_sub(start) < end.saturating_sub(at) { start } else { end };
    chars.get(cut).map_or(text, |(byte, _)| text.get(..*byte).unwrap_or(text))
}

/// `text` up to the first gap before a cell to its right.
fn first_cell(text: &str) -> &str {
    let end = GAPS.iter().filter_map(|g| text.find(g)).min();
    end.and_then(|at| text.get(..at)).unwrap_or(text).trim_end()
}

/// The parts ISO/IEC 9075-2:2023's `text` names, its own among them, each
/// with its number and title, in order of number.
///
/// # Errors
///
/// A part named has no title in the text.
fn parts(text: &str) -> Result<Vec<(String, String)>> {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let listed = flat.split(EDITION_ITEM).skip(1).filter_map(|item| {
        let (number, rest) = item.split_once(", ")?;
        // The list's last item ends with a full stop, the others with a semicolon.
        let edition = rest.split_once([';', '.']).is_some_and(|(e, _)| e.ends_with(EDITION));
        (edition && number.chars().all(|c| c.is_ascii_digit())).then_some(number)
    });
    let mut numbers: Vec<u32> = std::iter::once(THIS_PART).chain(listed).filter_map(|n| n.parse().ok()).collect();
    numbers.sort_unstable();
    numbers.dedup();
    numbers
        .into_iter()
        .map(|n| {
            let marker = format!("{PART_TITLE}{n}: ");
            let title = flat
                .split_once(&marker)
                .and_then(|(_, after)| after.split_once(')'))
                .map(|(title, _)| format!("{title})"))
                .ok_or_else(|| Error::new(format!("{PART_2} names part {n} without its title")))?;
            Ok((n.to_string(), title))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_COLUMNS: &str = "Table D.1, “Feature taxonomy”, is a table.

                        Table D.1 — Feature taxonomy for optional features

          Feature ID                                     Feature Name

  1      B011            Embedded Ada

             Feature ID                                       Feature Name

   2       T050                 POSITION, CHAR_LENGTH, OCTET_LENGTH, LOWER, TRIM, UPPER, and
                                SUBSTRING for CLOBs

 3     T863          SQL/JSON simplified accessor: single-quoted string literal as member
                     accessor
4   X059      Advanced table mapping: hex encoding

                                                 Annex E
  5      B012            Embedded C
";

    const THREE_COLUMNS: &str = "                   Table H.1 — Feature taxonomy and definition for mandatory features

           Feature ID           Feature Name                 Feature Description

   1       E011                 Numeric data types           Subclause 6.1, “<data type>”, <numeric type>,
                                                             including numeric expressions

       Feature ID    Feature Name                 Feature Description

  2    E011-03       DECIMAL and NUMERIC data — Subclause 5.2, “<token> and <separator>”:
                     types                    The <reserved word>s DEC, DECIMAL, and
                                              NUMERIC

   3       E021-04              CHARACTER_LENGTH func-          — Subclause 6.31, “<numeric value function>”:
                                tion                            The <char length expression>
                                           Bibliography
";

    /// Verifies: [TOOL] R-TRACE-3 a taxonomy of two columns is read: a
    /// name continued on the lines below at its column, a row at the
    /// margin, the page's headings passed over, and the table ended by the
    /// next Annex.
    #[test]
    fn a_table_of_two_columns_is_read() {
        let rows = table(TWO_COLUMNS, "Table D.1 — Feature taxonomy for optional features").unwrap();
        let names: Vec<(&str, &str)> = rows.iter().map(|(i, n)| (i.as_str(), n.as_str())).collect();
        assert_eq!(
            names,
            [
                ("B011", "Embedded Ada"),
                ("T050", "POSITION, CHAR_LENGTH, OCTET_LENGTH, LOWER, TRIM, UPPER, and SUBSTRING for CLOBs"),
                ("T863", "SQL/JSON simplified accessor: single-quoted string literal as member accessor"),
                ("X059", "Advanced table mapping: hex encoding"),
            ]
        );
    }

    /// Verifies: [TOOL] R-TRACE-3 a taxonomy of three columns is read: a
    /// name ends at two spaces or a dash before its description, goes on
    /// at its column below, a word broken at a line's end is joined, and
    /// the description's lines are passed over.
    #[test]
    fn a_table_of_three_columns_is_read() {
        let rows = table(THREE_COLUMNS, "Table H.1 — Feature taxonomy and definition for mandatory features").unwrap();
        let names: Vec<&str> = rows.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(names, ["Numeric data types", "DECIMAL and NUMERIC data types", "CHARACTER_LENGTH function"]);
    }

    /// Verifies: [TOOL] R-TRACE-3 a missing table, and a counter that skips
    /// a row, are refused, so no row is lost unseen.
    #[test]
    fn a_gap_in_the_counters_is_refused() {
        assert!(table(TWO_COLUMNS, "Table 9 — Nothing").is_err());
        let gap = TWO_COLUMNS.replace("   2       T050", "   7       T050");
        let error = table(&gap, "Table D.1 — Feature taxonomy for optional features").unwrap_err().to_string();
        assert!(error.contains("row 7 (T050)"), "{error}");
    }

    /// Verifies: [TOOL] R-TRACE-3 identifiers of a feature and a subfeature
    /// are told from other words.
    #[test]
    fn feature_identifiers_are_recognized() {
        for id in ["T031", "E011-01", "X400"] {
            assert!(is_feature_id(id), "{id}");
        }
        for id in ["t031", "T31", "T0311", "E011-1", "E011-001", "Table", "1"] {
            assert!(!is_feature_id(id), "{id}");
        }
    }

    /// Verifies: [TOOL] R-TRACE-3 the parts are the text's own and those
    /// its list of editions names, each titled as a reference names it,
    /// wrapped lines joined; a part without a title is refused.
    #[test]
    fn the_parts_are_read() {
        let text = "Information technology — Database\nlanguages SQL —\nPart 2:\nFoundation (SQL/Foundation)\n\
                    other parts:\n— ISO/IEC 9075-1, sixth edition;\n— ISO/IEC 9075-14, sixth edition;\n\
                    ISO/IEC 9075-1, Information technology — Database languages — SQL — Part 1: Framework\n(SQL/Framework)\n\
                    [10] ISO/IEC 9075-14, Information technology — Database languages — SQL — Part 14: XML-Related\nSpecifications (SQL/XML)\n";
        let parts = parts(text).unwrap();
        let expected =
            [("1", "Framework (SQL/Framework)"), ("2", "Foundation (SQL/Foundation)"), ("14", "XML-Related Specifications (SQL/XML)")];
        assert_eq!(parts, expected.map(|(n, t)| (n.to_owned(), t.to_owned())));
        assert!(parts_of_missing_title().is_err());
    }

    fn parts_of_missing_title() -> Result<Vec<(String, String)>> {
        parts("SQL — Part 2: Foundation (SQL/Foundation)\n— ISO/IEC 9075-3, sixth edition;\n")
    }
}
