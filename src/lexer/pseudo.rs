//! \[EXTENSION\] The names of the pseudo-columns of node and edge tables
//! (§4.2), each `$` then a name, read as one `<regular identifier>`.

use super::{Scanner, Tok, is_identifier_part};
use crate::catalog::PSEUDO_COLUMNS;

impl Scanner {
    /// \[EXTENSION\] (§4.2) The name of a pseudo-column of a node or
    /// edge table, `$` then its name in any case, as one identifier, in
    /// upper case: here, if it is one.
    pub(super) fn pseudo_column(&mut self) -> Option<Tok> {
        if self.peek(0) != Some('$') {
            return None;
        }
        let rest: String = self.c.get(self.at.saturating_add(1)..)?.iter().take_while(|c| is_identifier_part(**c)).collect();
        let word = format!("${}", rest.to_uppercase());
        let name = PSEUDO_COLUMNS.iter().find(|n| **n == word)?;
        self.advance(word.chars().count());
        Some(Tok::Identifier((*name).to_owned()))
    }
}
