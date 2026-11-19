//! \[DESIGN\] The files connection settings come from (RULES.md D203): a
//! service file of named sections of settings, and a password file of
//! passwords by server and user.
//!
//! A service file holds sections, each `[<name>]` on a line of its own,
//! then lines `<key> = <value>`, each key a setting's name but `service`
//! and `service_file`. A line beginning with `#` is a comment; blank
//! lines are skipped. A password file holds lines
//! `<host>:<port>:<user>:<password>`, a field `*` matching anything, `\`
//! taking the character after it as it is, so `\:` is a colon and `\\` a
//! backslash; the first line that matches is taken, and a line of fewer
//! fields is skipped.
//!
//! A password file should be readable by its owner alone. The standard
//! library gives no portable way to read a file's owner or its
//! permissions, so the client cannot check that it is: it checks only
//! that the file is a regular file, and its owner answers for the rest.

use std::path::Path;

use crate::connection::Result;

use super::invalid;
use super::options::{Options, SERVICE, SERVICE_FILE};

/// What begins a comment line, in either file.
const COMMENT: char = '#';
/// What matches any value in a password file.
const ANY: &str = "*";
/// What escapes the character after it in a password file.
const ESCAPE: char = '\\';
/// What separates a password file's fields.
const SEPARATOR: char = ':';
/// The fields of a password file's line.
const FIELDS: usize = 4;

/// The settings of the section `name` of the service file `text`, read
/// from `file`.
///
/// # Errors
///
/// SQLSTATE 08001 if a line is malformed or sets no setting, or there is
/// no section `name`.
pub(super) fn service(text: &str, name: &str, file: &Path) -> Result<Options> {
    let at = |n: usize, why: &str| invalid(&format!("[DESIGN] {} line {n}: {why}", file.display()));
    let mut found: Option<Options> = None;
    let mut current: Option<&str> = None;
    for (n, line) in text.lines().enumerate().map(|(i, l)| (i.saturating_add(1), l.trim())) {
        if line.is_empty() || line.starts_with(COMMENT) {
            continue;
        }
        if let Some(section) = line.strip_prefix('[') {
            let section = section.strip_suffix(']').ok_or_else(|| at(n, "a section's name ends with ]"))?;
            current = Some(section.trim());
            continue;
        }
        let section = current.ok_or_else(|| at(n, "a setting comes before any section"))?;
        let (key, value) = line.split_once('=').ok_or_else(|| at(n, "a setting is <key> = <value>"))?;
        let key = key.trim();
        if key == SERVICE || key == SERVICE_FILE {
            return Err(at(n, &format!("a service cannot set {key}")));
        }
        let mut options = Options::new();
        options.set(key, value.trim()).map_err(|e| at(n, &e.message))?;
        if section == name {
            found = Some(options.over(&found.unwrap_or_default()));
        }
    }
    found.ok_or_else(|| invalid(&format!("[DESIGN] {} has no service {name}", file.display())))
}

/// The fields of the password file's line `line`, unescaped: the first
/// three split at each colon not escaped, the fourth the rest.
fn fields(line: &str) -> Vec<(String, bool)> {
    let mut fields = Vec::with_capacity(FIELDS);
    let (mut field, mut escaped, mut plain) = (String::new(), false, true);
    for c in line.chars() {
        if escaped {
            field.push(c);
            (escaped, plain) = (false, false);
        } else if c == ESCAPE {
            escaped = true;
        } else if c == SEPARATOR && fields.len() < FIELDS.saturating_sub(1) {
            fields.push((std::mem::take(&mut field), plain));
            plain = true;
        } else {
            field.push(c);
        }
    }
    fields.push((field, plain));
    fields
}

/// Whether the field `(value, plain)` matches `wanted`: a `*` written
/// plain matches anything.
fn matches(field: &(String, bool), wanted: &str) -> bool {
    let (value, plain) = field;
    (*plain && value == ANY) || value == wanted
}

/// The password the password file `text` gives `user` at `host` and
/// `port`: that of its first line matching all three.
pub(super) fn password(text: &str, (host, port, user): (&str, &str, &str)) -> Option<String> {
    text.lines().filter(|l| !l.trim_start().starts_with(COMMENT)).map(fields).find_map(|f| match f.as_slice() {
        [h, p, u, (password, _)] => (matches(h, host) && matches(p, port) && matches(u, user)).then(|| password.clone()),
        _ => None,
    })
}

/// The text of the file at `path`, if it can be read and is a regular
/// file (a link followed to one); none otherwise.
pub(super) fn read_regular(path: &Path) -> Option<String> {
    let regular = std::fs::metadata(path).is_ok_and(|m| m.is_file());
    regular.then(|| std::fs::read_to_string(path).ok()).flatten()
}
