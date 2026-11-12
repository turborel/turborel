//! An XQuery regular expression read [F&O 3.1 §5.6.1; XSD 1.1 Part 2
//! §G.1-§G.3]: `regExp`, `branch`, `piece`, `atom` and `quantifier`, with
//! F&O's `^` and `$`, reluctant quantifiers, groups that capture nothing,
//! and back-references. Character class expressions are read in
//! `regex/class_expr.rs`.

use super::class::{CharSet, fixed_escape};
use super::flags::{Flag, Flags};
use super::unicode::{CATEGORIES, Table};
use super::{beyond, invalid};
use crate::error::Result;

/// [DESIGN] How deep groups and class subtractions nest at most, so the
/// parser's recursion, and the compiler's over what it reads, is bounded
/// (RULES.md R-CF-6).
pub(super) const MAX_NESTING: usize = 64;
/// The base of a quantity or back-reference's digits.
const DECIMAL: u32 = 10;
/// [XSD 1.1 Part 2 §G.4.2.2 \[88\]-\[95\]] The categories `\p` names by
/// two letters: each first letter, and the second letters it takes.
const CATEGORY_LETTERS: [(char, &str); 7] =
    [('L', "ultmo"), ('M', "nce"), ('N', "dlo"), ('P', "cdseifo"), ('Z', "slp"), ('S', "mcko"), ('C', "cfon")];
/// [§G.4.2.3 \[96\]] What a block escape's name begins with.
const BLOCK_PREFIX: &str = "Is";

/// A regular expression read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Node {
    /// Nothing: the empty string.
    Empty,
    /// One character of the set: a normal character, an escaped one, or a
    /// character class.
    Set(CharSet),
    /// [§9.28 GR9a i] The wildcard `.`.
    Dot,
    /// [§9.28 GR9a ii] `\s`: a space, a tab or a newline sequence, CRLF
    /// one of them.
    Space,
    /// `^`, a line's start.
    Start,
    /// `$`, a line's end.
    End,
    /// A parenthesized expression, capturing as the group numbered, if it is.
    Group(Box<Node>, Option<usize>),
    /// Pieces, one after another.
    Concat(Vec<Node>),
    /// Branches, the first that matches preferred.
    Alt(Vec<Node>),
    /// A quantified atom.
    Repeat(Box<Node>, Quantity),
    /// [F&O 3.1 §5.6.1.4] `\N`, what group N last captured.
    BackRef(usize),
}

/// How many times a quantified atom is matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Quantity {
    /// At least this many.
    pub(super) min: u32,
    /// At most this many, if a most is set.
    pub(super) max: Option<u32>,
    /// Whether as many as can be are preferred, rather than as few
    /// [F&O 3.1 §5.6.1.2].
    pub(super) greedy: bool,
}

/// A regular expression read, and what its compiling needs to know.
pub(super) struct Parsed {
    /// What it is.
    pub(super) node: Node,
    /// How many groups capture.
    pub(super) groups: usize,
    /// Whether a back-reference is in it.
    pub(super) back_references: bool,
}

/// A pattern being read.
pub(super) struct Parser<'t> {
    pub(super) chars: Vec<char>,
    pub(super) at: usize,
    caseless: bool,
    pub(super) table: &'t Table,
    opened: usize,
    closed: Vec<bool>,
    back_references: bool,
}

/// What an escape within or outside a class names.
pub(super) enum Escaped {
    /// One character [§G.4.2.1].
    Single(char),
    /// A class of characters [§G.4.2].
    Class(CharSet),
}

/// `pattern` read under `flags`.
///
/// # Errors
///
/// SQLSTATE 2201S if it is no XQuery regular expression \[GR4\]; SQLCODE
/// -900 if it nests deeper than `MAX_NESTING` or a quantity is too large
/// to count.
pub(super) fn parse(pattern: &str, flags: Flags, table: &Table) -> Result<Parsed> {
    let text = if flags.has(Flag::Extended) { super::flags::stripped(pattern) } else { pattern.to_owned() };
    let mut p = Parser {
        chars: text.chars().collect(),
        at: 0,
        caseless: flags.has(Flag::Caseless),
        table,
        opened: 0,
        closed: Vec::new(),
        back_references: false,
    };
    let node = if flags.has(Flag::Literal) {
        // [F&O 3.1 §5.6.2] q: each character itself.
        Node::Concat(p.chars.clone().into_iter().map(|c| Node::Set(p.cased(CharSet::single(c)))).collect())
    } else {
        let node = p.reg_exp(0)?;
        if p.peek().is_some() {
            return Err(p.invalid("a ')' closes no group"));
        }
        node
    };
    Ok(Parsed { node, groups: p.opened, back_references: p.back_references })
}

