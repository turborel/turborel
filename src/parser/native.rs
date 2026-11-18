//! \[EXTENSION\] The syntax of native node and edge tables (RULES.md
//! D-GRAPH-NATIVE), Mode B of the property graph implementation standard
//! (§4.2): AS NODE or AS EDGE after a `<table definition>`'s elements, and
//! the CONNECTION constraint of an edge table. NODE and EDGE are words
//! read as `<non-reserved word>`s, so a column or table may still be named
//! either.
//!
//! ```text
//! <graph table kind> ::= AS { NODE | EDGE }
//! <connection constraint> ::= CONNECTION ( <connection clause>
//!     [ { , <connection clause> }... ] ) [ ON DELETE { CASCADE | NO ACTION } ]
//! <connection clause> ::= <table name> TO <table name>
//! ```

use super::Parser;
use crate::ast::TableConstraintKind;
use crate::catalog::NativeKind;
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// The word of a node table.
pub(crate) const NODE: &str = "NODE";
/// The word of an edge table.
pub(crate) const EDGE: &str = "EDGE";

impl Parser {
    /// Whether `<graph table kind>` is next: AS, then NODE or EDGE, which
    /// no `<as subquery clause>` writes after AS.
    pub(super) fn at_native_kind(&self) -> bool {
        let word = |w: &str| *self.peek_at(1) == Tok::Identifier(w.to_owned());
        self.is_kw(kw::AS) && (word(NODE) || word(EDGE))
    }

    /// `<graph table kind>`, if next, and where it begins.
    pub(super) fn native_kind(&mut self) -> Option<(NativeKind, usize)> {
        if !self.at_native_kind() {
            return None;
        }
        let at = self.here();
        self.advance();
        let kind = if self.accept_word(NODE) { NativeKind::Node } else { NativeKind::Edge };
        if kind == NativeKind::Edge {
            self.advance();
        }
        Some((kind, at))
    }

    /// `<connection constraint>`, CONNECTION next.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it does not parse.
    pub(super) fn connection_constraint(&mut self) -> Result<TableConstraintKind> {
        self.expect_kw(kw::CONNECTION)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut clauses = Vec::new();
        loop {
            let from = self.table_name()?;
            self.expect_kw(kw::TO)?;
            clauses.push((from, self.table_name()?));
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        let cascade = if self.accept_kw(kw::ON) {
            self.expect_kw(kw::DELETE)?;
            let cascade = self.accept_kw(kw::CASCADE);
            if !cascade {
                self.expect_kw(kw::NO)?;
                self.expect_kw(kw::ACTION)?;
            }
            cascade
        } else {
            false
        };
        Ok(TableConstraintKind::Connection { clauses, cascade })
    }
}
