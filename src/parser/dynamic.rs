//! [ISO/IEC 9075:1992 §17] Dynamic SQL in SQL text: the grammar of its
//! statements, and of the names of the prepared statements, cursors and
//! descriptor areas they use [§5.4], with the Syntax Rules the text alone
//! decides. A session runs them (`db/dynamic.rs`, RULES.md D57). Split
//! from parser.rs (RULES.md R-SIZE-2); the methods extend the same
//! `Parser`.
//!
//! A `<simple value specification>` or `<simple target specification>` of
//! these statements is a parameter, written as SQL-86 writes one, an
//! `<identifier>` declared, without an indicator [§6.2], or, of a value,
//! a `<literal>`; so an `<identifier>` that is a parameter's name, where
//! a name of a statement or cursor may stand, is the parameter, and makes
//! the name an extended one.

use super::Parser;
use crate::ast::{
    DescriptorItem, DynamicName, DynamicStatement, ExecuteStatement, ExtendedName, FetchOrientation, GetDescriptor, ParameterSpecification,
    Scope, SetDescriptor, SqlStatement, TableName, UsingClause, ValueSpecification,
};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};

/// A `<using clause>` as read: whether INTO was written, rather than
/// USING; the clause; and where it begins.
type Using = (bool, UsingClause, usize);

/// [ISO/IEC 9075:1992 §17.5 SR3] The `<descriptor item name>`s a `<set
/// descriptor statement>` shall not set.
const NOT_SET: [DescriptorItem; 9] = [
    DescriptorItem::ReturnedLength,
    DescriptorItem::ReturnedOctetLength,
    DescriptorItem::OctetLength,
    DescriptorItem::Nullable,
    DescriptorItem::CollationCatalog,
    DescriptorItem::CollationSchema,
    DescriptorItem::CollationName,
    DescriptorItem::Name,
    DescriptorItem::Unnamed,
];

/// A dynamic statement, as the parser gives it.
fn dynamic(statement: DynamicStatement) -> SqlStatement {
    SqlStatement::Dynamic(Box::new(statement))
}

impl Parser {
    /// A Syntax Rule broken at `at`: SQLCODE -103.
    fn rule_at(at: usize, msg: String) -> SqlError {
        SqlError::at(Code::SyntaxRule, at, msg)
    }

    /// A `<parameter name>` [§5.4] as SQL-86 writes one: an `<identifier>`
    /// declared, without an indicator.
    fn parameter_name(&mut self) -> Result<ParameterSpecification> {
        let at = self.here();
        let name = self.identifier()?;
        if !self.params.contains(&name) {
            return Err(Self::rule_at(at, format!("[X3.135-1986 §7.3 SR3] parameter {name} is not declared")));
        }
        Ok(ParameterSpecification { name, indicator: None, at })
    }

    /// [§6.2] `<simple value specification>` ::= `<parameter name>` |
    /// `<embedded variable name>` | `<literal>`; an embedded variable is
    /// embedded SQL's, which is not implemented (SQLCODE -900).
    fn simple_value(&mut self) -> Result<ValueSpecification> {
        if self.is_delim(delim::COLON) {
            return Err(self.unsupported("<embedded variable name> (embedded SQL)"));
        }
        if self.at_parameter() {
            return self.parameter_name().map(ValueSpecification::Parameter);
        }
        self.literal().map(ValueSpecification::Literal)
    }

    /// [§6.2] `<simple target specification>` ::= `<parameter name>` |
    /// `<embedded variable name>`.
    fn simple_target(&mut self) -> Result<ParameterSpecification> {
        if self.is_delim(delim::COLON) {
            return Err(self.unsupported("<embedded variable name> (embedded SQL)"));
        }
        if !matches!(self.peek(), Tok::Identifier(_)) {
            return Err(self.syntax("expected a <simple target specification>"));
        }
        self.parameter_name()
    }

