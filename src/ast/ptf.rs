//! Abstract syntax of polymorphic table functions [ISO/IEC 9075-2:2023
//! §4.35.4] (Feature B200): the `<generic table parameter type>` and
//! `<descriptor parameter type>` of their parameters, their `<returns
//! table type>` and `<polymorphic table function body>` [§11.60], and
//! their invocation in a `<PTF derived table>` [§7.6], its `<table
//! argument>`s, `<descriptor argument>`s and `<co-partition clause>`
//! [§10.4].

use super::{ColumnSpecification, NamedTable, RoutineDesignator, SqlStatement, Subquery, TableName, TypeName, ValueExpression};

/// [§11.60] `<parameter type>` ::= `<data type>` | `<generic table
/// parameter type>` | `<descriptor parameter type>`.
#[derive(Debug, Clone)]
pub enum ParameterType {
    /// A `<data type>`.
    Data(TypeName),
    /// A `<generic table parameter type>`.
    Table(GenericTable),
    /// DESCRIPTOR.
    Descriptor,
}

/// [§11.60 SR10g ii] `<generic table parameter type>` ::= TABLE \[`<pass
/// through option>`\] \[`<generic table semantics>`\], what is implicit
/// made explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenericTable {
    /// PASS THROUGH, not NO PASS THROUGH, implicit or written.
    pub pass_through: bool,
    /// Its row or set semantics.
    pub semantics: TableSemantics,
}

/// [§11.60] `<generic table semantics>`, WITH SET SEMANTICS KEEP ON EMPTY
/// if none is written [SR10g ii 3, 4].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableSemantics {
    /// WITH ROW SEMANTICS.
    Row,
    /// WITH SET SEMANTICS, and whether PRUNE ON EMPTY.
    Set {
        /// PRUNE ON EMPTY, not KEEP ON EMPTY.
        prune: bool,
    },
}

/// [§11.60] What a polymorphic table function's definition has that an
/// SQL routine's has not: its `<returns table type>` and its
/// `<polymorphic table function body>`.
#[derive(Debug, Clone)]
pub struct PtfDefinition {
    /// Its `<table function column list>`, if written: each column's name,
    /// type and where it begins; none of RETURNS ONLY PASS THROUGH.
    pub columns: Option<Vec<(String, TypeName, usize)>>,
    /// RETURNS ONLY PASS THROUGH.
    pub only_pass_through: bool,
    /// Its private parameters, of `<PTF private parameters>` \[SR10e\].
    pub private: Vec<PrivateParameter>,
    /// DESCRIBE WITH's procedure, if written.
    pub describe: Option<RoutineDesignator>,
    /// START WITH's procedure, if written.
    pub start: Option<RoutineDesignator>,
    /// FULFILL WITH's procedure.
    pub fulfill: RoutineDesignator,
    /// FINISH WITH's procedure, if written.
    pub finish: Option<RoutineDesignator>,
}

/// [§11.60 SR10e] An `<SQL parameter declaration>` of `<PTF private
/// parameters>`: a name, a `<data type>` and a default.
#[derive(Debug, Clone)]
pub struct PrivateParameter {
    /// Its `<SQL parameter name>`.
    pub name: String,
    /// Its `<data type>`.
    pub data_type: TypeName,
    /// Its `<parameter default>`, if written: its value expression, none
    /// of NULL; and its text.
    pub default: Option<(Option<ValueExpression>, String)>,
    /// Where it begins in the statement.
    pub at: usize,
}

/// [§11.60] `<routine body>`: an `<SQL routine spec>`, or a `<polymorphic
/// table function body>`.
#[derive(Debug, Clone)]
pub enum RoutineBody {
    /// The `<SQL procedure statement>` of an SQL routine.
    Sql(Box<SqlStatement>),
    /// A polymorphic table function's.
    Polymorphic(Box<PtfDefinition>),
}

impl RoutineBody {
    /// Its `<SQL procedure statement>`, of an SQL routine; none of a
    /// polymorphic table function, which has none.
    pub fn statement(&self) -> Option<&SqlStatement> {
        match self {
            RoutineBody::Sql(s) => Some(s),
            RoutineBody::Polymorphic(_) => None,
        }
    }
}

/// [§7.6] `<PTF derived table>` ::= TABLE ( `<routine invocation>` ),
/// then its `<correlation or recognition>`.
#[derive(Debug, Clone)]
pub struct PtfTable {
    /// The `<routine invocation>`.
    pub invocation: PtfInvocation,
    /// Its `<correlation name>`, if written: none of a function of RETURNS
    /// ONLY PASS THROUGH \[SR7d\].
    pub correlation: Option<String>,
    /// Its `<derived column list>`, if written.
    pub columns: Option<Vec<String>>,
    /// Where TABLE is written.
    pub at: usize,
}

