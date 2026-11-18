//! Abstract syntax of the SQL control statements of SQL/PSM [ISO/IEC
//! 9075-4:2006 draft §11.2, Clause 14] and of its SQL diagnostics
//! statements SIGNAL and RESIGNAL [§17.2, §17.3] (Feature P002).
//!
//! A statement list holds `SqlStatement`s, so a control statement nests
//! any SQL procedure statement, control statements among them.

use super::{
    ConditionItem, CursorSpecification, DefaultOption, ParameterSpecification, RowValue, SearchCondition, SqlStatement, TypeName,
    ValueExpression, WhenOperand,
};

/// [ISO/IEC 9075-4:2006 draft §14.1] `<statement label>` ::=
/// `<identifier>`, and where it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// The `<identifier>`.
    pub name: String,
    /// Where it begins.
    pub at: usize,
}

/// [ISO/IEC 9075-4:2006 draft §11.2] `<SQL control statement>`, beside the
/// CALL and RETURN of ISO/IEC 9075-2.
#[derive(Debug, Clone)]
pub enum ControlStatement {
    /// [§14.5] `<assignment statement>`.
    Assignment(Assignment),
    /// [§14.1] `<compound statement>`.
    Compound(CompoundStatement),
    /// [§14.6] `<case statement>`.
    Case(CaseStatement),
    /// [§14.7] `<if statement>`.
    If(IfStatement),
    /// [§14.8] `<iterate statement>` ::= ITERATE `<statement label>`.
    Iterate(Label),
    /// [§14.9] `<leave statement>` ::= LEAVE `<statement label>`.
    Leave(Label),
    /// [§14.10 to §14.12] `<loop statement>`, `<while statement>` or
    /// `<repeat statement>`.
    Iterated(Iterated),
    /// [§14.13] `<for statement>`.
    For(ForStatement),
}

/// [ISO/IEC 9075-4:2006 draft §14.1] `<compound statement>` ::= \[`<beginning
/// label>` `<colon>`\] BEGIN \[\[NOT\] ATOMIC\] \[`<local declaration
/// list>`\] \[`<local cursor declaration list>`\] \[`<local handler
/// declaration list>`\] \[`<SQL statement list>`\] END \[`<ending label>`\]
#[derive(Debug, Clone)]
pub struct CompoundStatement {
    /// The `<beginning label>`, if written.
    pub label: Option<Label>,
    /// ATOMIC was written; NOT ATOMIC is the default \[SR5\].
    pub atomic: bool,
    /// [ISO/IEC 9075-14:2006 draft §15.1] DECLARE's `<XML lexically scoped
    /// options>`, if written: their scope is the compound statement.
    pub options: Option<super::LexicallyScopedOptions>,
    /// The `<local declaration>`s, in order: variables and conditions.
    pub declarations: Vec<LocalDeclaration>,
    /// The `<declare cursor>`s of its `<local cursor declaration list>`.
    pub cursors: Vec<LocalCursor>,
    /// The `<handler declaration>`s of its `<local handler declaration
    /// list>`.
    pub handlers: Vec<HandlerDeclaration>,
    /// Its `<SQL statement list>`.
    pub statements: Vec<SqlStatement>,
    /// The `<ending label>`, if written.
    pub ending: Option<Label>,
    /// Where BEGIN is written.
    pub at: usize,
}

/// [ISO/IEC 9075-4:2006 draft §14.1] `<local declaration>` ::= `<SQL
/// variable declaration>` | `<condition declaration>`
#[derive(Debug, Clone)]
pub enum LocalDeclaration {
    /// [§14.4] `<SQL variable declaration>`.
    Variable(VariableDeclaration),
    /// [§14.3] `<condition declaration>`.
    Condition(ConditionDeclaration),
}

/// [ISO/IEC 9075-4:2006 draft §14.4] `<SQL variable declaration>` ::=
/// DECLARE `<SQL variable name list>` `<data type>` \[`<default clause>`\]
#[derive(Debug, Clone)]
pub struct VariableDeclaration {
    /// Each `<SQL variable name>`, and where it is written.
    pub names: Vec<Label>,
    /// Its `<data type>`, or a `<domain name>`.
    pub data_type: TypeName,
    /// The `<default option>` of its `<default clause>`, if written.
    pub default: Option<DefaultOption>,
}

/// [ISO/IEC 9075-4:2006 draft §14.3] `<condition declaration>` ::= DECLARE
/// `<condition name>` CONDITION \[FOR `<sqlstate value>`\]
#[derive(Debug, Clone)]
pub struct ConditionDeclaration {
    /// The `<condition name>`.
    pub name: Label,
    /// The `<character string literal>` of its `<sqlstate value>`, if
    /// written, and where it is.
    pub sqlstate: Option<(String, usize)>,
}