impl Parser<'_> {
    pub(super) fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    pub(super) fn peek_at(&self, n: usize) -> Option<char> {
        self.chars.get(self.at.saturating_add(n)).copied()
    }

    pub(super) fn next(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at = self.at.saturating_add(1);
        Some(c)
    }

    fn accept(&mut self, c: char) -> bool {
        let found = self.peek() == Some(c);
        if found {
            self.at = self.at.saturating_add(1);
        }
        found
    }

    /// The error of a pattern no regular expression, `why`, at the
    /// character just read.
    pub(super) fn invalid(&self, why: &str) -> crate::error::SqlError {
        invalid(&format!("{why} (at character {})", self.at.max(1)))
    }

    /// `set`, and under the flag `i` each character's case variants [F&O
    /// 3.1 §5.6.2].
    pub(super) fn cased(&self, set: CharSet) -> CharSet {
        if !self.caseless {
            return set;
        }
        let more: Vec<(u32, u32)> = set.ranges().iter().flat_map(|&(first, last)| self.table.variants(first, last)).collect();
        set.union(&CharSet::of(more))
    }

    /// [§G.1 \[64\]] `regExp` ::= `branch` ( '|' `branch` )*, `depth`
    /// groups deep.
    fn reg_exp(&mut self, depth: usize) -> Result<Node> {
        if depth > MAX_NESTING {
            return Err(beyond(&format!("groups nest more than {MAX_NESTING} deep")));
        }
        let mut branches = vec![self.branch(depth)?];
        while self.accept('|') {
            branches.push(self.branch(depth)?);
        }
        Ok(if branches.len() == 1 { branches.pop().unwrap_or(Node::Empty) } else { Node::Alt(branches) })
    }

    /// [§G.1 \[65\]] `branch` ::= `piece`*.
    fn branch(&mut self, depth: usize) -> Result<Node> {
        let mut pieces = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            pieces.push(self.piece(depth)?);
        }
        Ok(match pieces.len() {
            0 => Node::Empty,
            1 => pieces.pop().unwrap_or(Node::Empty),
            _ => Node::Concat(pieces),
        })
    }

    /// [§G.2 \[66\]] `piece` ::= `atom` `quantifier`?
    fn piece(&mut self, depth: usize) -> Result<Node> {
        let atom = self.atom(depth)?;
        Ok(match self.quantifier()? {
            Some(q) => Node::Repeat(Box::new(atom), q),
            None => atom,
        })
    }

    /// [§G.2 \[72\]; F&O 3.1 §5.6.1.1, §5.6.1.3, §5.6.1.4] `atom` ::=
    /// `NormalChar` | `charClass` | '(' '?:'? `regExp` ')' |
    /// `backReference`, `charClass` taking `^` and `$` too.
    fn atom(&mut self, depth: usize) -> Result<Node> {
        let Some(c) = self.next() else { return Err(self.invalid("an atom is missing")) };
        Ok(match c {
            '(' => self.group(depth)?,
            '[' => Node::Set(self.class_expr(depth.saturating_add(1))?),
            '\\' => self.escape_atom()?,
            '.' => Node::Dot,
            '^' => Node::Start,
            '$' => Node::End,
            '?' | '*' | '+' | '{' | '}' | ')' | ']' | '|' => return Err(self.invalid(&format!("'{c}' begins no atom"))),
            _ => Node::Set(self.cased(CharSet::single(c))),
        })
    }

    /// The rest of a parenthesized expression, after its "(": a group that
    /// captures unless "?:" begins it [F&O 3.1 §5.6.1.3].
    fn group(&mut self, depth: usize) -> Result<Node> {
        let capturing = !(self.peek() == Some('?') && self.peek_at(1) == Some(':'));
        let number = if capturing {
            self.opened = self.opened.saturating_add(1);
            self.closed.push(false);
            Some(self.opened)
        } else {
            self.at = self.at.saturating_add(2);
            None
        };
        let inner = self.reg_exp(depth.saturating_add(1))?;
        if !self.accept(')') {
            return Err(self.invalid("a '(' is not closed"));
        }
        if let Some(slot) = number.and_then(|n| n.checked_sub(1)).and_then(|i| self.closed.get_mut(i)) {
            *slot = true;
        }
        Ok(Node::Group(Box::new(inner), number))
    }

    /// An escape as an atom, after its "\": a back-reference, `\s` with
    /// its CRLF, or what `escape` names.
    fn escape_atom(&mut self) -> Result<Node> {
        let Some(c) = self.peek() else { return Err(self.invalid("a '\\' ends the pattern")) };
        if c.is_ascii_digit() && c != '0' {
            self.at = self.at.saturating_add(1);
            return self.back_reference(c);
        }
        if c == 's' {
            self.at = self.at.saturating_add(1);
            return Ok(Node::Space);
        }
        Ok(match self.escape()? {
            Escaped::Single(single) => Node::Set(self.cased(CharSet::single(single))),
            Escaped::Class(set) => Node::Set(set),
        })
    }

    /// [§G.4.2.1 \[84\], §G.4.2 \[83\]; F&O 3.1 §5.6.1.1] What the escape
    /// after a "\" names: one character, or a class.
    pub(super) fn escape(&mut self) -> Result<Escaped> {
        let Some(c) = self.next() else { return Err(self.invalid("a '\\' ends the pattern")) };
        Ok(match c {
            'n' => Escaped::Single('\n'),
            'r' => Escaped::Single('\r'),
            't' => Escaped::Single('\t'),
            '\\' | '|' | '.' | '?' | '*' | '+' | '(' | ')' | '{' | '}' | '$' | '-' | '[' | ']' | '^' => Escaped::Single(c),
            'p' | 'P' => Escaped::Class(self.property(c == 'P')?),
            'd' | 'D' | 'w' | 'W' => Escaped::Class(self.categorized(c)),
            _ => match fixed_escape(c) {
                Some(set) => Escaped::Class(set),
                None => return Err(self.invalid(&format!("'\\{c}' is no escape"))),
            },
        })
    }

    /// [§G.4.2.5] `\d`, `\D`, `\w` or `\W`, of categories: `\d` is Nd, and
    /// `\w` every character but those of P, Z and C.
    fn categorized(&self, letter: char) -> CharSet {
        let nd = || self.table.categories(&[CATEGORIES.iter().position(|c| *c == "Nd").unwrap_or_default()]);
        let not_word = || {
            let places: Vec<usize> =
                CATEGORIES.iter().enumerate().filter(|(_, c)| c.starts_with(['P', 'Z', 'C']) && **c != "Cs").map(|(i, _)| i).collect();
            self.table.categories(&places)
        };
        match letter {
            'd' => nd(),
            'D' => nd().complement(),
            'w' => not_word().complement(),
            // \W, the one letter left.
            _ => not_word(),
        }
    }

    /// [§G.4.2.2, §G.4.2.3; F&O 3.1 §5.6.1.5] The rest of `\p{X}`, or of
    /// `\P{X}` if `complement`, after its "p": a category or a block.
    fn property(&mut self, complement: bool) -> Result<CharSet> {
        if !self.accept('{') {
            return Err(self.invalid("'\\p' and '\\P' are followed by '{'"));
        }
        let mut name = String::new();
        loop {
            match self.next() {
                Some('}') => break,
                Some(c) => name.push(c),
                None => return Err(self.invalid("a '\\p{' is not closed")),
            }
        }
        let set = self.category(&name).or_else(|| self.named_block(&name)).transpose()?;
        let set = set.ok_or_else(|| self.invalid(&format!("'{name}' is no category, and no block of Unicode 13.0")))?;
        Ok(if complement { set.complement() } else { set })
    }

    /// The characters of the category `name` [§G.4.2.2 \[88\]-\[95\]], a
    /// one-letter one all those it begins; none if it names none.
    fn category(&self, name: &str) -> Option<Result<CharSet>> {
        let mut letters = name.chars();
        let (first, second) = (letters.next()?, letters.next());
        let (_, seconds) = CATEGORY_LETTERS.iter().find(|(f, _)| *f == first)?;
        if letters.next().is_some() || second.is_some_and(|s| !seconds.contains(s)) {
            return None;
        }
        let wanted: Vec<usize> = CATEGORIES
            .iter()
            .enumerate()
            .filter(|(_, c)| **c != "Cs" && c.starts_with(first) && second.is_none_or(|s| c.ends_with(s)))
            .map(|(i, _)| i)
            .collect();
        Some(Ok(self.table.categories(&wanted)))
    }

    /// The characters of the block `name` names, "Is" and its normalized
    /// name [§G.4.2.3 \[96\]]; none if it is not of that form; an error if
    /// no block has the name [F&O 3.1 §5.6.1.5].
    fn named_block(&self, name: &str) -> Option<Result<CharSet>> {
        let block = name.strip_prefix(BLOCK_PREFIX)?;
        if block.is_empty() || !block.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return None;
        }
        Some(self.table.block(block).ok_or_else(|| self.invalid(&format!("'{block}' is no block of Unicode 13.0"))))
    }

    /// [F&O 3.1 §5.6.1.4] The rest of a back-reference after its first
    /// digit: a digit more only if the group it then numbers was opened
    /// before it, and the group closed before it.
    fn back_reference(&mut self, first: char) -> Result<Node> {
        let opened = u32::try_from(self.opened).unwrap_or(u32::MAX);
        let mut n = first.to_digit(DECIMAL).unwrap_or_default();
        while let Some(d) = self.peek().and_then(|c| c.to_digit(DECIMAL)) {
            let longer = n.checked_mul(DECIMAL).and_then(|m| m.checked_add(d));
            let Some(l) = longer.filter(|l| *l <= opened) else { break };
            n = l;
            self.at = self.at.saturating_add(1);
        }
        if n > opened {
            return Err(self.invalid(&format!("'\\{n}' refers to no group opened before it")));
        }
        let number = usize::try_from(n).unwrap_or(usize::MAX);
        if !number.checked_sub(1).and_then(|i| self.closed.get(i)).copied().unwrap_or(false) {
            return Err(self.invalid(&format!("'\\{n}' is within the group it refers to")));
        }
        self.back_references = true;
        Ok(Node::BackRef(number))
    }

    /// [§G.2 \[67\]-\[71\]; F&O 3.1 §5.6.1.2] A `quantifier`, if one is
    /// next, and the "?" that makes it reluctant.
    fn quantifier(&mut self) -> Result<Option<Quantity>> {
        let (min, max) = match self.peek() {
            Some('?') => (0, Some(1)),
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('{') => {
                self.at = self.at.saturating_add(1);
                self.quantity()?
            }
            Some(_) | None => return Ok(None),
        };
        self.at = self.at.saturating_add(1);
        let greedy = !self.accept('?');
        Ok(Some(Quantity { min, max, greedy }))
    }

    /// The rest of '{' `quantity` '}', after its "{" and up to its "}": n,
    /// n, or n,m with n no greater than m.
    fn quantity(&mut self) -> Result<(u32, Option<u32>)> {
        let min = self.count()?.ok_or_else(|| self.invalid("a '{' is followed by a number"))?;
        let max = if self.accept(',') { self.count()? } else { Some(min) };
        if self.peek() != Some('}') {
            return Err(self.invalid("a quantity is closed by '}'"));
        }
        if let Some(most) = max.filter(|m| *m < min) {
            return Err(self.invalid(&format!("the quantity {{{min},{most}}} is of a most less than its least")));
        }
        Ok((min, max))
    }

    /// The number whose digits are next, if any are.
    fn count(&mut self) -> Result<Option<u32>> {
        let mut n: Option<u32> = None;
        while let Some(d) = self.peek().and_then(|c| c.to_digit(DECIMAL)) {
            let more = n.unwrap_or(0).checked_mul(DECIMAL).and_then(|m| m.checked_add(d));
            n = Some(more.ok_or_else(|| beyond("a quantity is too large to count"))?);
            self.at = self.at.saturating_add(1);
        }
        Ok(n)
    }
}
