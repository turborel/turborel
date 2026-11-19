//! \[DESIGN\] `\h` (RULES.md D219): a statement's syntax, from the pages
//! of `doc/sql/statements`, compiled into the client so it is the
//! syntax of the release it came with. `\h` alone lists the statements;
//! `\h NAME` writes the page's one-line summary and its synopsis. A name
//! is matched in upper case, its spaces as one; a name no page has
//! exactly lists the statements it begins, or writes the one page it
//! begins.

mod pages;

use pages::PAGES;

/// The statement a page's file name names: `alter-table.md` is ALTER
/// TABLE.
fn statement(file: &str) -> String {
    file.trim_end_matches(".md").replace('-', " ").to_uppercase()
}

/// The summary and synopsis of the page `text`, of the statement `name`.
fn synopsis(name: &str, text: &str) -> String {
    let mut lines = text.lines();
    let summary = lines.by_ref().skip_while(|l| *l != "## NAME").nth(2).unwrap_or(name).to_owned();
    let body: Vec<&str> = lines
        .skip_while(|l| *l != "## SYNOPSIS")
        .skip_while(|l| !l.starts_with("```"))
        .skip(1)
        .take_while(|l| !l.starts_with("```"))
        .collect();
    format!("{summary}\n\n{}\n\nSee doc/sql/statements/{}.md.", body.join("\n"), name.to_lowercase().replace(' ', "-"))
}

/// What `\h` writes of `topic`.
///
/// # Errors
///
/// No statement is or begins with it.
pub fn help(topic: Option<&str>) -> Result<String, String> {
    let names: Vec<(String, &str)> = PAGES.iter().map(|(file, text)| (statement(file), *text)).collect();
    let Some(asked) = topic.map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ").to_uppercase()).filter(|t| !t.is_empty()) else {
        let all: Vec<&str> = names.iter().map(|(n, _)| n.as_str()).collect();
        return Ok(format!("Statements; \\h NAME writes one's syntax:\n\n  {}", all.join("\n  ")));
    };
    if let Some((name, text)) = names.iter().find(|(n, _)| *n == asked) {
        return Ok(synopsis(name, text));
    }
    let begun: Vec<&(String, &str)> = names.iter().filter(|(n, _)| n.starts_with(&format!("{asked} "))).collect();
    match begun.as_slice() {
        [] => Err(format!("\\h: no statement is named {asked}; \\h lists them")),
        [(name, text)] => Ok(synopsis(name, text)),
        many => {
            let listed: Vec<&str> = many.iter().map(|(n, _)| n.as_str()).collect();
            Ok(format!("Statements of {asked}:\n\n  {}", listed.join("\n  ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D219: the pages compiled in are every
    /// page of `doc/sql/statements`.
    #[test]
    fn every_statement_page_is_compiled_in() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../doc/sql/statements");
        let mut files: Vec<String> =
            std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        files.retain(|f| std::path::Path::new(f).extension().is_some_and(|e| e.eq_ignore_ascii_case("md")) && f != "README.md");
        files.sort();
        let compiled: Vec<&str> = PAGES.iter().map(|(f, _)| *f).collect();
        assert_eq!(compiled, files);
    }

    /// Verifies: [DESIGN] RULES.md D219: a statement's help is its summary
    /// and synopsis, its name matched in any case and spacing; a name that
    /// begins several lists them; none is refused; nothing lists all.
    #[test]
    fn help_writes_a_statements_syntax() {
        let table = help(Some("alter   table")).unwrap();
        assert!(table.starts_with("ALTER TABLE: change a base table\n\n<alter table statement> ::="), "{table}");
        assert!(table.ends_with("See doc/sql/statements/alter-table.md."));
        assert!(help(Some("ALTER")).unwrap().starts_with("Statements of ALTER:\n\n  ALTER DATABASE"));
        assert!(help(Some("nothing such")).is_err());
        assert!(help(None).unwrap().contains("\n  SELECT\n"));
        assert_eq!(help(Some(" ")).unwrap(), help(None).unwrap());
    }
}