/// [ISO/IEC 9075-4:2006 draft §12.1] A `<declare cursor>` of a `<local
/// cursor declaration list>`.
#[derive(Debug, Clone)]
pub struct LocalCursor {
    /// The `<cursor name>`.
    pub name: Label,
    /// INSENSITIVE was written.
    pub insensitive: bool,
    /// SCROLL was written.
    pub scroll: bool,
    /// The `<cursor specification>`.
    pub specification: CursorSpecification,
    /// The `<cursor specification>` as written.
    pub text: String,
}

/// [ISO/IEC 9075-4:2006 draft §14.2] `<handler type>` ::= CONTINUE | EXIT |
/// UNDO
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerType {
    /// CONTINUE: after the action, the statement after the one that raised.
    Continue,
    /// EXIT: after the action, the end of the compound statement.
    Exit,
    /// UNDO: the compound statement's changes undone, then as EXIT.
    Undo,
}

/// [ISO/IEC 9075-4:2006 draft §14.2] `<condition value>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionValue {
    /// `<sqlstate value>`: its `<character string literal>`.
    Sqlstate(String),
    /// A `<condition name>`.
    Name(String),
    /// SQLEXCEPTION.
    Exception,
    /// SQLWARNING.
    Warning,
    /// NOT FOUND.
    NotFound,
}

/// [ISO/IEC 9075-4:2006 draft §14.2] `<handler declaration>` ::= DECLARE
/// `<handler type>` HANDLER FOR `<condition value list>` `<handler
/// action>`
#[derive(Debug, Clone)]
pub struct HandlerDeclaration {
    /// The `<handler type>`.
    pub kind: HandlerType,
    /// The `<condition value list>`, each with where it is written.
    pub conditions: Vec<(ConditionValue, usize)>,
    /// The `<handler action>`: an `<SQL procedure statement>`.
    pub action: Box<SqlStatement>,
    /// Where DECLARE is written.
    pub at: usize,
}

/// [ISO/IEC 9075-4:2006 draft §14.5] `<assignment statement>` ::=
/// `<singleton variable assignment>` | `<multiple variable assignment>`
#[derive(Debug, Clone)]
pub struct Assignment {
    /// Each `<assignment target>`: one of a `<singleton variable
    /// assignment>`, of a `<multiple variable assignment>` one or more
    /// (Feature P006).
    pub targets: Vec<AssignmentTarget>,
    /// The `<assignment source>`, or the `<assigned row>` of a `<multiple
    /// variable assignment>`.
    pub source: AssignmentSource,
    /// Where SET is written.
    pub at: usize,
}

/// [ISO/IEC 9075-4:2006 draft §14.5] `<assignment target>` ::= `<target
/// specification>` | `<modified field reference>` | `<mutator reference>`,
/// which the binder tells apart by the type of each part.
#[derive(Debug, Clone)]
pub struct AssignmentTarget {
    /// The `<target specification>`: an SQL variable, an SQL parameter, a
    /// new transition variable's column, or an element of an array one of
    /// them holds.
    pub target: ParameterSpecification,
    /// Of a `<modified field reference>` or a `<mutator reference>`, each
    /// `<field name>` or `<method name>` after the target, the outermost
    /// first, and where it is written (Features T051, S023).
    pub fields: Vec<Label>,
}

/// [ISO/IEC 9075-4:2006 draft §14.5] `<assignment source>` ::= `<value
/// expression>` | `<contextually typed source>`; `<assigned row>`.
#[derive(Debug, Clone)]
pub enum AssignmentSource {
    /// A `<value expression>`.
    Value(ValueExpression),
    /// NULL, an `<implicitly typed value specification>`, where it is
    /// written.
    Null(usize),
    /// The `<assigned row>` of a `<multiple variable assignment>`, a
    /// `<contextually typed row value expression>`: a row value
    /// constructor's elements, or one subquery of a row.
    Row(RowValue, usize),
}

/// [ISO/IEC 9075-4:2006 draft §14.6] `<case statement>`.
#[derive(Debug, Clone)]
pub struct CaseStatement {
    /// The `<case operand>` of a `<simple case statement>`, a `<row value
    /// predicand>` (Feature P004); none of a `<searched case statement>`.
    pub operand: Option<RowValue>,
    /// Each WHEN: of a simple one, its `<when operand list>` (Feature
    /// P008); of a searched one, its `<search condition>`; and its `<SQL
    /// statement list>`.
    pub whens: Vec<(CaseWhen, Vec<SqlStatement>)>,
    /// The `<SQL statement list>` of the `<case statement else clause>`.
    pub otherwise: Option<Vec<SqlStatement>>,
    /// Where CASE is written.
    pub at: usize,
}

/// [ISO/IEC 9075-4:2006 draft §14.6] What a WHEN of a `<case statement>`
/// tests.
#[derive(Debug, Clone)]
pub enum CaseWhen {
    /// The `<when operand list>` of a `<simple case statement when clause>`.
    Operands(Vec<WhenOperand>),
    /// The `<search condition>` of a `<searched case statement when
    /// clause>`.
    Condition(SearchCondition),
}

