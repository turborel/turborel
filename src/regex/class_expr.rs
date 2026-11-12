//! A character class expression read [XSD 1.1 Part 2 §G.4.1 \[75\]-\[82\]]:
//! a positive or negative character group, of single characters, ranges
//! and character class escapes, less the class a subtraction names, with
//! the rules that tell a hyphen's meaning.

use super::class::CharSet;
use super::parse::{Escaped, MAX_NESTING, Parser};
use crate::error::Result;

/// What a `charGroupPart` begins with.
enum Part {
    /// A `singleChar`, and whether it is an unescaped hyphen.
    Single(char, bool),
    /// A `charClassEsc`.
    Class(CharSet),
}

impl Parser<'_> {
    /// The rest of `charClassExpr ::= '[' charGroup ']'`, after its "[",
    /// `depth` groups and subtractions deep, where `charGroup ::= (
    /// posCharGroup | negCharGroup ) ( '-' charClassExpr )?`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2201S if it is not one \[GR4\]; SQLCODE -900 if it nests
    /// deeper than `MAX_NESTING`.
    pub(super) fn class_expr(&mut self, depth: usize) -> Result<CharSet> {
        if depth > MAX_NESTING {
            return Err(super::beyond(&format!("groups and subtractions nest more than {MAX_NESTING} deep")));
        }
        // [§G.4.1] A first '^' begins a negCharGroup.
        let negative = self.peek() == Some('^');
        if negative {
            self.at = self.at.saturating_add(1);
        }
        let mut set = CharSet::default();
        let mut parts = 0_usize;
        loop {
            let Some(c) = self.peek() else { return Err(self.invalid("a '[' is not closed")) };
            if c == ']' || (c == '-' && self.peek_at(1) == Some('[')) {
                break;
            }
            set = set.union(&self.group_part()?);
            parts = parts.saturating_add(1);
        }
        // [§G.4.1 \[77\]] A posCharGroup has a part at least, so "[]" and
        // "[^]" are none.
        if parts == 0 {
            return Err(self.invalid("a character group is empty"));
        }
        if negative {
            set = set.complement();
        }
        if self.peek() == Some('-') {
            self.at = self.at.saturating_add(2);
            set = set.minus(&self.class_expr(depth.saturating_add(1))?);
        }
        if self.next() != Some(']') {
            return Err(self.invalid("a character class subtraction is followed by ']'"));
        }
        Ok(set)
    }

    /// [§G.4.1 \[79\]-\[81\]] A `charGroupPart`: a `singleChar`, a
    /// `charRange` or a `charClassEsc`, under the flag `i` a single
    /// character or range with its case variants.
    fn group_part(&mut self) -> Result<CharSet> {
        let (start, unescaped_hyphen) = match self.part()? {
            Part::Class(set) => return Ok(set),
            Part::Single(c, hyphen) => (c, hyphen),
        };
        if self.peek() != Some('-') {
            return Ok(self.cased(CharSet::single(start)));
        }
        match (self.peek_at(1), self.peek_at(2)) {
            // A subtraction follows.
            (Some('['), _) => return Ok(self.cased(CharSet::single(start))),
            // The hyphen is a singleChar, before ']' or "-[".
            (Some(']'), _) | (Some('-'), Some('[')) => {
                self.at = self.at.saturating_add(1);
                return Ok(self.cased(CharSet::chars(&[start, '-'])));
            }
            (Some(_) | None, _) => {}
        }
        self.at = self.at.saturating_add(1);
        if unescaped_hyphen {
            return Err(self.invalid("an unescaped '-' begins no range"));
        }
        let Part::Single(end, false) = self.part()? else {
            return Err(self.invalid("a range ends with a single character other than an unescaped '-'"));
        };
        // [§G.4.1 \[81\]] s-e, the code point of s no greater than e's.
        if start > end {
            return Err(self.invalid(&format!("the range {start}-{end} ends before it begins")));
        }
        Ok(self.cased(CharSet::of(vec![(u32::from(start), u32::from(end))])))
    }

    /// The `singleChar` or `charClassEsc` next: `SingleCharNoEsc` any
    /// character but `[` and `]` [§G.4.1 \[82\]].
    fn part(&mut self) -> Result<Part> {
        let Some(c) = self.next() else { return Err(self.invalid("a '[' is not closed")) };
        Ok(match c {
            '\\' => match self.escape()? {
                Escaped::Single(e) => Part::Single(e, false),
                Escaped::Class(set) => Part::Class(set),
            },
            '[' | ']' => return Err(self.invalid(&format!("an unescaped '{c}' is within a character group"))),
            _ => Part::Single(c, c == '-'),
        })
    }
}
