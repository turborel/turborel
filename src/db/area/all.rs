//! The string GET DIAGNOSTICS ALL assigns [ISO/IEC 9075-2:2023 §23.1
//! GR6a] (Feature F123): each item `X = ` its value as a character
//! string, joined by semicolons, an item whose value is a zero-length
//! string left out.

use crate::error::{Code, Result, SqlState, fail_with};
use crate::value::{MAX_CHARACTER_LENGTH, Value};

/// [ISO/IEC 9075-2:2023 §23.1 GR6a] What separates two items'
/// presentations.
const SEPARATOR: char = ';';
/// [ISO/IEC 9075-2:2023 §23.1 GR6a] What joins an item's name to its
/// value in PRESENTATION ( X ).
const PRESENTED_AS: &str = " = ";

/// [ISO/IEC 9075-2:2023 §23.1 GR6a] S, of each item of `presented`, by
/// its name and value, in turn: SEMICOLONCAT ( S, X ).
///
/// # Errors
///
/// SQLSTATE 22001 if S is longer than the longest character string
/// varying (IL006), but for spaces [§6.32 GR2b ii 3 B].
pub(super) fn concatenated(presented: Vec<(&'static str, Value)>) -> Result<Value> {
    let mut s = String::new();
    for (name, value) in presented {
        let form = charform(&value);
        // CHARFORM ( X ) = '' compares as character strings compare,
        // the shorter padded with spaces [§8.2 GR3b], so a value of
        // spaces alone is left out too.
        if form.trim_end_matches(' ').is_empty() {
            continue;
        }
        if !s.is_empty() {
            s.push(SEPARATOR);
        }
        s.push_str(name);
        s.push_str(PRESENTED_AS);
        s.push_str(&form);
    }
    within_longest(s)
}

/// [ISO/IEC 9075-2:2023 §23.1 GR6a] CHARFORM ( X ): the item's value,
/// cast to a character string varying [§6.13 GR12]: a number as the
/// shortest literal of its value.
fn charform(value: &Value) -> String {
    match value {
        Value::Char(s) => s.clone(),
        Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Lob(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Json(_)
        | Value::Extension(_)
        | Value::Null => value.to_string(),
    }
}

/// [ISO/IEC 9075-2:2023 §6.32 GR2b ii 3 B] `s`, if no longer than the
/// longest character string varying; else its first characters, if
/// those after are all spaces.
fn within_longest(s: String) -> Result<Value> {
    let longest = usize::try_from(MAX_CHARACTER_LENGTH).unwrap_or(usize::MAX);
    let Some((cut, _)) = s.char_indices().nth(longest) else { return Ok(Value::Char(s)) };
    if s.get(cut..).is_some_and(|past| past.chars().any(|c| c != ' ')) {
        let msg =
            format!("[ISO/IEC 9075-2:2023 §6.32 GR2b ii 3 B] ALL's string of {} characters is longer than {longest}", s.chars().count());
        return fail_with(Code::Data, SqlState::STRING_DATA_RIGHT_TRUNCATION, msg);
    }
    Ok(Value::Char(s.get(..cut).unwrap_or_default().to_owned()))
}

// Unit tests: the presentations joined, values of no characters left
// out, and the longest string (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
