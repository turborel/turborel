//! \[EXTENSION\] GQL's data-modifying statements (graph standard §12.3):
//! INSERT of path patterns, SET and REMOVE of properties and labels, and
//! DELETE, DETACH DELETE and NODETACH DELETE of elements.

use super::super::Parser;
use crate::ast::{GqlClause, GqlRemoveItem, GqlSetItem};
use crate::error::Result;
use crate::lexer::{delim, kw};

// [ISO/IEC 39075:2024] The words of the data-modifying statements.
/// REMOVE.
const REMOVE: &str = "REMOVE";
/// DETACH, of DETACH DELETE.
const DETACH: &str = "DETACH";
/// NODETACH, of NODETACH DELETE.
const NODETACH: &str = "NODETACH";

impl Parser {
    /// The next data-modifying statement, if one is next.
    pub(super) fn gql_modifying(&mut self) -> Result<Option<GqlClause>> {
        if self.accept_kw(kw::INSERT) {
            let mut paths = vec![self.path_pattern()?];
            while self.accept_delim(delim::COMMA) {
                paths.push(self.path_pattern()?);
            }
            return Ok(Some(GqlClause::Insert(paths)));
        }
        if self.accept_kw(kw::SET) {
            return self.gql_set_items().map(|items| Some(GqlClause::Set(items)));
        }
        if self.accept_gql(REMOVE) {
            return self.gql_remove_items().map(|items| Some(GqlClause::Remove(items)));
        }
        let detach = self.accept_gql(DETACH);
        if !detach {
            self.accept_gql(NODETACH);
        }
        if !self.accept_kw(kw::DELETE) {
            if detach {
                return Err(self.syntax("[EXTENSION] expected DELETE"));
            }
            return Ok(None);
        }
        let mut items = Vec::new();
        loop {
            let at = self.here();
            items.push((self.identifier()?, at));
            if !self.accept_delim(delim::COMMA) {
                return Ok(Some(GqlClause::Delete { detach, items }));
            }
        }
    }

    /// `<set item>` \[, ...\].
    fn gql_set_items(&mut self) -> Result<Vec<GqlSetItem>> {
        let mut items = Vec::new();
        loop {
            items.push(self.gql_set_item()?);
            if !self.accept_delim(delim::COMMA) {
                return Ok(items);
            }
        }
    }

    /// `x.p = v`, `x = { p : v, ... }`, `x : L` or `x IS L`.
    fn gql_set_item(&mut self) -> Result<GqlSetItem> {
        let at = self.here();
        let variable = self.identifier()?;
        if self.accept_delim(delim::COLON) || self.accept_kw(kw::IS) {
            return Ok(GqlSetItem::Label { variable, label: self.identifier()?, at });
        }
        if self.accept_delim(delim::PERIOD) {
            let property = self.identifier()?;
            self.expect_delim(delim::EQUALS)?;
            return Ok(GqlSetItem::Property { variable, property, value: self.gql_expression()?, at });
        }
        self.expect_delim(delim::EQUALS)?;
        self.expect_delim(delim::LEFT_BRACE)?;
        let mut properties = Vec::new();
        while !self.accept_delim(delim::RIGHT_BRACE) {
            let name = self.identifier()?;
            self.expect_delim(delim::COLON)?;
            properties.push((name, self.gql_expression()?));
            if !self.is_delim(delim::RIGHT_BRACE) {
                self.expect_delim(delim::COMMA)?;
            }
        }
        Ok(GqlSetItem::All { variable, properties, at })
    }

    /// `<remove item>` \[, ...\]: `x.p`, `x : L` or `x IS L`.
    fn gql_remove_items(&mut self) -> Result<Vec<GqlRemoveItem>> {
        let mut items = Vec::new();
        loop {
            let at = self.here();
            let variable = self.identifier()?;
            if self.accept_delim(delim::COLON) || self.accept_kw(kw::IS) {
                items.push(GqlRemoveItem::Label(variable, self.identifier()?, at));
            } else {
                self.expect_delim(delim::PERIOD)?;
                items.push(GqlRemoveItem::Property(variable, self.identifier()?, at));
            }
            if !self.accept_delim(delim::COMMA) {
                return Ok(items);
            }
        }
    }
}
