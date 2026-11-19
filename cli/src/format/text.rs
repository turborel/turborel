//! \[DESIGN\] Output of values apart by a separator (RULES.md D213, D219):
//! unaligned, apart by `\format separator`; CSV, apart by commas and
//! quoted as RFC 4180 §2 says; tab-separated, a tab, a newline, a carriage
//! return and a backslash in a value escaped. Expanded, each row is a line
//! for each column, its name and value, and a blank line after.

use super::{Printer, Style};

/// What separates CSV's fields [RFC 4180 §2].
const COMMA: &str = ",";
/// What separates tab-separated fields.
const TAB: &str = "\t";

/// The separator of the printer's style.
fn separator<'p>(p: &'p Printer<'_>) -> &'p str {
    match p.layout.style {
        Style::Csv => COMMA,
        Style::Tabs => TAB,
        Style::Unaligned | Style::Aligned | Style::Wrapped | Style::Html | Style::Json => &p.layout.separator,
    }
}

/// `value` as the printer's style writes a field.
fn field(p: &Printer<'_>, value: &str) -> String {
    match p.layout.style {
        Style::Csv => csv(value),
        Style::Tabs => escaped(value),
        Style::Unaligned | Style::Aligned | Style::Wrapped | Style::Html | Style::Json => value.to_owned(),
    }
}

/// [RFC 4180 §2] `value` as a CSV field: enclosed in double quotes, each
/// doubled, if it holds a comma, a quote or a line break.
pub(crate) fn csv(value: &str) -> String {
    if value.contains([',', '"', '\r', '\n']) { format!("\"{}\"", value.replace('"', "\"\"")) } else { value.to_owned() }
}

/// `value` as a tab-separated field: its backslashes, tabs, newlines and
/// carriage returns escaped.
pub(crate) fn escaped(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\r', "\\r")
}

/// Writes the header: the columns' names, unless rows alone are asked for
/// or rows are expanded.
pub(super) fn columns(p: &mut Printer<'_>) {
    let sep = separator(p).len().saturating_mul(p.shapes.len().saturating_sub(1));
    p.decide(p.shapes.iter().map(|s| s.width).fold(sep, usize::saturating_add));
    if p.expanded || p.layout.tuples_only {
        return;
    }
    let names: Vec<String> = p.names.iter().map(|n| field(p, n)).collect();
    let line = names.join(separator(p));
    p.write(&line);
}

/// Writes a row of `cells`: on a line, or expanded.
pub(super) fn row(p: &mut Printer<'_>, cells: &[String]) {
    let sep = separator(p).to_owned();
    if !p.expanded {
        let fields: Vec<String> = cells.iter().map(|c| field(p, c)).collect();
        p.write(&fields.join(&sep));
        return;
    }
    if p.rows > 1 {
        p.write("");
    }
    let names = p.names.clone();
    for (name, value) in names.iter().zip(cells) {
        let line = format!("{}{sep}{}", field(p, name), field(p, value));
        p.write(&line);
    }
}

/// Writes the footer of unaligned output, if any: CSV and tab-separated
/// output have none.
pub(super) fn finish(p: &mut Printer<'_>, footer: Option<&str>) {
    if p.layout.style != Style::Unaligned {
        return;
    }
    if let Some(text) = footer {
        if p.expanded && p.rows > 0 {
            p.write("");
        }
        p.write(text);
    }
}