    /// [§5.4] `<descriptor name>`, or an extended name ::= \[`<scope
    /// option>`\] `<simple value specification>`.
    fn extended_name(&mut self) -> Result<ExtendedName> {
        let at = self.here();
        let scope = if self.accept_kw(kw::GLOBAL) {
            Scope::Global
        } else {
            self.accept_kw(kw::LOCAL);
            Scope::Local
        };
        Ok(ExtendedName { scope, value: self.simple_value()?, at })
    }

    /// [§5.4] An `<extended statement name>` or `<extended cursor name>`,
    /// whose value is no `<literal>` \[SR13\].
    fn extended_variable(&mut self) -> Result<ExtendedName> {
        let name = self.extended_name()?;
        if let ValueSpecification::Literal(_) = name.value {
            return Err(Self::rule_at(
                name.at,
                "[ISO/IEC 9075:1992 §5.4 SR13] an <extended statement name> or <extended cursor name> is not a <literal>".to_owned(),
            ));
        }
        Ok(name)
    }

    /// Whether a `<statement name>` or `<cursor name>` is next, an
    /// `<identifier>` not a parameter's, rather than an extended name.
    fn at_plain_name(&self) -> bool {
        matches!(self.peek(), Tok::Identifier(_)) && !self.at_parameter()
    }

    /// [§5.4] `<SQL statement name>` ::= `<statement name>` | `<extended
    /// statement name>`; so too `<dynamic cursor name>`.
    fn dynamic_name(&mut self) -> Result<DynamicName> {
        if self.at_plain_name() {
            let at = self.here();
            return Ok(DynamicName::Named(self.identifier()?, at));
        }
        self.extended_variable().map(DynamicName::Extended)
    }

    /// [§17.9] `<using clause>` ::= {USING | INTO} SQL DESCRIPTOR
    /// `<descriptor name>` | {USING | INTO} `<argument>` \[{, `<argument>`}...\],
    /// where `<argument>` ::= `<target specification>`; if one is next.
    fn using_clause(&mut self) -> Result<Option<Using>> {
        let at = self.here();
        let into = self.is_kw(kw::INTO);
        if !(self.accept_kw(kw::USING) || self.accept_kw(kw::INTO)) {
            return Ok(None);
        }
        if self.accept_kw(kw::SQL) {
            self.expect_kw(kw::DESCRIPTOR)?;
            return Ok(Some((into, UsingClause::Descriptor(self.extended_name()?), at)));
        }
        Ok(Some((into, UsingClause::Arguments(self.target_list()?), at)))
    }

    /// A `<using clause>` of USING alone, if one is next: \[SR1\] INTO only
    /// in a `<dynamic fetch statement>` or a `<result using clause>`.
    fn using_only(&mut self) -> Result<Option<UsingClause>> {
        match self.using_clause()? {
            None => Ok(None),
            Some((false, clause, _)) => Ok(Some(clause)),
            Some((true, _, at)) => Err(Self::rule_at(
                at,
                "[ISO/IEC 9075:1992 §17.9 SR1] INTO is of a <dynamic fetch statement> or a <result using clause> alone".to_owned(),
            )),
        }
    }

    /// [§17.4] A `<descriptor item name>`.
    fn descriptor_item(&mut self) -> Result<DescriptorItem> {
        let item = match self.peek() {
            Tok::Identifier(w) => DescriptorItem::named(w),
            Tok::KeyWord(w) => DescriptorItem::named(w),
            Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BitString(_)
            | Tok::Introducer(_)
            | Tok::Delimiter(_)
            | Tok::End => None,
        };
        let Some(item) = item else { return Err(self.syntax("expected a <descriptor item name>")) };
        self.advance();
        Ok(item)
    }

