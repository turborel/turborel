//! Sequence generators [ISO/IEC 9075-2:2023 §6.14, §11.72 to §11.74]:
//! their definition, alteration and `<next value expression>`. SEQUENCE,
//! START, INCREMENT, MAXVALUE, MINVALUE, CYCLE and RESTART are
//! `<non-reserved word>`s, identifiers elsewhere.

use super::Parser;
use crate::ast::{AlterSequence, SequenceDefinition, SequenceOptions, SignedNumber, TableName, ValueExpression};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};
use crate::value::{Decimal, MAX_EXACT_PRECISION};

/// The `<non-reserved word>` that names a sequence generator.
pub const SEQUENCE: &str = "SEQUENCE";

/// The option a word begins, read by `Parser::sequence_option`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opt {
    Start,
    Increment,
    Maximum,
    Minimum,
    Cycle,
    Restart,
}

impl Opt {
    /// How it is written, for an error.
    fn written(self) -> &'static str {
        match self {
            Opt::Start => "START WITH",
            Opt::Increment => "INCREMENT BY",
            Opt::Maximum => "MAXVALUE",
            Opt::Minimum => "MINVALUE",
            Opt::Cycle => "CYCLE",
            Opt::Restart => "RESTART",
        }
    }
}

impl Parser {
    /// Whether SEQUENCE is the token after the one at hand.
    pub(super) fn next_is_sequence(&self) -> bool {
        matches!(self.peek_at(1), Tok::Identifier(w) if w == SEQUENCE)
    }

    /// [ISO/IEC 9075-2:2023 §11.72] `<sequence generator definition>` ::=
    /// CREATE SEQUENCE `<sequence generator name>` \[`<sequence generator
    /// options>`\], each option written at most once \[SR5\].
    pub(super) fn sequence_definition(&mut self) -> Result<SequenceDefinition> {
        self.expect_kw(kw::CREATE)?;
        self.advance();
        let name = self.table_name()?;
        let (mut data_type, mut options) = (None, SequenceOptions::default());
        loop {
            if self.is_kw(kw::AS) {
                let at = self.here();
                if data_type.is_some() {
                    return Err(twice(at, "AS"));
                }
                self.advance();
                data_type = Some((self.data_type()?, at));
            } else if !self.sequence_option(&mut options, false)? {
                break;
            }
        }
        Ok(SequenceDefinition { name, data_type, options })
    }

    /// [ISO/IEC 9075-2:2023 §11.73] `<alter sequence generator statement>`
    /// ::= ALTER SEQUENCE `<sequence generator name>` `<alter sequence
    /// generator option>`..., each written at most once [§9.37 SR4].
    pub(super) fn alter_sequence(&mut self) -> Result<AlterSequence> {
        self.expect_kw(kw::ALTER)?;
        self.advance();
        let name = self.table_name()?;
        let mut options = SequenceOptions::default();
        if !self.sequence_option(&mut options, true)? {
            return Err(self.syntax("expected an <alter sequence generator option>"));
        }
        while self.sequence_option(&mut options, true)? {}
        Ok(AlterSequence { name, options })
    }

    /// One option of a sequence generator, if one is next, added to
    /// `options`: START WITH of a definition, RESTART of an alteration
    /// (`altering`). Whether one was.
    fn sequence_option(&mut self, options: &mut SequenceOptions, altering: bool) -> Result<bool> {
        let at = self.here();
        let Some((opt, negated)) = self.sequence_word(altering) else { return Ok(false) };
        let seen = match opt {
            Opt::Start => options.start.is_some(),
            Opt::Increment => options.increment.is_some(),
            Opt::Maximum => options.maximum.is_some(),
            Opt::Minimum => options.minimum.is_some(),
            Opt::Cycle => options.cycle.is_some(),
            Opt::Restart => options.restart.is_some(),
        };
        if seen {
            return Err(twice(at, opt.written()));
        }
        match opt {
            Opt::Start => options.start = Some(self.sequence_value(kw::WITH)?),
            Opt::Increment => options.increment = Some(self.sequence_value(kw::BY)?),
            Opt::Maximum => options.maximum = Some(self.bound(negated)?),
            Opt::Minimum => options.minimum = Some(self.bound(negated)?),
            Opt::Cycle => options.cycle = Some(!negated),
            Opt::Restart => options.restart = Some(self.restart_value()?),
        }
        Ok(true)
    }

