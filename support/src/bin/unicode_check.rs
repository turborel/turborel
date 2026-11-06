//! `cargo xtask unicode-check --from DIR`: checks the collation UNICODE
//! against `CollationTest_SHIFTED.txt` in DIR (RULES.md D60). The file's
//! lines are in order, so each must not sort before the last. The file is
//! not in the repository (D6), so this runs outside `verify`.

#![expect(clippy::print_stdout, clippy::print_stderr, reason = "a command-line tool reports by printing (RULES.md R-DEV-3)")]

use std::cmp::Ordering;
use std::path::Path;
use std::process::ExitCode;
use std::{env, fs};

use turborel::value::collation::{Collation, Pad, UNICODE, builtin};

/// The test file's name.
const FILE: &str = "CollationTest_SHIFTED.txt";
/// A field of code points holds hexadecimal.
const HEX: u32 = 16;
/// The most failures shown.
const SHOWN: usize = 20;

/// The string of a line, unless it holds a surrogate, which no `str` can.
fn string(line: &str) -> Option<String> {
    let codes = line.split(';').next()?;
    codes.split_whitespace().map(|c| u32::from_str_radix(c, HEX).ok().and_then(char::from_u32)).collect()
}

fn key(collation: &dyn Collation, text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    collation.sort_key(text, Pad::None, &mut out);
    out
}

/// The lines of `text` read, and those that sort before the line above.
/// Equal keys pass, since the file breaks their ties by a level this
/// collation has not [UTS #10 S3.10].
fn checked(collation: &dyn Collation, text: &str) -> (usize, usize) {
    let (mut lines, mut failures) = (0_usize, 0_usize);
    let mut last: Option<(Vec<u8>, String)> = None;
    for line in text.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let Some(string) = string(line) else { continue };
        let now = key(collation, &string);
        if let Some((before, was)) = &last
            && before.cmp(&now) == Ordering::Greater
        {
            failures = failures.saturating_add(1);
            if failures <= SHOWN {
                println!("out of order: {was:?} then {string:?} ({line})");
            }
        }
        lines = lines.saturating_add(1);
        last = Some((now, string));
    }
    (lines, failures)
}

fn main() -> ExitCode {
    let Some(dir) = env::args().nth(1) else {
        eprintln!("usage: unicode_check DIR");
        return ExitCode::FAILURE;
    };
    let path = Path::new(&dir).join(FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("cannot read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let collations = builtin();
    let Some(collation) = collations.iter().find(|c| c.name() == UNICODE) else {
        eprintln!("no collation {UNICODE}");
        return ExitCode::FAILURE;
    };
    match checked(collation.as_ref(), &text) {
        (lines, 0) => {
            println!("unicode_check: {lines} lines in order");
            ExitCode::SUCCESS
        }
        (lines, failures) => {
            eprintln!("unicode_check: {failures} of {lines} lines out of order");
            ExitCode::FAILURE
        }
    }
}