    /// [§17.2, §17.3, §17.7, §17.13] ALLOCATE DESCRIPTOR `<descriptor
    /// name>` \[WITH MAX `<occurrences>`\]; DEALLOCATE DESCRIPTOR
    /// `<descriptor name>`; DEALLOCATE PREPARE `<SQL statement name>`;
    /// ALLOCATE `<extended cursor name>` \[INSENSITIVE\] \[SCROLL\] CURSOR
    /// FOR `<extended statement name>`.
    pub(super) fn allocation(&mut self) -> Result<SqlStatement> {
        if self.accept_kw(kw::DEALLOCATE) {
            if self.accept_kw(kw::PREPARE) {
                return Ok(dynamic(DynamicStatement::DeallocatePrepare(self.dynamic_name()?)));
            }
            self.expect_kw(kw::DESCRIPTOR)?;
            return Ok(dynamic(DynamicStatement::DeallocateDescriptor(self.extended_name()?)));
        }
        self.expect_kw(kw::ALLOCATE)?;
        if self.accept_kw(kw::DESCRIPTOR) {
            let name = self.extended_name()?;
            let max = if self.accept_kw(kw::WITH) {
                self.expect_kw(kw::MAX)?;
                Some(self.simple_value()?)
            } else {
                None
            };
            return Ok(dynamic(DynamicStatement::AllocateDescriptor { name, max }));
        }
        let cursor = self.extended_variable()?;
        let insensitive = self.accept_kw(kw::INSENSITIVE);
        let scroll = self.accept_kw(kw::SCROLL);
        self.expect_kw(kw::CURSOR)?;
        self.expect_kw(kw::FOR)?;
        let statement = self.extended_variable()?;
        Ok(dynamic(DynamicStatement::AllocateCursor { cursor, insensitive, scroll, statement }))
    }

    /// [§17.4] GET DESCRIPTOR `<descriptor name>` {`<simple target
    /// specification 1>` = COUNT | VALUE `<item number>` `<simple target
    /// specification 2>` = `<descriptor item name>` \[{, ...}...\]}, after
    /// GET.
    pub(super) fn get_descriptor(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::DESCRIPTOR)?;
        let name = self.extended_name()?;
        if !self.accept_kw(kw::VALUE) {
            let target = self.simple_target()?;
            self.expect_delim(delim::EQUALS)?;
            self.expect_kw(kw::COUNT)?;
            return Ok(dynamic(DynamicStatement::GetDescriptor { name, info: GetDescriptor::Count(target) }));
        }
        let number = self.simple_value()?;
        let mut items = Vec::new();
        loop {
            let target = self.simple_target()?;
            self.expect_delim(delim::EQUALS)?;
            items.push((target, self.descriptor_item()?));
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        Ok(dynamic(DynamicStatement::GetDescriptor { name, info: GetDescriptor::Value { number, items } }))
    }

    /// [§17.5] SET DESCRIPTOR `<descriptor name>` {COUNT = `<simple value
    /// specification 1>` | VALUE `<item number>` `<descriptor item name>` =
    /// `<simple value specification 2>` \[{, ...}...\]}, after SET; of the
    /// items \[SR3\] allows, each once.
    pub(super) fn set_descriptor(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::DESCRIPTOR)?;
        let name = self.extended_name()?;
        if !self.accept_kw(kw::VALUE) {
            self.expect_kw(kw::COUNT)?;
            self.expect_delim(delim::EQUALS)?;
            return Ok(dynamic(DynamicStatement::SetDescriptor { name, info: SetDescriptor::Count(self.simple_value()?) }));
        }
        let number = self.simple_value()?;
        let mut items: Vec<(DescriptorItem, ValueSpecification)> = Vec::new();
        loop {
            let at = self.here();
            let item = self.descriptor_item()?;
            if NOT_SET.contains(&item) || items.iter().any(|(set, _)| *set == item) {
                let msg = format!("[ISO/IEC 9075:1992 §17.5 SR3] {} is not set here, or is set twice", item.name());
                return Err(Self::rule_at(at, msg));
            }
            self.expect_delim(delim::EQUALS)?;
            items.push((item, self.simple_value()?));
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        Ok(dynamic(DynamicStatement::SetDescriptor { name, info: SetDescriptor::Value { number, items } }))
    }