/// [§10.4] A `<routine invocation>` whose arguments may be `<table
/// argument>`s and `<descriptor argument>`s.
#[derive(Debug, Clone)]
pub struct PtfInvocation {
    /// The `<schema name>`, if written.
    pub schema: Option<String>,
    /// The `<qualified identifier>`.
    pub name: String,
    /// Its `<SQL argument>`s, in order.
    pub arguments: Vec<PtfArgument>,
    /// Its `<co-partition clause>`: each `<co-partition specification>`'s
    /// `<range variable>`s, each with where it begins.
    pub copartition: Vec<Vec<(TableName, usize)>>,
    /// Where its name begins.
    pub at: usize,
    /// \[EXTENSION\] Whether the session took function calls as it was
    /// read: of one the binder reads as a function's invocation.
    pub extension: bool,
}

impl PtfInvocation {
    /// Calls `table` with each table or query its `<table argument>`s name
    /// and `subquery` with each subquery of its arguments, those of the
    /// `<table function invocation>`s among them too.
    pub fn for_each_operand<'a>(&'a self, table: &mut dyn FnMut(&'a NamedTable), subquery: &mut dyn FnMut(&'a Subquery)) {
        let mut todo = vec![self];
        while let Some(invocation) = todo.pop() {
            for argument in &invocation.arguments {
                match argument {
                    PtfArgument::Value(v) => v.for_each_subquery(subquery),
                    PtfArgument::Table(t) => match &t.proper {
                        TableArgumentProper::Named(n) => table(n),
                        TableArgumentProper::Query(q) => subquery(q),
                        TableArgumentProper::Invocation(i) => todo.push(i),
                    },
                    PtfArgument::Null(_) | PtfArgument::Descriptor(..) => {}
                }
            }
        }
    }

    /// Calls `each` with the `<table subquery>` of each of its `<table
    /// argument>`s that is one, those of the `<table function
    /// invocation>`s among them too.
    pub fn for_each_table_query<'a>(&'a self, each: &mut dyn FnMut(&'a Subquery)) {
        let mut todo = vec![self];
        while let Some(invocation) = todo.pop() {
            for argument in &invocation.arguments {
                let PtfArgument::Table(t) = argument else { continue };
                match &t.proper {
                    TableArgumentProper::Query(q) => each(q),
                    TableArgumentProper::Invocation(i) => todo.push(i),
                    TableArgumentProper::Named(_) => {}
                }
            }
        }
    }

    /// Its subqueries and those of the tables its `<table argument>`s name
    /// as `for_each_operand` gives them, the tables to `table`.
    pub fn subqueries<'a>(&'a self, table: &mut dyn FnMut(&'a NamedTable)) -> Vec<&'a Subquery> {
        let mut subqueries = Vec::new();
        self.for_each_operand(table, &mut |s| subqueries.push(s));
        subqueries
    }
}

/// [§10.4] An `<SQL argument>` of a polymorphic table function.
#[derive(Debug, Clone)]
pub enum PtfArgument {
    /// A `<value expression>`; of a generic table parameter, a `<routine
    /// invocation>` read as a `<table function invocation>` [SR12b ii].
    Value(ValueExpression),
    /// A `<table argument>`.
    Table(Box<TableArgument>),
    /// NULL, or DEFAULT, of a parameter of no default [§9.18 SR12e i 2];
    /// and where it is written.
    Null(usize),
    /// A `<descriptor argument>`: DESCRIPTOR ( its columns ), or none of
    /// CAST ( NULL AS DESCRIPTOR ); and where it begins.
    Descriptor(Option<Vec<DescriptorColumn>>, usize),
}

/// [§20.16] A `<descriptor column specification>`.
#[derive(Debug, Clone)]
pub struct DescriptorColumn {
    /// Its `<column name>`.
    pub name: String,
    /// Its `<data type>`, if written.
    pub data_type: Option<TypeName>,
    /// Where it begins.
    pub at: usize,
}

/// [§10.4] `<table argument>`.
#[derive(Debug, Clone)]
pub struct TableArgument {
    /// Its `<table argument proper>`.
    pub proper: TableArgumentProper,
    /// Its `<table argument correlation name>`, if written.
    pub correlation: Option<String>,
    /// Its `<table argument parenthesized derived column list>`, if written.
    pub columns: Option<Vec<String>>,
    /// Its `<table argument partitioning>`'s columns, if written.
    pub partition: Option<Vec<ColumnSpecification>>,
    /// Its `<table argument pruning>`, if written: whether PRUNE WHEN EMPTY.
    pub prune: Option<bool>,
    /// Its `<table argument ordering>`'s columns, each with whether DESC
    /// and its `<null ordering>`, if written: whether NULLS FIRST.
    pub order: Vec<(ColumnSpecification, bool, Option<bool>)>,
    /// Where it begins.
    pub at: usize,
}

/// [§10.4] `<table argument proper>`.
#[derive(Debug, Clone)]
pub enum TableArgumentProper {
    /// TABLE ( `<table or query name>` ): its correlation name and column
    /// list are the `<table argument>`'s, none of its own.
    Named(NamedTable),
    /// TABLE `<table subquery>`.
    Query(Box<Subquery>),
    /// A `<table function invocation>`.
    Invocation(Box<PtfInvocation>),
}
