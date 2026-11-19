//! The features CONFORMANCE.md claims: the rows of the table under its
//! heading "Features claimed", each a feature's or subfeature's identifier,
//! its name as the standard gives it, and a link to the section that says
//! how it is done, whose title becomes the row's COMMENTS.

use crate::error::{Error, Result};

/// The heading the table is under.
const HEADING: &str = "## Features claimed";
/// What begins the next heading, which ends the section.
const SECTION: char = '#';
/// What begins a row of a Markdown table, and separates its cells.
const CELL: char = '|';
/// The cells of a row of the table.
const CELLS: usize = 3;
/// The first cell of the table's heading row.
const FIRST_HEADING: &str = "Feature";
/// What COMMENTS says before the section's title.
const SEE: &str = "CONFORMANCE.md, ";

/// One feature or subfeature claimed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// Its identifier, as `T031` or `E011-01`.
    pub key: String,
    /// Its name.
    pub name: String,
    /// COMMENTS: the section of CONFORMANCE.md that says how it is done.
    pub comment: String,
}

/// The claims of CONFORMANCE.md's `text`.
///
/// # Errors
///
/// It has no heading `HEADING`, a row of the table under it has not
/// `CELLS` cells, or two rows claim the same feature.
pub fn read(text: &str) -> Result<Vec<Claim>> {
    let mut lines = text.lines().skip_while(|l| l.trim_end() != HEADING);
    if lines.next().is_none() {
        return Err(Error::new(format!("CONFORMANCE.md has no heading '{HEADING}'")));
    }
    let rows = lines.take_while(|l| !l.starts_with(SECTION)).filter(|l| l.starts_with(CELL));
    let mut claims: Vec<Claim> = Vec::new();
    for row in rows {
        let cells: Vec<&str> = row.trim().trim_matches(CELL).split(CELL).map(str::trim).collect();
        let [key, name, section] = cells.as_slice() else {
            return Err(Error::new(format!("CONFORMANCE.md: '{row}' has not {CELLS} cells")));
        };
        if *key == FIRST_HEADING || key.starts_with('-') {
            continue;
        }
        if claims.iter().any(|c| c.key == *key) {
            return Err(Error::new(format!("CONFORMANCE.md claims {key} twice")));
        }
        claims.push(Claim { key: (*key).to_owned(), name: (*name).to_owned(), comment: format!("{SEE}{}", title(section)) });
    }
    Ok(claims)
}

/// The title of a section `cell` links to, `[Truth values](#truth-values)`,
/// or the cell as written if it is no link.
fn title(cell: &str) -> &str {
    cell.strip_prefix('[').and_then(|rest| rest.split_once("](")).map_or(cell, |(title, _)| title)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [TOOL] R-TRACE-3 the table under the heading is read, its
    /// heading and rule rows passed over, a link's title made the comment;
    /// a table before the heading or after the next is not read.
    #[test]
    fn the_claimed_features_are_read() {
        let text = "# C\n\n| Feature | x | y |\n| T999 | Other | z |\n\n## Features claimed\n\nThese.\n\n| Feature | Name | Claimed in |\n|---|---|---|\n\
                    | T031 | BOOLEAN data type | [Truth values](#truth-values) |\n| E011-01 | INTEGER and SMALLINT data types | Core |\n\nAfter.\n\n### Next\n\n| T998 | No | no |\n";
        let claims = read(text).unwrap();
        assert_eq!(claims.len(), 2);
        assert_eq!(
            claims[0],
            Claim { key: "T031".to_owned(), name: "BOOLEAN data type".to_owned(), comment: "CONFORMANCE.md, Truth values".to_owned() }
        );
        assert_eq!(claims[1].key, "E011-01");
        assert_eq!(claims[1].comment, "CONFORMANCE.md, Core");
    }

    /// Verifies: [TOOL] R-TRACE-3 a document without the heading, a row of
    /// too few cells and a feature claimed twice are refused.
    #[test]
    fn a_malformed_table_is_refused() {
        assert!(read("# C\n| T031 | a | b |\n").is_err());
        assert!(read("## Features claimed\n\n| T031 | BOOLEAN data type |\n").is_err());
        assert!(read("## Features claimed\n\n| T031 | a | b |\n| T031 | a | c |\n").is_err());
        assert_eq!(read("## Features claimed\n\nNone yet.\n\n## Next\n\n| T031 | a | b |\n").unwrap(), Vec::new());
    }
}
