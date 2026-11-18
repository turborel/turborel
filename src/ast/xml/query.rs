//! Abstract syntax of the SQL/XML operators that evaluate XQuery
//! [ISO/IEC 9075-14:2006 draft §6.18, §7.1, §8.4]: XMLQUERY, XMLEXISTS
//! and XMLTABLE.

use super::super::{DefaultOption, TypeName, ValueExpression};
use super::{NamespaceItem, Passing, Returning};

/// [§6.18] `<XML query argument>`: a `<value expression>`, and the
/// `<identifier>` AS names it of an `<XML query variable>`, none of the
/// `<XML query context item>`; its `<XML passing mechanism>`, if written.
#[derive(Debug, Clone)]
pub struct XmlArgument {
    /// The `<value expression>`.
    pub value: ValueExpression,
    /// The variable's name, none of the context item.
    pub name: Option<String>,
    /// The `<XML passing mechanism>`, if written.
    pub passing: Option<Passing>,
    /// Where it begins in the statement.
    pub at: usize,
}

/// [§6.18] `<XML query argument list>` ::= PASSING `<XML query default
/// passing mechanism>` `<XML query argument>` \[ { , `<XML query
/// argument>` }... \].
#[derive(Debug, Clone)]
pub struct XmlArguments {
    /// The `<XML query default passing mechanism>`.
    pub default: Passing,
    /// The `<XML query argument>`s.
    pub arguments: Vec<XmlArgument>,
}

/// [§6.18] `<XML query empty handling option>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnEmpty {
    /// NULL ON EMPTY.
    Null,
    /// EMPTY ON EMPTY.
    Empty,
}

/// [§6.18] `<XML query>` ::= XMLQUERY ( `<XQuery expression>` \[`<XML
/// query argument list>`\] \[`<XML returning clause>` \[`<XML query
/// returning mechanism>`\]\] `<XML query empty handling option>` ); and
/// [§8.4] `<XML exists predicate>` ::= XMLEXISTS ( `<XQuery expression>`
/// \[`<XML query argument list>`\] ), of no clause after its arguments.
#[derive(Debug, Clone)]
pub struct XmlQuery {
    /// The `<XQuery expression>`, a `<character string literal>`'s value.
    pub text: String,
    /// The `<XML query argument list>`, if written.
    pub arguments: Option<XmlArguments>,
    /// The `<XML returning clause>`, if written.
    pub returning: Option<Returning>,
    /// The `<XML query returning mechanism>`, if written.
    pub mechanism: Option<Passing>,
    /// The `<XML query empty handling option>`, none of XMLEXISTS.
    pub empty: Option<OnEmpty>,
}

/// [§7.1] `<XML table column definition>`.
#[derive(Debug, Clone)]
pub enum XmlTableColumn {
    /// `<XML table ordinality column definition>` ::= `<column name>` FOR
    /// ORDINALITY.
    Ordinality(String),
    /// `<XML table regular column definition>`.
    Regular(XmlRegularColumn),
}

/// [§7.1] `<XML table regular column definition>` ::= `<column name>`
/// `<data type>` \[`<XML passing mechanism>`\] \[`<default clause>`\] \[
/// PATH `<XML table column pattern>` \].
#[derive(Debug, Clone)]
pub struct XmlRegularColumn {
    /// The `<column name>`.
    pub name: String,
    /// The `<data type>`, or a domain's name.
    pub data_type: TypeName,
    /// The `<XML passing mechanism>`, if written.
    pub passing: Option<Passing>,
    /// The `<default option>` of its `<default clause>`, if written.
    pub default: Option<DefaultOption>,
    /// The `<XML table column pattern>`, if written.
    pub path: Option<String>,
    /// Where it begins in the statement.
    pub at: usize,
}

/// [§7.1] `<XML table>` ::= XMLTABLE ( \[`<XML namespace declaration>` ,\]
/// `<XML table row pattern>` \[`<XML table argument list>`\] COLUMNS `<XML
/// table column definitions>` ).
#[derive(Debug, Clone)]
pub struct XmlTable {
    /// The `<XML namespace declaration>`'s items, if written.
    pub namespaces: Option<Vec<NamespaceItem>>,
    /// The `<XML table row pattern>`.
    pub pattern: String,
    /// The `<XML table argument list>`, if written.
    pub arguments: Option<XmlArguments>,
    /// The `<XML table column definitions>`.
    pub columns: Vec<XmlTableColumn>,
}

impl XmlArguments {
    /// Calls `f` on each value expression it holds.
    pub fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        self.arguments.iter().for_each(|a| f(&a.value));
    }
}