/// [ISO/IEC 9075-4:2006 draft §14.7] `<if statement>` ::= IF `<search
/// condition>` THEN `<SQL statement list>` \[ELSEIF ...\]... \[ELSE `<SQL
/// statement list>`\] END IF
#[derive(Debug, Clone)]
pub struct IfStatement {
    /// The condition of IF, then of each ELSEIF, with its `<SQL statement
    /// list>`.
    pub branches: Vec<(SearchCondition, Vec<SqlStatement>)>,
    /// The `<SQL statement list>` of ELSE.
    pub otherwise: Option<Vec<SqlStatement>>,
    /// Where IF is written.
    pub at: usize,
}

/// [ISO/IEC 9075-4:2006 draft §14.10 to §14.12] How an iterated statement
/// repeats its `<SQL statement list>`.
#[derive(Debug, Clone)]
pub enum Iteration {
    /// [§14.10] LOOP: until it is left.
    Loop,
    /// [§14.11] WHILE `<search condition>` DO: while it is true.
    While(SearchCondition),
    /// [§14.12] REPEAT ... UNTIL `<search condition>`: until it is true,
    /// read after each time.
    Repeat(SearchCondition),
}

/// [ISO/IEC 9075-4:2006 draft §14.10 to §14.12] `<loop statement>`,
/// `<while statement>` or `<repeat statement>`.
#[derive(Debug, Clone)]
pub struct Iterated {
    /// The `<beginning label>`, if written.
    pub label: Option<Label>,
    /// How it repeats.
    pub iteration: Iteration,
    /// Its `<SQL statement list>`.
    pub statements: Vec<SqlStatement>,
    /// The `<ending label>`, if written.
    pub ending: Option<Label>,
    /// Where LOOP, WHILE or REPEAT is written.
    pub at: usize,
}

/// [ISO/IEC 9075-4:2006 draft §14.13] `<for statement>` ::= \[`<beginning
/// label>` `<colon>`\] FOR \[`<for loop variable name>` AS\] \[`<cursor
/// name>` \[`<cursor sensitivity>`\] CURSOR FOR\] `<cursor specification>`
/// DO `<SQL statement list>` END FOR \[`<ending label>`\]
#[derive(Debug, Clone)]
pub struct ForStatement {
    /// The `<beginning label>`, if written.
    pub label: Option<Label>,
    /// The `<for loop variable name>`, if written.
    pub variable: Option<Label>,
    /// The `<cursor name>`, if written.
    pub cursor: Option<Label>,
    /// INSENSITIVE was written as its `<cursor sensitivity>`.
    pub insensitive: bool,
    /// The `<cursor specification>`.
    pub specification: CursorSpecification,
    /// The `<cursor specification>` as written.
    pub text: String,
    /// Its `<SQL statement list>`.
    pub statements: Vec<SqlStatement>,
    /// The `<ending label>`, if written.
    pub ending: Option<Label>,
    /// Where FOR is written.
    pub at: usize,
}

/// [ISO/IEC 9075-4:2006 draft §17.2] `<signal value>` ::= `<condition
/// name>` | `<sqlstate value>`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalValue {
    /// A `<condition name>`.
    Name(Label),
    /// The `<character string literal>` of an `<sqlstate value>`, and
    /// where it is written.
    Sqlstate(String, usize),
}

/// [ISO/IEC 9075-4:2006 draft §17.2, §17.3] `<signal statement>` ::= SIGNAL
/// `<signal value>` \[`<set signal information>`\], or `<resignal
/// statement>` ::= RESIGNAL \[`<signal value>`\] \[`<set signal
/// information>`\]
#[derive(Debug, Clone)]
pub struct SignalStatement {
    /// RESIGNAL was written.
    pub resignal: bool,
    /// The `<signal value>`; of SIGNAL always one.
    pub value: Option<SignalValue>,
    /// Each `<signal information item>`: a `<condition information item
    /// name>` and its `<simple value specification>`.
    pub items: Vec<(ConditionItem, ValueExpression)>,
    /// Where SIGNAL or RESIGNAL is written.
    pub at: usize,
}

impl CompoundStatement {
    /// Each statement it holds directly: its handlers' actions, then its
    /// statement list.
    pub fn statements(&self) -> impl Iterator<Item = &SqlStatement> {
        self.handlers.iter().map(|h| &*h.action).chain(&self.statements)
    }
}

impl ControlStatement {
    /// Each `<SQL procedure statement>` it holds directly, in order.
    pub fn statements(&self) -> Vec<&SqlStatement> {
        match self {
            ControlStatement::Assignment(_) | ControlStatement::Iterate(_) | ControlStatement::Leave(_) => Vec::new(),
            ControlStatement::Compound(c) => c.statements().collect(),
            ControlStatement::Case(c) => c.whens.iter().flat_map(|(_, s)| s).chain(c.otherwise.iter().flatten()).collect(),
            ControlStatement::If(i) => i.branches.iter().flat_map(|(_, s)| s).chain(i.otherwise.iter().flatten()).collect(),
            ControlStatement::Iterated(l) => l.statements.iter().collect(),
            ControlStatement::For(f) => f.statements.iter().collect(),
        }
    }
}
