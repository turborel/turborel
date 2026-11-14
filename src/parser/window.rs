//! [ISO/IEC 9075-2:2023 §6.10] `<window function>` and [§7.15] `<window
//! clause>`. SQL-92 reserves CURRENT, NO and ROWS. OVER, WINDOW, RANGE,
//! GROUPS, ROW_NUMBER and the rank names, which SQL:2023 reserves and SQL-92
//! does not, and PRECEDING, FOLLOWING, UNBOUNDED, EXCLUDE, TIES, OTHERS and
//! ROW, which neither reserves, are key words only where this syntax has
//! them, so a column, table or function may have any of those names. The
//! window functions of arguments but the aggregates are in `values.rs`.

mod values;

use super::Parser;
use super::query::PARTITION;
use crate::ast::{
    Exclusion, Frame, FrameBound, FrameUnits, RankKind, ValueExpression, WindowDefinition, WindowFunction, WindowKind, WindowOver,
    WindowSpecification,
};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};

/// OVER, a key word only after a function's ")" and before "(" or a name.
pub(super) const OVER: &str = "OVER";
/// WINDOW, a key word only before a name and AS.
pub(super) const WINDOW: &str = "WINDOW";
/// The words of a `<window frame clause>` that SQL-92 does not reserve.
const RANGE: &str = "RANGE";
const GROUPS: &str = "GROUPS";
const PRECEDING: &str = "PRECEDING";
const FOLLOWING: &str = "FOLLOWING";
const UNBOUNDED: &str = "UNBOUNDED";
const EXCLUDE: &str = "EXCLUDE";
const TIES: &str = "TIES";
const OTHERS: &str = "OTHERS";
const ROW: &str = "ROW";

/// ROW_NUMBER and the `<rank function type>`s, by name.
const RANKS: [(&str, WindowKind); 5] = [
    ("ROW_NUMBER", WindowKind::RowNumber),
    ("RANK", WindowKind::Rank(RankKind::Rank)),
    ("DENSE_RANK", WindowKind::Rank(RankKind::DenseRank)),
    ("PERCENT_RANK", WindowKind::Rank(RankKind::PercentRank)),
    ("CUME_DIST", WindowKind::Rank(RankKind::CumeDist)),
];

