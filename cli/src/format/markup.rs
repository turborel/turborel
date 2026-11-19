//! \[DESIGN\] HTML and JSON output (RULES.md D213). HTML is a table, a
//! header row of the columns' names and a row for each row, `&`, `<`, `>`
//! and `"` written as references; expanded, a pair of cells for each
//! column. JSON (RFC 8259) is an array of an object for each row, a
//! column's name its member's; a number as a number, a truth value as
//! `true` or `false`, a null as `null`, a JSON value as itself, anything
//! else as a string; expanded or not alike.

use turborel::storage::Row;
use turborel::value::Value;

use super::{Printer, Style};

/// `text` as HTML text, its markup characters as references.
pub(crate) fn html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `text` as a JSON string [RFC 8259 §7].
pub(crate) fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => {
                if other.is_control() {
                    out.extend(format!("\\u{:04x}", u32::from(other)).chars());
                } else {
                    out.push(other);
                }
            }
        }
    }
    out.push('"');
    out
}

/// Whether `text` is a JSON number [RFC 8259 §6]: an optional minus, an
/// integer part without leading zeros, an optional fraction and exponent.
fn is_json_number(text: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let (mantissa, exponent) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
    let (int, fraction) = mantissa.split_once('.').unwrap_or((mantissa, "0"));
    let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
    let no_leading_zero = int == "0" || !int.starts_with('0');
    digits(int) && no_leading_zero && digits(fraction) && digits(exponent)
}

/// `v` as a JSON value, its text `text`.
fn json_value(v: &Value, text: &str) -> String {
    match v {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Json(_) => text.to_owned(),
        Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_) => {
            if is_json_number(text) {
                text.to_owned()
            } else {
                json_string(text)
            }
        }
        Value::Char(_)
        | Value::Binary(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Lob(_)
        | Value::Extension(_)
        | Value::Xml(_) => json_string(text),
    }
}

/// The alignment of the column at `at`, as an HTML attribute's value.
fn align(p: &Printer<'_>, at: usize) -> &'static str {
    if p.shapes.get(at).is_some_and(|s| s.right) { "right" } else { "left" }
}

/// Writes the beginning, the table and its header row, or the array.
pub(super) fn columns(p: &mut Printer<'_>) {
    if p.layout.style == Style::Json {
        p.put("[");
        return;
    }
    p.decide(0);
    p.write(&format!("<table border=\"{}\">", p.layout.border));
    if p.expanded || p.layout.tuples_only {
        return;
    }
    let heads: Vec<String> = p.names.iter().enumerate().map(|(at, n)| format!("<th align=\"{}\">{}</th>", align(p, at), html(n))).collect();
    p.write(&format!("  <tr>{}</tr>", heads.concat()));
}

/// Writes a row, of values `row` whose texts are `cells`.
pub(super) fn row(p: &mut Printer<'_>, row: &Row, cells: &[String]) {
    if p.layout.style == Style::Json {
        let members: Vec<String> =
            p.names.iter().zip(row.iter().zip(cells)).map(|(n, (v, text))| format!("{}:{}", json_string(n), json_value(v, text))).collect();
        let lead = if p.rows > 1 { ",\n" } else { "\n" };
        p.put(&format!("{lead}{{{}}}", members.join(",")));
        return;
    }
    if !p.expanded {
        let tds: Vec<String> = cells.iter().enumerate().map(|(at, c)| format!("<td align=\"{}\">{}</td>", align(p, at), html(c))).collect();
        p.write(&format!("  <tr>{}</tr>", tds.concat()));
        return;
    }
    p.write(&format!("  <tr><th colspan=\"2\">Row {}</th></tr>", p.rows));
    let names = p.names.clone();
    for (at, (name, value)) in names.iter().zip(cells).enumerate() {
        p.write(&format!("  <tr><td>{}</td><td align=\"{}\">{}</td></tr>", html(name), align(p, at), html(value)));
    }
}

/// Writes the end, the table closed and its footer, or the array closed.
pub(super) fn finish(p: &mut Printer<'_>, footer: Option<&str>) {
    if p.layout.style == Style::Json {
        p.write(if p.rows > 0 { "\n]" } else { "]" });
        return;
    }
    p.write("</table>");
    if let Some(text) = footer {
        p.write(&format!("<p>{}</p>", html(text)));
    }
}