    /// [§17.6] PREPARE `<SQL statement name>` FROM `<SQL statement
    /// variable>`, no `<literal>` \[SR1\]; [§17.8] DESCRIBE \[INPUT |
    /// OUTPUT\] `<SQL statement name>` `<using descriptor>`; [§17.10]
    /// EXECUTE; [§17.11] EXECUTE IMMEDIATE `<SQL statement variable>`.
    pub(super) fn dynamic_statement(&mut self) -> Result<SqlStatement> {
        if self.accept_kw(kw::PREPARE) {
            let name = self.dynamic_name()?;
            self.expect_kw(kw::FROM)?;
            let at = self.here();
            let source = self.simple_value()?;
            if let ValueSpecification::Literal(_) = source {
                return Err(Self::rule_at(at, "[ISO/IEC 9075:1992 §17.6 SR1] an <SQL statement variable> is not a <literal>".to_owned()));
            }
            return Ok(dynamic(DynamicStatement::Prepare { name, source }));
        }
        if self.accept_kw(kw::DESCRIBE) {
            return self.describe();
        }
        self.expect_kw(kw::EXECUTE)?;
        let execute = if self.accept_kw(kw::IMMEDIATE) {
            ExecuteStatement::Immediate(self.simple_value()?)
        } else {
            let name = self.dynamic_name()?;
            let (result, parameters) = self.execute_clauses()?;
            ExecuteStatement::Prepared { name, result, parameters }
        };
        Ok(SqlStatement::Execute(Box::new(execute)))
    }

    /// [§17.8] The rest of a `<describe statement>`, after DESCRIBE.
    fn describe(&mut self) -> Result<SqlStatement> {
        let input = self.accept_kw(kw::INPUT);
        if !input {
            self.accept_kw(kw::OUTPUT);
        }
        let name = self.dynamic_name()?;
        let at = self.here();
        let into = self.accept_kw(kw::INTO);
        if !into {
            self.expect_kw(kw::USING)?;
        }
        self.expect_kw(kw::SQL)?;
        self.expect_kw(kw::DESCRIPTOR)?;
        let descriptor = self.extended_name()?;
        if into {
            let msg = "[ISO/IEC 9075:1992 §17.9 SR1] INTO is of a <dynamic fetch statement> or a <result using clause> alone";
            return Err(Self::rule_at(at, msg.to_owned()));
        }
        Ok(dynamic(DynamicStatement::Describe { input, name, descriptor }))
    }

    /// [§17.10] \[`<result using clause>`\] \[`<parameter using clause>`\]:
    /// the first of INTO \[SR2\], the second of USING [§17.9 SR1]; one
    /// alone is the one its key word says.
    fn execute_clauses(&mut self) -> Result<(Option<UsingClause>, Option<UsingClause>)> {
        let Some((into, first, at)) = self.using_clause()? else { return Ok((None, None)) };
        match (into, self.using_only()?) {
            (true, second) => Ok((Some(first), second)),
            (false, None) => Ok((None, Some(first))),
            (false, Some(_)) => Err(Self::rule_at(at, "[ISO/IEC 9075:1992 §17.10 SR2] a <result using clause> is of INTO".to_owned())),
        }
    }

    /// [§17.14, §17.16] OPEN or CLOSE of an `<extended cursor name>`, after
    /// the key word, OPEN's with a `<using clause>` of USING if written;
    /// none if a `<cursor name>` is next.
    pub(super) fn dynamic_open_or_close(&mut self, open: bool) -> Result<Option<SqlStatement>> {
        if self.at_plain_name() {
            return Ok(None);
        }
        let name = self.extended_variable()?;
        if !open {
            return Ok(Some(dynamic(DynamicStatement::Close(name))));
        }
        let using = self.using_only()?;
        Ok(Some(dynamic(DynamicStatement::Open { cursor: DynamicName::Extended(name), using })))
    }

