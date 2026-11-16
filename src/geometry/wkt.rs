//! \[EXTENSION\] XY WKT grammar, OGC 06-103r4 §§7.2.1–7.2.2.
//! Each component and coordinate consumes a bounded input budget.

use super::shape::{self, COLLECTION, EMPTY, LINE, NAMES, POINT, POLYGON, Point, Shape};
use super::{MAX_DEPTH, MAX_PARTS, invalid, limited, unsupported};
use crate::error::Result;

/// OGC 06-103r4 §§7.2.3–7.2.5 dimension suffixes, not supported here.
const DIMENSIONS: [&str; 3] = ["Z", "M", "ZM"];

pub(super) fn read(text: &str) -> Result<Shape> {
    let mut reader = Text { rest: text, parts: 0 };
    let shape = reader.tagged(1)?;
    if !reader.rest.trim().is_empty() {
        return Err(invalid("text follows the geometry"));
    }
    Ok(shape)
}

struct Text<'a> {
    rest: &'a str,
    parts: usize,
}

impl Text<'_> {
    fn charge(&mut self) -> Result<()> {
        self.parts = self.parts.saturating_add(1);
        limited(self.parts, MAX_PARTS)
    }

    fn tagged(&mut self, depth: usize) -> Result<Shape> {
        self.rest = self.rest.trim_start();
        let end = self.rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(self.rest.len());
        let (name, rest) = self.rest.split_at(end);
        self.rest = rest;
        if name.is_empty() {
            return Err(invalid("expected a geometry type name"));
        }
        let code = NAMES
            .iter()
            .position(|n| name.eq_ignore_ascii_case(n))
            .and_then(|i| u32::try_from(i.saturating_add(1)).ok())
            .ok_or_else(|| unsupported("that WKT geometry type"))?;
        self.body(code, depth)
    }

    fn body(&mut self, code: u32, depth: usize) -> Result<Shape> {
        limited(depth, MAX_DEPTH)?;
        self.charge()?;
        match code {
            POINT => {
                let points = self.list(Self::point)?;
                if points.len() > 1 {
                    return Err(invalid("a POINT holds at most one coordinate"));
                }
                Ok(Shape::Point(points.first().copied()))
            }
            LINE => {
                let points = self.list(Self::point)?;
                shape::line(&points)?;
                Ok(Shape::Line(points))
            }
            POLYGON => Ok(Shape::Polygon(self.list(Self::ring)?)),
            _ => {
                let child = shape::child(code)?;
                let next = depth.saturating_add(1);
                let parts = self.list(|r| match child {
                    Some(kind) => r.body(kind, next),
                    None => r.tagged(next),
                })?;
                Ok(Shape::Multi(code, parts))
            }
        }
    }

    fn ring(&mut self) -> Result<Vec<Point>> {
        self.charge()?;
        let points = self.list(Self::point)?;
        shape::ring(&points)?;
        Ok(points)
    }

    fn point(&mut self) -> Result<Point> {
        self.charge()?;
        let x = self.number()?;
        if !self.rest.starts_with(char::is_whitespace) {
            return Err(invalid("X and Y need separating whitespace"));
        }
        shape::coordinate([x, self.number()?])
    }

    fn number(&mut self) -> Result<f64> {
        self.rest = self.rest.trim_start();
        let end = self.rest.find(|c: char| c.is_whitespace() || c == ',' || c == ')').unwrap_or(self.rest.len());
        let (word, rest) = self.rest.split_at(end);
        self.rest = rest;
        // The grammar uses decimal literals, not Rust's inf/NaN spellings.
        if word.is_empty() || !word.chars().all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E')) {
            return Err(invalid("expected a decimal coordinate"));
        }
        word.parse::<f64>().map_err(|_| invalid("expected a decimal coordinate"))
    }

    fn list<T>(&mut self, mut item: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        self.rest = self.rest.trim_start();
        let end = self.rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(self.rest.len());
        let (word, rest) = self.rest.split_at(end);
        if DIMENSIONS.iter().any(|d| word.eq_ignore_ascii_case(d)) {
            return Err(unsupported("Z or M coordinates"));
        }
        if word.eq_ignore_ascii_case(EMPTY) {
            self.rest = rest;
            return Ok(Vec::new());
        }
        self.punctuation('(')?;
        let mut items = vec![item(self)?];
        loop {
            self.rest = self.rest.trim_start();
            if self.rest.starts_with(')') {
                self.punctuation(')')?;
                return Ok(items);
            }
            self.punctuation(',')?;
            items.push(item(self)?);
        }
    }

    fn punctuation(&mut self, expected: char) -> Result<()> {
        self.rest = self.rest.trim_start().strip_prefix(expected).ok_or_else(|| invalid("expected geometry parentheses or comma"))?;
        Ok(())
    }
}

pub(super) fn write(shape: &Shape) -> String {
    format!("{} {}", shape.name(), body(shape))
}

fn body(shape: &Shape) -> String {
    match shape {
        Shape::Point(point) => list(point.iter().map(point_text)),
        Shape::Line(points) => list(points.iter().map(point_text)),
        Shape::Polygon(rings) => list(rings.iter().map(|ring| list(ring.iter().map(point_text)))),
        Shape::Multi(code, parts) => {
            let tagged = *code == COLLECTION;
            list(parts.iter().map(|part| if tagged { write(part) } else { body(part) }))
        }
    }
}

fn list(parts: impl Iterator<Item = String>) -> String {
    let joined = parts.collect::<Vec<_>>().join(", ");
    if joined.is_empty() { EMPTY.to_owned() } else { format!("({joined})") }
}

fn point_text(point: &Point) -> String {
    let [x, y] = point.map(number_text);
    format!("{x} {y}")
}

fn number_text(number: f64) -> String {
    // Scientific notation keeps subnormals from expanding past MAX_BYTES.
    let ordinary = number.to_string();
    let scientific = format!("{number:e}");
    if ordinary.len() <= scientific.len() { ordinary } else { scientific }
}
