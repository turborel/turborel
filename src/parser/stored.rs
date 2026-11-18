//! \[EXTENSION\] The text of a definition read again, and whether it takes
//! calls of registered functions (RULES.md D74).
//!
//! A definition keeps its text, read again each time it is used. It was
//! checked when made, so its calls of registered functions are taken
//! whatever SQL.FUNCTION_CALLS is when it is read. Only the definitions a
//! statement is making are checked against its session's setting. A
//! definition is made by binding the text it keeps, so while a statement
//! of a session that takes no such calls makes its definitions
//! (`Parser::refusing_calls`), a text read again takes none if its tokens
//! are written in the statement's. Any other text read then is of a
//! definition made before, its calls taken.

use std::cell::RefCell;

use super::{Catalogs, Parser, VALUE_COLUMN};
use crate::error::Result;
use crate::lexer::{Tok, kw, lex};

std::thread_local! {
    /// The tokens of the statement this thread is making the definitions
    /// of, if its session takes no calls of registered functions.
    static REFUSING: RefCell<Option<Vec<Tok>>> = const { RefCell::new(None) };
}

/// While held, the texts read again of the statement this thread makes
/// the definitions of take no calls of registered functions; dropped, as
/// before it was held.
#[must_use]
pub(crate) struct RefusingCalls(Option<Vec<Tok>>);

impl Drop for RefusingCalls {
    fn drop(&mut self) {
        let before = self.0.take();
        REFUSING.with(|r| *r.borrow_mut() = before);
    }
}

/// The tokens of `src`, without the end; none if it does not lex. The
/// column a domain constraint's VALUE is bound as is VALUE again
/// (`db/domains.rs`).
fn tokens(src: &str) -> Option<Vec<Tok>> {
    let value = |t: Tok| if t == Tok::Identifier(VALUE_COLUMN.to_owned()) { Tok::KeyWord(kw::VALUE) } else { t };
    let mut toks: Vec<Tok> = lex(src).ok()?.into_iter().map(|t| value(t.tok)).collect();
    toks.pop();
    Some(toks)
}

/// Whether `text`, read again, is written in the statement whose
/// definitions are made refusing calls of registered functions.
fn refused(text: &str) -> bool {
    REFUSING.with(|r| {
        let refusing = r.borrow();
        let Some(statement) = refusing.as_ref() else { return false };
        let Some(written) = tokens(text) else { return false };
        !written.is_empty() && statement.windows(written.len()).any(|w| w == written.as_slice())
    })
}

impl Parser {
    /// A parser of the text of a definition read again, as it was taken
    /// when defined (RULES.md D74). It takes any `<catalog name>` written
    /// in a name, its names checked then against the catalog's name. It
    /// takes the \[EXTENSION\] function call syntax whatever
    /// SQL.FUNCTION_CALLS is now, as a call refused then made no
    /// definition; but not in a text written in the statement whose
    /// definitions are being made refusing them (`refusing_calls`).
    ///
    /// # Errors
    ///
    /// As for `new`.
    pub fn for_stored(src: &str) -> Result<Parser> {
        Self::for_stored_of(src, src)
    }

    /// As `for_stored`, of `src` written around the text `kept` that a
    /// definition keeps, which is what is told written in the statement.
    ///
    /// # Errors
    ///
    /// As for `new`.
    pub(crate) fn for_stored_of(src: &str, kept: &str) -> Result<Parser> {
        let mut parser = Self::with_extensions(src, !refused(kept))?;
        parser.catalogs = Catalogs::Kept;
        Ok(parser)
    }

    /// Makes the definitions of the statement `statement`, of a session
    /// that takes calls of registered functions only if `taken`. While
    /// held, if not, a text read again that it writes takes none, as it
    /// was read when the session ran it.
    pub(crate) fn refusing_calls(statement: &str, taken: bool) -> RefusingCalls {
        let toks = if taken { None } else { tokens(statement) };
        RefusingCalls(REFUSING.with(|r| std::mem::replace(&mut *r.borrow_mut(), toks)))
    }
}
