//! \[EXTENSION\] A package's license (RULES.md D265 to D268): the SPDX
//! license expression its manifest's LICENSE writes, checked by the
//! expression grammar of the SPDX specification alone, and the file of
//! its text LICENSE FILE names, an entry of the package. A license is
//! told, never enforced, so a package without one installs.
//!
//! The grammar, its operators in upper case as the specification asks:
//!
//! ```text
//! license-expression  := or-expression
//! or-expression       := and-expression [ "OR" and-expression ]...
//! and-expression      := with-expression [ "AND" with-expression ]...
//! with-expression     := primary [ "WITH" idstring ]
//! primary             := simple-expression | "(" or-expression ")"
//! simple-expression   := idstring [ "+" ] | [ "DocumentRef-" idstring ":" ] "LicenseRef-" idstring
//! idstring            := ( ALPHA | DIGIT | "-" | "." )...
//! ```
//!
//! No list of license identifiers is kept, as it changes with each
//! release of the specification; any idstring is taken as one.

use std::path::Path;

use crate::error::Result;
use crate::plugin::MANIFEST_FILE;
use crate::plugin::errors::invalid_manifest;
use crate::plugin::manifest::Manifest;

use super::archive::{place, read_error};

/// The operator of a disjunction of licenses.
pub const OR: &str = "OR";
/// The operator of a conjunction of licenses.
pub const AND: &str = "AND";
/// The operator of a license with an exception.
pub const WITH: &str = "WITH";
/// What begins a reference to a license of no identifier.
pub const LICENSE_REF: &str = "LicenseRef-";
/// What begins the document a reference is of.
pub const DOCUMENT_REF: &str = "DocumentRef-";
/// The deepest parentheses an expression may nest (RULES.md R-CF-6).
pub const MAX_DEPTH: usize = 32;

/// What a package declares of its license.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct License {
    /// The SPDX expression of LICENSE, if written.
    pub expression: Option<String>,
    /// The path of LICENSE FILE in the package, if written.
    pub file: Option<String>,
    /// The text of that file, if it is there.
    pub text: Option<String>,
}

impl License {
    /// What `manifest` declares, no text read.
    pub fn of(manifest: &Manifest) -> License {
        License { expression: manifest.license.clone(), file: manifest.license_file.clone(), text: None }
    }

    /// Whether it declares anything.
    pub fn declared(&self) -> bool {
        self.expression.is_some() || self.file.is_some()
    }

    /// One line telling it: the expression and the file, or that none is
    /// declared.
    pub fn told(&self) -> String {
        match (&self.expression, &self.file) {
            (Some(e), Some(f)) => format!("license {e} (text in {f})"),
            (Some(e), None) => format!("license {e}"),
            (None, Some(f)) => format!("license in {f}"),
            (None, None) => "no license declared".to_owned(),
        }
    }
}

/// The license of the package in `dir`, a package installed or a
/// plug-in's directory: what its manifest declares, and the text of its
/// LICENSE FILE. A host that declares plug-ins in its configuration
/// shows their licenses by it, as `turborel extension license` does.
///
/// # Errors
///
/// SQLSTATE 38T05 if its manifest cannot be read; the manifest's errors;
/// SQLCODE -910 of a LICENSE FILE that cannot be read.
pub fn package_license(dir: &Path) -> Result<License> {
    let file = dir.join(MANIFEST_FILE);
    let text = std::fs::read_to_string(&file)
        .map_err(|e| crate::plugin::errors::unavailable(&format!("{} cannot be read: {e}", file.display())))?;
    let mut license = License::of(&Manifest::parse(&text)?);
    if let Some(path) = &license.file {
        let at = place(dir, path);
        license.text = Some(std::fs::read_to_string(&at).map_err(|e| read_error(&at, &e))?);
    }
    Ok(license)
}

/// A token of an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// `(`.
    Open,
    /// `)`.
    Close,
    /// A word: an operator, an idstring, a reference, with any `+`.
    Word(String),
}

/// The tokens of `text`.
fn tokens(text: &str) -> Vec<Token> {
    let spaced = text.replace('(', " ( ").replace(')', " ) ");
    spaced
        .split_whitespace()
        .map(|w| match w {
            "(" => Token::Open,
            ")" => Token::Close,
            other => Token::Word(other.to_owned()),
        })
        .collect()
}

