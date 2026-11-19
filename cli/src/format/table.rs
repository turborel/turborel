//! \[DESIGN\] Aligned and wrapped output (RULES.md D46, D213). It has
//! columns as wide as their types, or made narrower to fit the screen and
//! their values wrapped within them; a border of none, of lines between
//! columns (the default), or around them too; or each row expanded, a
//! line for each of its columns, its name then its value.

use super::{Printer, Style};

/// The narrowest a wrapped column is made.
const NARROWEST: usize = 3;
/// What is between two columns, of each border.
const BETWEEN: [&str; 3] = [" ", " | ", " | "];
/// What a line begins with, of each border.
const BEGIN: [&str; 3] = ["", " ", "| "];
/// What a line ends with, of each border.
const END: [&str; 3] = ["", "", " |"];

/// The border, as an index of the texts above.
fn border(p: &Printer<'_>) -> usize {
    usize::from(p.layout.border.min(super::MAX_BORDER))
}

/// The columns of a line of columns `widths` wide, for border `b`.
fn line_width(widths: impl Iterator<Item = usize>, b: usize) -> usize {
    let (mut total, mut count) = (0_usize, 0_usize);
    for w in widths {
        total = total.saturating_add(w);
        count = count.saturating_add(1);
    }
    let between = BETWEEN.get(b).map_or(0, |s| s.len()).saturating_mul(count.saturating_sub(1));
    let ends = BEGIN.get(b).map_or(0, |s| s.len()).saturating_add(END.get(b).map_or(0, |s| s.len()));
    total.saturating_add(between).saturating_add(ends)
}

/// Writes the header, after the columns are known. Wrapped columns are
/// made narrower to fit the screen first, and expanded rows decided.
pub(super) fn columns(p: &mut Printer<'_>) {
    let b = border(p);
    p.decide(line_width(p.shapes.iter().map(|s| s.width), b));
    if p.layout.style == Style::Wrapped && !p.expanded {
        fit(p, b);
    }
    if p.expanded {
        return;
    }
    if b == 2 {
        let rule = rule(p, b);
        p.write(&rule);
    }
    if p.layout.tuples_only {
        return;
    }
    let names = p.names.clone();
    lines(p, &names);
    let rule = rule(p, b);
    p.write(&rule);
}

/// Makes the widest columns narrower, one column at a time, until a line
/// fits the screen or none can be.
fn fit(p: &mut Printer<'_>, b: usize) {
    let Some(screen) = p.screen else { return };
    let mut over = line_width(p.shapes.iter().map(|s| s.width), b).saturating_sub(screen);
    while over > 0 {
        let Some(widest) = p.shapes.iter_mut().filter(|s| s.width > NARROWEST).max_by_key(|s| s.width) else { return };
        widest.width = widest.width.saturating_sub(1);
        over = over.saturating_sub(1);
    }
}

/// The rule under the header, and around the table of border 2.
fn rule(p: &Printer<'_>, b: usize) -> String {
    match b {
        0 => p.shapes.iter().map(|s| "-".repeat(s.width)).collect::<Vec<_>>().join(" "),
        1 => p.shapes.iter().map(|s| "-".repeat(s.width.saturating_add(2))).collect::<Vec<_>>().join("+"),
        _ => format!("+{}+", p.shapes.iter().map(|s| "-".repeat(s.width.saturating_add(2))).collect::<Vec<_>>().join("+")),
    }
}

/// Writes a row of `cells`, expanded or in its columns.
pub(super) fn row(p: &mut Printer<'_>, cells: &[String]) {
    if p.expanded {
        expanded(p, cells);
    } else {
        lines(p, cells);
    }
}

/// Writes the lines of `cells` in their columns, either one, values past
/// their columns shown whole, or, wrapped, as many as the longest value
/// needs.
fn lines(p: &mut Printer<'_>, cells: &[String]) {
    let b = border(p);
    let wrapped = p.layout.style == Style::Wrapped;
    let parts: Vec<Vec<String>> = if wrapped {
        cells.iter().zip(p.shapes.clone()).map(|(c, s)| wrap(p, c, s.width)).collect()
    } else {
        cells.iter().map(|c| vec![c.clone()]).collect()
    };
    let height = parts.iter().map(Vec::len).max().unwrap_or(1);
    for at in 0..height {
        let padded: Vec<String> = parts
            .iter()
            .zip(p.shapes.clone())
            .map(|(part, s)| p.widths.pad(part.get(at).map_or("", String::as_str), s.width, s.right))
            .collect();
        let text = format!("{}{}{}", BEGIN.get(b).unwrap_or(&""), padded.join(BETWEEN.get(b).unwrap_or(&" ")), END.get(b).unwrap_or(&""));
        let line = if b == 2 { text } else { text.trim_end().to_owned() };
        p.write(&line);
    }
}

/// `text` as lines of at most `width` columns, broken at each newline and
/// where a line fills the width.
fn wrap(p: &mut Printer<'_>, text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for piece in text.split('\n') {
        let mut rest = piece;
        loop {
            let (head, tail) = p.widths.split_at_width(rest, width.max(1));
            out.push(head.to_owned());
            if tail.is_empty() {
                break;
            }
            rest = tail;
        }
    }
    out
}

/// Writes a row expanded, a line naming it, then each column's name and
/// value, values wrapped to the screen if the style is wrapped.
fn expanded(p: &mut Printer<'_>, cells: &[String]) {
    let b = border(p);
    let label = p.names.iter().map(|n| p.widths.of(n)).max().unwrap_or(0);
    let value_room = p.screen.map(|s| s.saturating_sub(label.saturating_add(3)).max(NARROWEST));
    let wrapped = p.layout.style == Style::Wrapped;
    let mut entries: Vec<(String, Vec<String>)> = Vec::new();
    for (name, value) in p.names.clone().iter().zip(cells) {
        let parts = match (wrapped, value_room) {
            (true, Some(room)) => wrap(p, value, room),
            (true, None) | (false, _) => vec![value.clone()],
        };
        entries.push((name.clone(), parts));
    }
    let widest = entries.iter().flat_map(|(_, parts)| parts.iter()).map(|v| p.widths.of(v)).max().unwrap_or(0);
    let title = format!("-[ ROW {} ]", p.rows);
    let dashes = "-".repeat(label.saturating_add(3).saturating_add(widest).saturating_sub(title.len()));
    p.write(&format!("{title}{dashes}"));
    let between = if b == 0 { " " } else { " | " };
    for (name, parts) in entries {
        for (at, part) in parts.iter().enumerate() {
            let shown = if at == 0 { name.as_str() } else { "" };
            let padded = p.widths.pad(shown, label, false);
            p.write(format!("{padded}{between}{part}").trim_end());
        }
    }
}

/// Writes the end, the frame of border 2, then the footer, if any.
pub(super) fn finish(p: &mut Printer<'_>, footer: Option<&str>) {
    let b = border(p);
    if b == 2 && !p.expanded && !p.shapes.is_empty() {
        let rule = rule(p, b);
        p.write(&rule);
    }
    if let Some(text) = footer {
        p.write(text);
    }
}
