//! \[DESIGN\] How a query's rows are written (RULES.md D46, D213): aligned
//! in columns, as they come (the default); unaligned, apart by a
//! separator; wrapped to the screen's width; as CSV (RFC 4180), as an
//! HTML table, as JSON, or as tab-separated text. Each style may be
//! expanded, a row a block of lines, a column's name and value each; or
//! expanded only when its rows are wider than the screen. A border, the
//! text of a null, the separator, whether the header and footer are
//! written, are options of it (`\format`).

mod markup;
mod table;
mod text;
pub mod width;

use std::io::{self, Write};

use turborel::storage::Row;
use turborel::value::Value;
use turborel_client::Column;
use turborel_client::connection::Sink;

use crate::show::{Shape, shape};
use crate::vars::truth;
use width::Widths;

/// The most a border may be: 0 none, 1 between columns, 2 around too.
pub const MAX_BORDER: u8 = 2;
/// The separator of unaligned output, unless one is named.
pub const DEFAULT_SEPARATOR: &str = "|";

/// A style of output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// In columns as wide as their types, as rows come.
    Aligned,
    /// Each value apart by the separator.
    Unaligned,
    /// In columns made to fit the screen, values wrapped within them.
    Wrapped,
    /// Comma-separated values, as RFC 4180 writes them.
    Csv,
    /// An HTML table.
    Html,
    /// A JSON array of an object for each row.
    Json,
    /// Values apart by tabs, a tab, a newline and a backslash escaped.
    Tabs,
}

/// The styles, by the names `\format style` takes.
const STYLES: [(&str, Style); 7] = [
    ("aligned", Style::Aligned),
    ("unaligned", Style::Unaligned),
    ("wrapped", Style::Wrapped),
    ("csv", Style::Csv),
    ("html", Style::Html),
    ("json", Style::Json),
    ("tabs", Style::Tabs),
];

/// Whether rows are expanded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expanded {
    /// Never.
    Off,
    /// Always.
    On,
    /// When the screen is narrower than a row.
    Auto,
}

/// When output goes through a pager (`pager.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paging {
    /// Never.
    Off,
    /// When it is taller than the screen.
    On,
    /// Whenever the output is a screen.
    Always,
}

/// How results are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The style.
    pub style: Style,
    /// Whether rows are expanded.
    pub expanded: Expanded,
    /// The border: 0, 1 or 2.
    pub border: u8,
    /// What a null is written as.
    pub null: String,
    /// What separates unaligned values.
    pub separator: String,
    /// Whether the rows alone are written, without header or footer.
    pub tuples_only: bool,
    /// Whether the count of rows is written after them.
    pub footer: bool,
    /// When output goes through a pager.
    pub paging: Paging,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout {
            style: Style::Aligned,
            expanded: Expanded::Off,
            border: 1,
            null: String::new(),
            separator: DEFAULT_SEPARATOR.to_owned(),
            tuples_only: false,
            footer: true,
            paging: Paging::On,
        }
    }
}

/// The names of `\format`'s options, in the order listed.
pub const OPTIONS: [&str; 8] = ["style", "expanded", "border", "null", "separator", "tuples", "footer", "pager"];

impl Layout {
    /// Sets `option` to `value`.
    ///
    /// # Errors
    ///
    /// No option has the name, or the value is not one of it.
    pub fn set(&mut self, option: &str, value: &str) -> Result<(), String> {
        let wrong = || format!("{value} is no value of {option}");
        match option {
            "style" => self.style = STYLES.iter().find(|(n, _)| n.eq_ignore_ascii_case(value)).map(|(_, s)| *s).ok_or_else(wrong)?,
            "expanded" => self.expanded = expanded(value).ok_or_else(wrong)?,
            "border" => self.border = value.parse().ok().filter(|b| *b <= MAX_BORDER).ok_or_else(wrong)?,
            "null" => value.clone_into(&mut self.null),
            "separator" => value.clone_into(&mut self.separator),
            "tuples" => self.tuples_only = truth(value).ok_or_else(wrong)?,
            "footer" => self.footer = truth(value).ok_or_else(wrong)?,
            "pager" => self.paging = paging(value).ok_or_else(wrong)?,
            _ => return Err(format!("{option} is no option of \\format: {}", OPTIONS.join(", "))),
        }
        Ok(())
    }

    /// The value of `option`, as `set` takes it, if it is one.
    pub fn get(&self, option: &str) -> Option<String> {
        let on = |b: bool| if b { "on" } else { "off" }.to_owned();
        Some(match option {
            "style" => STYLES.iter().find(|(_, s)| *s == self.style).map(|(n, _)| (*n).to_owned()).unwrap_or_default(),
            "expanded" => ["off", "on", "auto"].get(expanded_index(self.expanded)).copied().unwrap_or_default().to_owned(),
            "border" => self.border.to_string(),
            "null" => self.null.clone(),
            "separator" => self.separator.clone(),
            "tuples" => on(self.tuples_only),
            "footer" => on(self.footer),
            "pager" => ["off", "on", "always"].get(paging_index(self.paging)).copied().unwrap_or_default().to_owned(),
            _ => return None,
        })
    }
}

/// How rows are expanded, by `value`'s name.
fn expanded(value: &str) -> Option<Expanded> {
    if value.eq_ignore_ascii_case(crate::command::AUTO) {
        return Some(Expanded::Auto);
    }
    truth(value).map(|on| if on { Expanded::On } else { Expanded::Off })
}

/// The place of `e` in the names of expanded's values.
fn expanded_index(e: Expanded) -> usize {
    match e {
        Expanded::Off => 0,
        Expanded::On => 1,
        Expanded::Auto => 2,
    }
}