    /// [§17.14] OPEN `<cursor name>` `<using clause>`, the cursor's name
    /// read: a dynamic OPEN if a using clause is next.
    pub(super) fn dynamic_open_using(&mut self, cursor: &str, at: usize) -> Result<Option<SqlStatement>> {
        let Some(using) = self.using_only()? else { return Ok(None) };
        let open = DynamicStatement::Open { cursor: DynamicName::Named(cursor.to_owned(), at), using: Some(using) };
        Ok(Some(dynamic(open)))
    }

    /// [§17.15] The `<dynamic cursor name>` and `<using clause>` of a
    /// dynamic FETCH, after FETCH \[\[`<fetch orientation>`\] FROM\]: an
    /// extended cursor name, or a cursor's with USING, or INTO SQL
    /// DESCRIPTOR; the `<using clause>` of INTO \[SR2\]. None if neither is
    /// next.
    pub(super) fn dynamic_fetch(&mut self, orientation: FetchOrientation) -> Result<std::result::Result<SqlStatement, FetchOrientation>> {
        let into_descriptor = matches!(self.peek_at(1), Tok::KeyWord(kw::INTO)) && matches!(self.peek_at(2), Tok::KeyWord(kw::SQL));
        let named_dynamic = matches!(self.peek_at(1), Tok::KeyWord(kw::USING)) || into_descriptor;
        if self.at_plain_name() && !named_dynamic {
            return Ok(Err(orientation));
        }
        let cursor = self.dynamic_name()?;
        match self.using_clause()? {
            Some((true, using, _)) => Ok(Ok(dynamic(DynamicStatement::Fetch { cursor, orientation, using }))),
            Some((false, _, at)) => {
                Err(Self::rule_at(at, "[ISO/IEC 9075:1992 §17.15 SR2] the <using clause> of a FETCH is of INTO".to_owned()))
            }
            None => Err(self.syntax("expected a <using clause>")),
        }
    }

    /// [§17.19, §17.20] The table a `<preparable dynamic delete statement:
    /// positioned>` or `<preparable dynamic update statement: positioned>`
    /// of no `<table name>` is of: that of the `<cursor specification>` of
    /// the cursor it names \[SR1\], which the statement's preparer found
    /// (`Parser::implying`); then WHERE CURRENT OF `<cursor name>`.
    pub(super) fn preparable_positioned(&mut self) -> Result<(TableName, (String, usize))> {
        self.expect_kw(kw::WHERE)?;
        self.expect_kw(kw::CURRENT)?;
        self.expect_kw(kw::OF)?;
        let at = self.here();
        let cursor = self.identifier()?;
        let Some(table) = self.implicit.clone() else {
            return Err(Self::rule_at(at, format!("[ISO/IEC 9075:1992 §17.19 SR1, §17.20 SR1] the table of cursor {cursor} is not known")));
        };
        Ok((table, (cursor, at)))
    }

    /// [§17.17, §17.18] WHERE CURRENT OF an `<extended cursor name>`,
    /// after CURRENT OF, if one is next.
    pub(super) fn dynamic_current_of(&mut self) -> Result<Option<ExtendedName>> {
        if self.at_plain_name() {
            return Ok(None);
        }
        self.extended_variable().map(Some)
    }
}

/// [ISO/IEC 9075:1992 §17.1 SR3] The `<data type>` `text` writes, the
/// fields of an item descriptor area written as one, if it is valid.
pub(crate) fn descriptor_type(text: &str) -> Option<crate::value::DataType> {
    let mut parser = Parser::new(text).ok()?;
    let data_type = parser.data_type().ok()?;
    parser.end().ok()?;
    Some(data_type)
}
