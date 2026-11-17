//! \[EXTENSION\] The grammar of XMLMAP, which maps a table, a schema or
//! the catalog to XML and to an XML Schema document [ISO/IEC 9075-14:2006
//! draft §9.11, §9.14, §9.17], mappings Part 14 gives no syntax [§4.10.9
//! NOTE 13, §4.10.10 NOTE 14, §4.10.11 NOTE 15] (RULES.md D86).

use super::super::Parser;
use super::publish::{ABSENT, NIL};
use super::{ELEMENT, ENCODING};
use crate::ast::{MapResults, MapSource, ValueExpression, XmlMapping, XmlOperator, rows_of};
use crate::error::Result;
use crate::functions::BinaryEncoding;
use crate::functions::xml_map::Nulls;
use crate::lexer::{delim, kw};

/// `<XML map>`'s word.
pub(super) const XMLMAP: &str = "XMLMAP";
/// NULLS, of `<XML map nulls>`.
const NULLS: &str = "NULLS";
/// FOREST, of `<XML map form>`.
const FOREST: &str = "FOREST";
/// TARGETNAMESPACE, of `<XML map target namespace>`.
const TARGETNAMESPACE: &str = "TARGETNAMESPACE";
/// DATA, of `<XML map results>`.
const DATA: &str = "DATA";
/// METADATA, of `<XML map results>`.
const METADATA: &str = "METADATA";
/// BASE64, of `<XML map encoding>`.
const BASE64: &str = "BASE64";
/// HEX, of `<XML map encoding>`.
const HEX: &str = "HEX";

impl Parser {
    /// `<XML map>` ::= XMLMAP ( `<XML map source>` \[`<XML map nulls>`\]
    /// \[`<XML map form>`\] \[`<XML map target namespace>`\] \[`<XML map
    /// results>`\] \[`<XML map encoding>`\] ).
    pub(super) fn xml_map(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.expect_word(XMLMAP)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let source = self.map_source()?;
        let nulls = self.map_nulls()?;
        let forest = self.map_form()?;
        let target =
            if self.accept_word(TARGETNAMESPACE) { Some(self.character_string_literal("<XML map target namespace>")?) } else { None };
        let results = self.map_results()?;
        let encoding = self.map_encoding()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let mapping = XmlMapping { source, nulls, forest, target, results, encoding };
        Ok(ValueExpression::Xml(Box::new(XmlOperator::Map(mapping)), at))
    }

    /// `<XML map source>` ::= TABLE `<table name>` | SCHEMA `<schema
    /// name>` | CATALOG: a table as `SELECT * FROM` it, so its name is
    /// found as a `<table reference>`'s is; a schema's catalog the one
    /// there is [ISO/IEC 9075:1992 §5.4 SR20].
    fn map_source(&mut self) -> Result<MapSource> {
        if self.accept_kw(kw::TABLE) {
            let at = self.here();
            return Ok(MapSource::Table(Box::new(rows_of(self.table_name()?, at))));
        }
        if self.accept_kw(kw::SCHEMA) {
            let (catalog, name, at) = self.schema_name()?;
            if let Some(catalog) = catalog {
                self.catalog_named(&catalog, at)?;
            }
            return Ok(MapSource::Schema(name, at));
        }
        self.expect_kw(kw::CATALOG)?;
        Ok(MapSource::Catalog)
    }

    /// `<XML map nulls>` ::= NULLS { ABSENT | NIL }, if next.
    fn map_nulls(&mut self) -> Result<Option<Nulls>> {
        if !self.accept_word(NULLS) {
            return Ok(None);
        }
        if self.accept_word(ABSENT) {
            return Ok(Some(Nulls::Absent));
        }
        if self.accept_word(NIL) {
            return Ok(Some(Nulls::Nil));
        }
        Err(self.syntax("expected ABSENT or NIL of XMLMAP (RULES.md D86)"))
    }

    /// `<XML map form>` ::= AS { FOREST | ELEMENT }, if next: whether
    /// FOREST.
    fn map_form(&mut self) -> Result<Option<bool>> {
        if !self.accept_kw(kw::AS) {
            return Ok(None);
        }
        if self.accept_word(FOREST) {
            return Ok(Some(true));
        }
        if self.accept_word(ELEMENT) {
            return Ok(Some(false));
        }
        Err(self.syntax("expected FOREST or ELEMENT of XMLMAP (RULES.md D86)"))
    }

    /// `<XML map results>` ::= DATA | METADATA | DATA AND METADATA, if
    /// next.
    fn map_results(&mut self) -> Result<Option<MapResults>> {
        if self.accept_word(METADATA) {
            return Ok(Some(MapResults::Metadata));
        }
        if !self.accept_word(DATA) {
            return Ok(None);
        }
        if !self.accept_kw(kw::AND) {
            return Ok(Some(MapResults::Data));
        }
        self.expect_word(METADATA)?;
        Ok(Some(MapResults::DataAndMetadata))
    }

    /// `<XML map encoding>` ::= ENCODING { BASE64 | HEX }, if next.
    fn map_encoding(&mut self) -> Result<Option<BinaryEncoding>> {
        if !self.accept_word(ENCODING) {
            return Ok(None);
        }
        if self.accept_word(BASE64) {
            return Ok(Some(BinaryEncoding::Base64));
        }
        if self.accept_word(HEX) {
            return Ok(Some(BinaryEncoding::Hex));
        }
        Err(self.syntax("expected BASE64 or HEX of XMLMAP (RULES.md D86)"))
    }
}