/// Whether `s` is an idstring: letters, digits, `-` and `.`, at least one.
fn idstring(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

/// Whether `word` is a simple expression.
fn simple(word: &str) -> bool {
    let reference = word.split_once(':').map_or(Some(word), |(document, rest)| {
        document.strip_prefix(DOCUMENT_REF).filter(|d| idstring(d)).map(|_| rest)
    });
    if let Some(r) = reference.and_then(|r| r.strip_prefix(LICENSE_REF)) {
        return idstring(r);
    }
    let id = word.strip_suffix('+').unwrap_or(word);
    !word.contains(':') && idstring(id) && ![OR, AND, WITH].contains(&id)
}

/// Reads an expression, token by token.
struct Reader {
    tokens: Vec<Token>,
    at: usize,
}

impl Reader {
    /// The next token, if any.
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    /// Whether the word `word` is next, taking it if it is.
    fn accept(&mut self, word: &str) -> bool {
        let next = self.peek() == Some(&Token::Word(word.to_owned()));
        if next {
            self.at = self.at.saturating_add(1);
        }
        next
    }

    /// `operator`-separated operands, each read by `operand`.
    fn joined(&mut self, operator: &str, depth: usize, operand: fn(&mut Reader, usize) -> Result<()>) -> Result<()> {
        operand(self, depth)?;
        while self.accept(operator) {
            operand(self, depth)?;
        }
        Ok(())
    }

    /// `or-expression`.
    fn or(&mut self, depth: usize) -> Result<()> {
        self.joined(OR, depth, |r, d| r.joined(AND, d, Reader::with))
    }

    /// `with-expression`.
    fn with(&mut self, depth: usize) -> Result<()> {
        self.primary(depth)?;
        if !self.accept(WITH) {
            return Ok(());
        }
        let exception = self.word().unwrap_or_default();
        if idstring(&exception) { Ok(()) } else { Err(bad("WITH must be followed by an exception's identifier")) }
    }

    /// The next token if it is a word, taken.
    fn word(&mut self) -> Option<String> {
        let Some(Token::Word(w)) = self.peek().cloned() else { return None };
        self.at = self.at.saturating_add(1);
        Some(w)
    }

    /// `primary`.
    fn primary(&mut self, depth: usize) -> Result<()> {
        if self.peek() == Some(&Token::Open) {
            if depth >= MAX_DEPTH {
                return Err(bad(&format!("its parentheses nest deeper than {MAX_DEPTH}")));
            }
            self.at = self.at.saturating_add(1);
            self.or(depth.saturating_add(1))?;
            if self.peek() != Some(&Token::Close) {
                return Err(bad("a parenthesis is not closed"));
            }
            self.at = self.at.saturating_add(1);
            return Ok(());
        }
        let w = self.word().ok_or_else(|| bad("a license is missing"))?;
        if simple(&w) {
            return Ok(());
        }
        Err(bad(&format!("{w} is no license identifier or LicenseRef- reference")))
    }
}

/// The error of an expression that breaks the grammar, `why`.
fn bad(why: &str) -> crate::error::SqlError {
    invalid_manifest(&format!("LICENSE is not an SPDX license expression: {why}"))
}

/// That `text` is an SPDX license expression.
///
/// # Errors
///
/// SQLSTATE 38T06 if it is not, naming why.
pub fn check_expression(text: &str) -> Result<()> {
    let mut r = Reader { tokens: tokens(text), at: 0 };
    r.or(0)?;
    match r.peek() {
        None => Ok(()),
        Some(Token::Close) => Err(bad("a parenthesis closes none opened")),
        Some(Token::Open | Token::Word(_)) => Err(bad("licenses must be joined by AND, OR or WITH, in upper case")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] RULES.md D265: expressions of the SPDX
    /// grammar are taken, identifiers with `+`, LicenseRef- and
    /// DocumentRef- references, WITH, AND over OR and parentheses; and
    /// each break of it refused with SQLSTATE 38T06: an operator in
    /// lower case, two licenses not joined, an operator with no operand, a
    /// parenthesis not closed or closed twice, WITH of no exception, a
    /// LicenseRef- of no idstring, and nesting past the bound.
    #[test]
    fn license_expressions_follow_the_spdx_grammar() {
        let taken = [
            "MIT",
            "GPL-2.0+",
            "Apache-2.0 OR MIT",
            "MIT AND (LGPL-2.1-or-later OR BSD-3-Clause)",
            "GPL-2.0-or-later WITH Classpath-exception-2.0",
            "LicenseRef-my.license",
            "DocumentRef-spdx-tool-1.2:LicenseRef-MIT-Style-2",
            "((MIT))",
        ];
        for t in taken {
            check_expression(t).unwrap_or_else(|e| panic!("{t}: {}", e.msg));
        }
        let deep = format!("{}MIT{}", "(".repeat(MAX_DEPTH + 1), ")".repeat(MAX_DEPTH + 1));
        let refused =
            ["", "MIT or Apache-2.0", "MIT Apache-2.0", "MIT AND", "OR MIT", "(MIT", "MIT)", "MIT WITH", "LicenseRef-", "a:b", deep.as_str()];
        for t in refused {
            assert_eq!(check_expression(t).unwrap_err().sqlstate(), crate::plugin::errors::INVALID_MANIFEST, "{t}");
        }
        check_expression(&format!("{}MIT{}", "(".repeat(MAX_DEPTH), ")".repeat(MAX_DEPTH))).unwrap();
    }
}