    /// The option the words next begin, if any, read, and whether NO was
    /// before it, as it may be before MAXVALUE, MINVALUE or CYCLE. START
    /// WITH is of a definition, RESTART of an alteration (`altering`).
    fn sequence_word(&mut self, altering: bool) -> Option<(Opt, bool)> {
        let words = [
            ("START", Opt::Start),
            ("INCREMENT", Opt::Increment),
            ("MAXVALUE", Opt::Maximum),
            ("MINVALUE", Opt::Minimum),
            ("CYCLE", Opt::Cycle),
            ("RESTART", Opt::Restart),
        ];
        let offered = |opt: Opt| match opt {
            Opt::Start => !altering,
            Opt::Restart => altering,
            Opt::Increment | Opt::Maximum | Opt::Minimum | Opt::Cycle => true,
        };
        let negated = self.is_kw(kw::NO);
        let Tok::Identifier(word) = self.peek_at(usize::from(negated)) else { return None };
        let (_, opt) = words.into_iter().find(|(w, opt)| w == word && offered(*opt))?;
        if negated && !matches!(opt, Opt::Maximum | Opt::Minimum | Opt::Cycle) {
            return None;
        }
        if negated {
            self.advance();
        }
        self.advance();
        Some((opt, negated))
    }

    /// The `<signed numeric literal>` after the key word `then`.
    fn sequence_value(&mut self, then: &str) -> Result<SignedNumber> {
        self.expect_kw(then)?;
        self.signed_number()
    }

    /// The `<signed numeric literal>` of MAXVALUE or MINVALUE, its word
    /// read; none of NO MAXVALUE or NO MINVALUE (`negated`).
    fn bound(&mut self, negated: bool) -> Result<Option<SignedNumber>> {
        if negated { Ok(None) } else { self.signed_number().map(Some) }
    }

    /// \[WITH `<sequence generator restart value>`\], after RESTART.
    fn restart_value(&mut self) -> Result<Option<SignedNumber>> {
        if self.accept_kw(kw::WITH) { self.signed_number().map(Some) } else { Ok(None) }
    }

    /// [ISO/IEC 9075-2:2023 §5.3] `<signed numeric literal>` ::= \[`<sign>`\]
    /// `<unsigned numeric literal>`.
    fn signed_number(&mut self) -> Result<SignedNumber> {
        let at = self.here();
        let negative = self.is_delim(delim::MINUS);
        if negative || self.is_delim(delim::PLUS) {
            self.advance();
        }
        if !matches!(self.peek(), Tok::ExactNumeric(_) | Tok::ApproximateNumeric(_)) {
            return Err(self.syntax("expected a <signed numeric literal>"));
        }
        let value = match self.literal()? {
            crate::ast::Literal::Exact(d) => Some(d),
            crate::ast::Literal::Approximate(x) => exact_of(x),
            crate::ast::Literal::Character(_)
            | crate::ast::Literal::Binary(_)
            | crate::ast::Literal::Datetime(_)
            | crate::ast::Literal::Interval(_)
            | crate::ast::Literal::Boolean(_) => None,
        };
        Ok(SignedNumber { value: value.map(|d| if negative { d.negated() } else { d }), at })
    }

    /// [ISO/IEC 9075-2:2023 §6.14] `<next value expression>` ::= NEXT VALUE
    /// FOR `<sequence generator name>`.
    pub(super) fn next_value_expression(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.expect_kw(kw::NEXT)?;
        self.expect_kw(kw::VALUE)?;
        self.expect_kw(kw::FOR)?;
        let name: TableName = self.table_name()?;
        Ok(ValueExpression::NextValue { name, at })
    }
}

/// [ISO/IEC 9075-2:2023 §9.36 SR3, §9.37 SR4, §11.72 SR5] An option
/// written twice.
fn twice(at: usize, written: &str) -> SqlError {
    SqlError::at(
        Code::SyntaxRule,
        at,
        format!("[ISO/IEC 9075-2:2023 §9.36 SR3, §9.37 SR4, §11.72 SR5] {written} is written at most once of a sequence generator"),
    )
}

/// The integer `x` is, if it is one an exact number holds: else none, a
/// value no exact numeric type of scale 0 represents [§9.36 SR10].
fn exact_of(x: f64) -> Option<Decimal> {
    if !x.is_finite() || x.fract().abs() > 0.0_f64 {
        return None;
    }
    let text = format!("{x:.0}");
    if u32::try_from(text.trim_start_matches('-').len()).ok()? > MAX_EXACT_PRECISION {
        return None;
    }
    text.parse::<i128>().ok().map(|units| Decimal::new(units, 0))
}