impl Parser {
    /// ROW_NUMBER or a `<rank function type>`, if one is at hand followed
    /// by ( ) OVER, so a function of that name stays callable otherwise.
    pub(super) fn at_rank_function(&self) -> Option<WindowKind> {
        let Tok::Identifier(name) = self.peek() else { return None };
        let (_, kind) = RANKS.into_iter().find(|(n, _)| name == n)?;
        let called = matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN))
            && matches!(self.peek_at(2), Tok::Delimiter(delim::RIGHT_PAREN))
            && matches!(self.peek_at(3), Tok::Identifier(s) if s == OVER);
        called.then_some(kind)
    }

    /// [ISO/IEC 9075-2:2023 §6.10] `<window function>` ::= `<window
    /// function type>` OVER `<window name or specification>`, of ROW_NUMBER
    /// or a rank function `kind`, at hand.
    pub(super) fn rank_function(&mut self, kind: WindowKind) -> Result<ValueExpression> {
        let at = self.here();
        // The name, "(" and ")" that `at_rank_function` saw, then OVER.
        for _ in 0_u32..4 {
            self.advance();
        }
        let over = self.window_over()?;
        Ok(ValueExpression::Window(Box::new(WindowFunction { kind, over, at })))
    }

    /// `primary`, written at `at`, as the `<aggregate function>` of a
    /// `<window function>` if OVER follows it and it is a set function or
    /// a call, which the binder finds an aggregate or not. OVER is read only
    /// before "(" or a window name, so it stays a select item's name.
    pub(super) fn windowed(&mut self, primary: ValueExpression, at: usize) -> Result<ValueExpression> {
        let aggregate = matches!(primary, ValueExpression::SetFunction(_) | ValueExpression::Call { .. });
        let follows = self.is_word(OVER) && matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN) | Tok::Identifier(_));
        if !(aggregate && follows) {
            return Ok(primary);
        }
        self.advance();
        let over = self.window_over()?;
        Ok(ValueExpression::Window(Box::new(WindowFunction { kind: WindowKind::Aggregate(Box::new(primary)), over, at })))
    }

    /// `<window name or specification>` ::= `<window name>` | `<in-line
    /// window specification>`, OVER taken.
    fn window_over(&mut self) -> Result<WindowOver> {
        if self.is_delim(delim::LEFT_PAREN) {
            return Ok(WindowOver::Specification(Box::new(self.window_specification()?)));
        }
        let at = self.here();
        Ok(WindowOver::Name(self.identifier()?, at))
    }

    /// Whether a `<window clause>` begins here: WINDOW, a name and AS.
    pub(super) fn at_window_clause(&self) -> bool {
        self.is_word(WINDOW) && matches!(self.peek_at(1), Tok::Identifier(_)) && matches!(self.peek_at(2), Tok::KeyWord(kw::AS))
    }

    /// [ISO/IEC 9075-2:2023 §7.15] `<window clause>` ::= WINDOW `<window
    /// definition>` \[{, `<window definition>`}...\], none if no clause
    /// begins here.
    pub(super) fn window_clause(&mut self) -> Result<Vec<WindowDefinition>> {
        let mut definitions = Vec::new();
        if !self.at_window_clause() {
            return Ok(definitions);
        }
        self.advance();
        loop {
            let at = self.here();
            let name = self.identifier()?;
            self.expect_kw(kw::AS)?;
            let specification = self.window_specification()?;
            definitions.push(WindowDefinition { name, specification, at });
            if !self.accept_delim(delim::COMMA) {
                return Ok(definitions);
            }
        }
    }

    /// `<window specification>` ::= ( \[`<existing window name>`\]
    /// \[`<window partition clause>`\] \[`<window order clause>`\]
    /// \[`<window frame clause>`\] )
    fn window_specification(&mut self) -> Result<WindowSpecification> {
        let at = self.here();
        self.expect_delim(delim::LEFT_PAREN)?;
        let existing = if self.at_existing_window_name() {
            let named = self.here();
            Some((self.identifier()?, named))
        } else {
            None
        };
        let mut partition = Vec::new();
        if self.at_partitioning() {
            self.advance();
            self.advance();
            partition.push(self.grouping_column()?);
            while self.accept_delim(delim::COMMA) {
                partition.push(self.grouping_column()?);
            }
        }
        let order = if self.accept_kw(kw::ORDER) {
            self.expect_kw(kw::BY)?;
            self.sort_specification_list()?
        } else {
            Vec::new()
        };
        let frame = if self.at_frame_units().is_some() { Some(self.frame()?) } else { None };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(WindowSpecification { existing, partition, order, frame, at })
    }

    /// Whether the name at hand is an `<existing window name>`: any name
    /// but PARTITION before BY, and RANGE or GROUPS only where a frame's
    /// extent does not follow.
    fn at_existing_window_name(&self) -> bool {
        if !matches!(self.peek(), Tok::Identifier(_)) || self.at_partitioning() {
            return false;
        }
        if self.at_frame_units().is_none() {
            return true;
        }
        match self.peek_at(1) {
            Tok::Delimiter(delim::RIGHT_PAREN) | Tok::KeyWord(kw::ORDER | kw::ROWS) => true,
            Tok::Identifier(s) => s == PARTITION || s == RANGE || s == GROUPS,
            Tok::KeyWord(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::Delimiter(_)
            | Tok::End => false,
        }
    }

    /// The `<window frame units>` at hand, if any.
    fn at_frame_units(&self) -> Option<FrameUnits> {
        if self.is_kw(kw::ROWS) {
            Some(FrameUnits::Rows)
        } else if self.is_word(RANGE) {
            Some(FrameUnits::Range)
        } else if self.is_word(GROUPS) {
            Some(FrameUnits::Groups)
        } else {
            None
        }
    }

    /// `<window frame clause>` ::= `<window frame units>` `<window frame
    /// extent>` \[`<window frame exclusion>`\], where `<window frame
    /// extent>` ::= `<window frame start>` | BETWEEN `<window frame bound
    /// 1>` AND `<window frame bound 2>`.
    fn frame(&mut self) -> Result<Frame> {
        let at = self.here();
        let units = self.at_frame_units().ok_or_else(|| self.syntax("expected ROWS, RANGE or GROUPS"))?;
        self.advance();
        let (start, end) = if self.accept_kw(kw::BETWEEN) {
            let start = self.frame_bound()?;
            self.expect_kw(kw::AND)?;
            (start, Some(self.frame_bound()?))
        } else {
            // <window frame start> ::= UNBOUNDED PRECEDING | <window frame
            // preceding> | CURRENT ROW
            let alone = self.here();
            let start = self.frame_bound()?;
            if matches!(start, FrameBound::Following(_) | FrameBound::UnboundedFollowing) {
                return Err(SqlError::at(
                    Code::Syntax,
                    alone,
                    "a <window frame start> is UNBOUNDED PRECEDING, a <window frame preceding> or CURRENT ROW [ISO/IEC 9075-2:2023 §7.15]",
                ));
            }
            (start, None)
        };
        let exclusion = if self.accept_word(EXCLUDE) { self.exclusion()? } else { Exclusion::NoOthers };
        Ok(Frame { units, start, end, exclusion, at })
    }

    /// `<window frame bound>` ::= UNBOUNDED PRECEDING | `<window frame
    /// preceding>` | CURRENT ROW | `<window frame following>` | UNBOUNDED
    /// FOLLOWING, an offset an `<unsigned value specification>`.
    fn frame_bound(&mut self) -> Result<FrameBound> {
        if self.accept_kw(kw::CURRENT) {
            return self.expect_word(ROW).map(|()| FrameBound::CurrentRow);
        }
        if self.accept_word(UNBOUNDED) {
            return self.direction(FrameBound::UnboundedPreceding, FrameBound::UnboundedFollowing);
        }
        if matches!(self.peek(), Tok::Delimiter(delim::PLUS | delim::MINUS)) {
            return Err(self.syntax("expected an <unsigned value specification> as a frame's offset [ISO/IEC 9075-2:2023 §7.15]"));
        }
        let offset = Box::new(ValueExpression::Value(self.value_specification()?));
        if self.accept_word(PRECEDING) {
            return Ok(FrameBound::Preceding(offset));
        }
        self.expect_word(FOLLOWING)?;
        Ok(FrameBound::Following(offset))
    }

    /// `preceding` or `following`, by the word at hand.
    fn direction(&mut self, preceding: FrameBound, following: FrameBound) -> Result<FrameBound> {
        if self.accept_word(PRECEDING) {
            return Ok(preceding);
        }
        self.expect_word(FOLLOWING).map(|()| following)
    }

    /// `<window frame exclusion>` ::= EXCLUDE CURRENT ROW | EXCLUDE GROUP |
    /// EXCLUDE TIES | EXCLUDE NO OTHERS, EXCLUDE taken.
    fn exclusion(&mut self) -> Result<Exclusion> {
        if self.accept_kw(kw::CURRENT) {
            return self.expect_word(ROW).map(|()| Exclusion::CurrentRow);
        }
        if self.accept_kw(kw::GROUP) {
            return Ok(Exclusion::Group);
        }
        if self.accept_word(TIES) {
            return Ok(Exclusion::Ties);
        }
        self.expect_kw(kw::NO)?;
        self.expect_word(OTHERS).map(|()| Exclusion::NoOthers)
    }

    /// The word `word`, not a key word of SQL-92, taken; an error if it is
    /// not at hand.
    fn expect_word(&mut self, word: &str) -> Result<()> {
        if self.accept_word(word) { Ok(()) } else { Err(self.syntax(format!("expected {word}"))) }
    }
}