/// When output is paged, by `value`'s name.
fn paging(value: &str) -> Option<Paging> {
    if value.eq_ignore_ascii_case("always") {
        return Some(Paging::Always);
    }
    truth(value).map(|on| if on { Paging::On } else { Paging::Off })
}

/// The place of `p` in the names of pager's values.
fn paging_index(p: Paging) -> usize {
    match p {
        Paging::Off => 0,
        Paging::On => 1,
        Paging::Always => 2,
    }
}

/// The text of `v`, a null written as `null`.
pub fn cell(v: &Value, null: &str) -> String {
    if matches!(v, Value::Null) { null.to_owned() } else { v.to_string() }
}

/// A query's rows written to `out` as rows come, as a layout says.
pub struct Printer<'a> {
    out: &'a mut dyn Write,
    layout: &'a Layout,
    /// The screen's width in columns, if output is a screen.
    screen: Option<usize>,
    /// Whether this result's rows are expanded, as decided by its columns.
    expanded: bool,
    names: Vec<String>,
    shapes: Vec<Shape>,
    /// The rows written so far.
    rows: u64,
    widths: Widths,
    /// The first failure to write: nothing more is written after it.
    failed: Option<io::Error>,
    /// The widest value of each column, of rows known before they are
    /// written, so their columns are no wider than they need be.
    known: Option<Vec<usize>>,
}

impl<'a> Printer<'a> {
    /// A printer to `out`, as `layout` says, of a screen `screen` columns
    /// wide if output is a screen, its rows expanded if `expanded` or the
    /// layout says.
    pub fn new(out: &'a mut dyn Write, layout: &'a Layout, screen: Option<usize>, expanded: bool) -> Printer<'a> {
        let expanded = expanded || layout.expanded == Expanded::On;
        Printer {
            out,
            layout,
            screen,
            expanded,
            names: Vec::new(),
            shapes: Vec::new(),
            rows: 0,
            widths: Widths::default(),
            failed: None,
            known: None,
        }
    }

    /// The printer, its columns as wide as `rows`, all it is to write,
    /// need, or their names.
    #[must_use]
    pub fn fitted(mut self, rows: &[Row]) -> Printer<'a> {
        let mut widest: Vec<usize> = Vec::new();
        for row in rows {
            widest.resize(widest.len().max(row.len()), 0);
            for (w, v) in widest.iter_mut().zip(row) {
                *w = (*w).max(self.widths.of(&cell(v, &self.layout.null)));
            }
        }
        self.known = Some(widest);
        self
    }

    /// Ends the output, `count` rows written, and the SQLSTATE of the
    /// warning its query completed with, if any [ISO/IEC 9075:1992 §22.1].
    ///
    /// # Errors
    ///
    /// The first failure to write.
    pub fn finish(mut self, count: u64, warning: Option<&str>) -> io::Result<()> {
        let plural = if count == 1 { "" } else { "s" };
        let warned = warning.map_or_else(String::new, |state| format!(", with a warning: SQLSTATE {state}"));
        let footer = (self.layout.footer && !self.layout.tuples_only).then(|| format!("({count} row{plural}{warned})"));
        match self.layout.style {
            Style::Aligned | Style::Wrapped => table::finish(&mut self, footer.as_deref()),
            Style::Unaligned | Style::Csv | Style::Tabs => text::finish(&mut self, footer.as_deref()),
            Style::Html | Style::Json => markup::finish(&mut self, footer.as_deref()),
        }
        if self.failed.is_none() {
            self.failed = self.out.flush().err();
        }
        self.failed.map_or(Ok(()), Err)
    }

    /// Writes `line` and a newline, unless writing failed.
    fn write(&mut self, line: &str) {
        if self.failed.is_none() {
            self.failed = writeln!(self.out, "{line}").err();
        }
    }

    /// Writes `text` as it is, unless writing failed.
    fn put(&mut self, text: &str) {
        if self.failed.is_none() {
            self.failed = self.out.write_all(text.as_bytes()).err();
        }
    }

    /// Whether this result's rows are expanded: asked for, or automatic
    /// and wider than the screen, `width` columns.
    fn decide(&mut self, width: usize) {
        let auto = self.layout.expanded == Expanded::Auto && self.screen.is_some_and(|s| width > s);
        self.expanded |= auto;
    }
}

impl Sink for Printer<'_> {
    fn columns(&mut self, columns: &[Column]) {
        self.names = columns.iter().map(|c| c.name.clone().unwrap_or_default()).collect();
        let named: Vec<usize> = self.names.iter().map(|n| self.widths.of(n)).collect();
        self.shapes = columns.iter().zip(&named).map(|(c, w)| shape(*w, &c.data_type)).collect();
        if let Some(known) = &self.known {
            for ((s, w), name) in self.shapes.iter_mut().zip(known.iter().chain(std::iter::repeat(&0))).zip(named) {
                s.width = s.width.min((*w).max(name));
            }
        }
        match self.layout.style {
            Style::Aligned | Style::Wrapped => table::columns(self),
            Style::Unaligned | Style::Csv | Style::Tabs => text::columns(self),
            Style::Html | Style::Json => markup::columns(self),
        }
    }

    fn rows(&mut self, rows: &[Row]) {
        for row in rows {
            self.rows = self.rows.saturating_add(1);
            let cells: Vec<String> = row.iter().map(|v| cell(v, &self.layout.null)).collect();
            match self.layout.style {
                Style::Aligned | Style::Wrapped => table::row(self, &cells),
                Style::Unaligned | Style::Csv | Style::Tabs => text::row(self, &cells),
                Style::Html | Style::Json => markup::row(self, row, &cells),
            }
        }
        if self.failed.is_none() {
            self.failed = self.out.flush().err();
        }
    }
}

#[cfg(test)]
mod tests;
