//! \[DESIGN\] Parsers of a text already lexed (`lexer::Lexed`): a
//! statement run directly is lexed once, for its normalized text
//! (`system/statements/text.rs`) and its parser alike.

use std::collections::{HashMap, HashSet};

use super::{Catalogs, Parser};
use crate::lexer::{Lexed, Tok, Token};

impl Parser {
    /// As `for_procedure`, of a text lexed as `lexed`.
    pub(crate) fn of_lexed(Lexed { toks, chars }: Lexed, extensions: bool, params: HashSet<String>) -> Parser {
        let end = Token { tok: Tok::End, start: chars.len(), end: chars.len() };
        Parser {
            toks,
            end,
            pos: 0,
            depth: 0,
            extensions,
            params,
            dynamic: None,
            questions: (0, 0),
            src: chars,
            in_domain: false,
            refused: None,
            catalogs: Catalogs::Refused,
            implicit: None,
            read_values: HashMap::new(),
            trigger: None,
            routine: None,
            blocks: Vec::new(),
            module: None,
            graph: 0,
            compat: Vec::new(),
            numbers: Vec::new(),
        }
    }

    /// As `for_prepare`, of a text lexed as `lexed`.
    pub(crate) fn prepare_lexed(lexed: Lexed, extensions: bool) -> Parser {
        let mut parser = Self::of_lexed(lexed, extensions, HashSet::new());
        parser.dynamic = Some(0);
        parser
    }
}
