//! Abstract syntax: the parsed form of `<schema>` [X3.135-1986 §6], of the
//! `<SQL statement>`s and `<declare cursor>` [X3.135-1986 §7.3, §8], and of the
//! common elements they are built from [X3.135-1986 §5]. Each type is named
//! after the production it represents.
//! Comments give the BNF exactly as implemented, and alternatives not implemented under "Not implemented".

use crate::catalog::QualifiedName;
use crate::value::datetime::Written;
use crate::value::interval::Interval;
use crate::value::{Binary, Decimal};

/// [X3.135-1986 §5.4] `<table name>` ::= \[`<authorization identifier>`.\] `<table identifier>`
///
/// Also used for [X3.135-1986 §5.7] `<qualifier>` ::= `<table name>` | `<correlation name>`,
/// where a single identifier may be either one (resolved during binding).
#[derive(Debug, Clone)]
pub struct TableName {
    /// The `<authorization identifier>`, if written.
    pub auth: Option<String>,
    /// The `<table identifier>`, or a `<correlation name>` when this is a `<qualifier>`.
    pub ident: String,
    /// Where it begins in the statement: a character offset, for the
    /// positions of errors (`SqlError::or_at`).
    pub at: usize,
}

mod alter;
mod composite;
mod connection;
mod contents;
mod cursor;
mod delta;
mod diagnostics;
mod drop;
mod dynamic;
mod embedding;
mod expression;
mod extension;
mod generated;
mod gql;
mod graph;
mod graph_ddl;
mod hybrid_search;
mod insert;
mod joined;
mod json;
mod json_table;
mod merge;
mod modules;
mod ordered;
mod partitions;
mod pattern;
mod periods;
mod privileges;
mod psm;
mod ptf;
mod query;
mod references;
mod roles;
mod routines;
mod schema;
mod search_cycle;
mod sequences;
mod set_functions;
mod structured;
mod target;
mod transaction;
mod triggers;
mod user_types;
mod users;
mod window;
mod xml;

pub use alter::AlterAction;
pub use composite::{Composite, ValueOrder};
pub use connection::{ConnectionStatement, Disconnected, ServedStatement};
pub use contents::{AsSubquery, LikeClause};
pub use cursor::{DeclareCursor, FetchOrientation};
pub use delta::{DeltaResult, DeltaRows, DeltaTable, Given};
pub use diagnostics::{AllQualifier, ConditionItem, DiagnosticsInformation, GetDiagnostics, StatementItem};
pub use drop::{DropBehavior, DropObject};
pub use dynamic::{
    CopiedItem, CopyOptions, DescriptorItem, DynamicName, DynamicStatement, ExecuteStatement, ExtendedName, GetDescriptor, HeaderItem,
    Scope, SetDescriptor, UsingClause,
};
pub use embedding::{ModelDefinition, PipelineDefinition, VectorObject, VectorStatement};
pub use expression::{
    CaseExpression, ColumnSpecification, DatetimeFunction, ExtractField, Predicate, RowValue, SearchCondition, Sign, Subquery, TruthValue,
    ValueExpression, WhenOperand,
};
pub use extension::{ExtensionDefinition, ExtensionRead, ExtensionStatement};
pub use generated::{AlterColumnAction, ColumnGeneration, Overriding};
pub use gql::{
    EdgeTypeDefinition, ExecuteGql, GqlCall, GqlClause, GqlComposite, GqlCondition, GqlExpression, GqlGraphDefinition, GqlLinear, GqlPage,
    GqlProcedure, GqlRemoveItem, GqlRequest, GqlReset, GqlResult, GqlReturn, GqlReturnItem, GqlSession, GqlSetItem, GqlSetOperator,
    GqlSort, GqlSpan, GqlTable, GqlTransaction, GraphTypeDefinition, GraphTyping, NodeTypeDefinition, PropertyTypeDefinition,
};
pub use graph::{
    Cheapest, Directions, EdgePattern, ElementPattern, GRAPH_FUNCTIONS, GraphColumn, GraphFunction, GraphFunctionName, GraphHints,
    GraphLimits, GraphMeasure, GraphPattern, GraphPredicate, GraphReference, GraphRows, GraphTable, GraphVariable, LabelExpression,
    MatchClause, MatchMode, ORIENTATIONS, Orientation, OrientationForm, ParenthesizedPath, PathAlgorithm, PathEnd, PathExpression,
    PathFactor, PathPattern, PathPrefix, PathPrimary, PathTable, PathTerm, PropertySpec, Quantifier, QuantifierSugar, Restrictor, SameKind,
    SegmentFilter, Selector, VariableKind,
};
pub use graph_ddl::{
    AdjacencyIndex, AlterGraph, AlterGraphAction, DerivedProperty, ElementAction, ElementDefinition, EndpointDefinition, GraphDefinition,
    GraphModeOption, LabelDefinition, LabelIndex, PropertiesDefinition, ReachabilityIndex,
};
pub use hybrid_search::{Fusion, HybridSearch, SearchArgument, SearchValue};
pub use insert::{ColumnName, InsertSource, InsertStatement};
pub use joined::{JoinKind, JoinSpecification, JoinedTable, Partitioning};
pub use json::{
    IsJson, JsonAggregate, JsonArgument, JsonClauses, JsonCommon, JsonConstructor, JsonExpression, JsonFormat, JsonInput, JsonMember,
    JsonOperator, JsonOutput, JsonTyped, JsonValueBehavior,
};
pub use json_table::{
    JsonTable, JsonTableColumn, JsonTableColumnKind, JsonTableFormatted, JsonTableNamed, JsonTableNested, JsonTablePath, JsonTablePlan,
    JsonTablePlanClause,
};
pub use merge::{IntoProperty, MatchedAction, MergeInsert, MergeStatement, MergeWhen};
pub use modules::ModuleDefinition;
pub use ordered::{FetchFirst, FetchQuantity, OrderedQuery, RowCount};
pub use partitions::{BoundDatum, BoundSpec, PartitionBy, PartitionKeyPart, PartitionOfClause, TablePartitioning};
pub use pattern::{
    Definition, EmptyMatches, Logical, Measure, Navigation, PatternCommon, Physical, Quantified, RecognizedTable, RowPattern,
    RowPatternRecognition, RowsPerMatch, Semantics, SkipTo, Subset, WindowRecognition,
};
pub use periods::{PeriodDefinition, PeriodName, PeriodOperand, PeriodOperator, Portion, SYSTEM_TIME, SystemTimeKind, SystemTimeRead};
pub use query::{
    Correspondence, CursorSpecification, DerivedTable, GroupBy, GroupingColumn, GroupingElement, NamedTable, QueryExpression,
    QuerySpecification, RowElement, SelectItem, SelectList, SelectSublist, SetOperation, SetOperator, SortColumn, SortSpecification,
    TableExpression, TableReference, UnnestTable, Updatability, ValuesRow, WithElement, WithQuery, from_table_names,
};

pub use privileges::{Action, PrivilegeDefinition, PrivilegeObject, RevokeStatement};
pub use psm::{
    Assignment, AssignmentSource, AssignmentTarget, CaseStatement, CaseWhen, CompoundStatement, ConditionDeclaration, ConditionValue,
    ControlStatement, ForStatement, HandlerDeclaration, HandlerType, IfStatement, Iterated, Iteration, Label, LocalCursor,
    LocalDeclaration, SignalStatement, SignalValue, VariableDeclaration,
};
pub use ptf::{
    DescriptorColumn, GenericTable, ParameterType, PrivateParameter, PtfArgument, PtfDefinition, PtfInvocation, PtfTable, RoutineBody,
    TableArgument, TableArgumentProper, TableSemantics,
};
pub use references::{MatchType, References, ReferentialAction};
pub use roles::{Grantor, RoleStatement, SetRole};
pub use routines::{
    CallStatement, DataAccess, ParameterDeclaration, ParameterDefault, ParameterMode, ReturnStatement, RoutineDefinition,
    RoutineDesignator, RoutineKind, RoutineType, SqlArgument,
};
pub use schema::{
    AlterDomain, AlterDomainAction, AssertionDefinition, Check, CheckLevels, CollationDefinition, Collection, ColumnConstraint,
    ColumnConstraintKind, ColumnDefinition, ConstraintAttributes, DefaultOption, DomainDefinition, IMPLICIT_NULLS_DISTINCT, OptionValue,
    Schema, SchemaStatement, TableConstraint, TableConstraintKind, TableDefinition, TableElement, TableOptionValue, TableStorage,
    Temporary, TemporaryKind, TypeName, TypeParam, ViewDefinition,
};
pub use search_cycle::{CycleClause, SearchClause, SearchOrder};
pub use sequences::{AlterSequence, SequenceDefinition, SequenceOptions, SignedNumber};
pub use set_functions::{Listagg, Overflow, SetArg, SetFunctionSpecification};
pub use structured::{
    AlterGroup, AlterGroupAction, AlterTransform, AlterType, AlterTypeAction, AttributeDefinition, CastDefinition, ColumnOptions,
    MethodCharacteristics, MethodDefinition, MethodDesignator, MethodKind, MethodSignature, MethodSpecification, OrderingCategory,
    OrderingDefinition, OrderingForm, RefGeneration, RefRepresentation, StructuredExpression, TransformDefinition, TransformGroup,
    TransformGroupSetting, TransformKind, TypeList, TypedTable,
};
pub use target::{ElementPosition, ParameterSpecification, TargetKind};
pub use transaction::{AccessMode, IsolationLevel, SavepointStatement, TransactionModes};
pub use triggers::{ActionTime, Referencing, TriggerDefinition, TriggerEvent, TriggerLevel};
pub use user_types::{TypeDefinition, TypeOption};
pub use users::UserStatement;
pub use window::{
    Exclusion, Frame, FrameBound, FrameUnits, NestedWindow, OffsetFunction, RankKind, RowMarker, ValueFunction, Which, WindowDefinition,
    WindowFunction, WindowKind, WindowOver, WindowSpecification,
};
pub use xml::{
    ContentOption, Dcs, ElementConstructor, ForestConstructor, LexicallyScopedOptions, MapResults, MapSource, Named, NamespaceItem,
    OnEmpty, Passing, Returning, XQueryModuleDefinition, XmlArgument, XmlArguments, XmlMapping, XmlOperator, XmlQuery, XmlRegularColumn,
    XmlSchemaDefinition, XmlTable, XmlTableColumn, XmlValidAccording, XmlValidWhat, rows_of,
};

// ---------------------------------------------------------------------------
// [X3.135-1986 §5.6] <value specification>, [X3.135-1986 §5.2] <literal>
// ---------------------------------------------------------------------------

/// [X3.135-1986 §5.6] `<value specification>` ::= `<parameter specification>` | `<literal>` | USER
/// | CURRENT_USER | SESSION_USER | SYSTEM_USER | `<dynamic parameter specification>` [ISO/IEC 9075:1992 §6.2]
///
/// Not implemented: `<variable specification>` (embedded SQL).
#[derive(Debug, Clone)]
pub enum ValueSpecification {
    /// A `<parameter specification>`.
    Parameter(ParameterSpecification),
    /// A `<literal>`.
    Literal(Literal),
    /// USER, or another `<authorization identifier>`.
    User(UserFunction),
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §6.2] A `<dynamic parameter
    /// specification>`, `?`, of a prepared statement \[SR2\]: the `index`-th
    /// of its statement, from 0, written at `at`.
    Dynamic {
        /// Its place among the statement's, in the order written.
        index: usize,
        /// Where it begins in the statement.
        at: usize,
    },
}

/// [ISO/IEC 9075:1992 §6.2] The `<authorization identifier>` a `<general
/// value specification>` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserFunction {
    /// CURRENT_USER, or USER, which is it \[SR3\]: the current one \[GR9\].
    Current,
    /// SESSION_USER: the SQL-session's \[GR10\].
    Session,
    /// SYSTEM_USER: the operating system user's \[GR11\].
    System,
    /// [ISO/IEC 9075-2:2023 §6.4 GR8] CURRENT_ROLE: the current role name,
    /// or the null value (Feature T332).
    Role,
}

/// [X3.135-1986 §5.2; ISO/IEC 9075:1992 §5.3; ISO/IEC 9075-2:2023 §5.3]
/// `<literal>`: character string, numeric, binary string, Boolean, datetime
/// or interval
#[derive(Debug, Clone)]
pub enum Literal {
    /// A `<character string literal>`'s value.
    Character(String),
    /// An `<exact numeric literal>`.
    Exact(Decimal),
    /// An `<approximate numeric literal>`.
    Approximate(f64),
    /// A `<binary string literal>`'s value [ISO/IEC 9075-2:2023 §5.3].
    Binary(Binary),
    /// A `<datetime literal>`'s fields, as written [ISO/IEC 9075:1992
    /// §5.3]: of a displacement the SQL-session gives, if it writes none
    /// \[SR18\].
    Datetime(Written),
    /// An `<interval literal>`'s value, its sign applied [ISO/IEC
    /// 9075:1992 §5.3 GR5].
    Interval(Interval),
    /// [ISO/IEC 9075-2:2023 §5.3] A `<boolean literal>`: TRUE, FALSE, or
    /// none for UNKNOWN, the null value [§4.6.1].
    Boolean(Option<bool>),
}

// ---------------------------------------------------------------------------
// [X3.135-1986 §7.3] <SQL statement>, and [X3.135-1986 §8.3] <declare cursor>
// ---------------------------------------------------------------------------

/// [X3.135-1986 §7.3] `<SQL statement>` ::= `<close statement>` | `<commit statement>`
///   | `<delete statement: positioned>` | `<delete statement: searched>`
///   | `<fetch statement>` | `<insert statement>` | `<open statement>`
///   | `<rollback statement>` | `<select statement>`
///   | `<update statement: positioned>` | `<update statement: searched>`;
/// and the statements of ISO/IEC 9075:1992 and the \[EXTENSION\]s below.
#[derive(Debug, Clone)]
pub enum SqlStatement {
    /// [X3.135-1986 §8.1] `<close statement>` ::= CLOSE `<cursor name>`
    Close(String),
    /// [ISO/IEC 9075-2:2023 §19.10] `<set session collation statement>`: the
    /// `<collation specification>`, or none for SET NO COLLATION.
    SetCollation(Option<ValueSpecification>),
    /// [ISO/IEC 9075-2:2023 §19.13] `<set transform group statement>` ::=
    /// SET `<transform group characteristic>`.
    SetTransformGroup(TransformGroupSetting),
    /// [X3.135-1986 §8.2; ISO/IEC 9075:1992 §14.3] `<commit statement>` ::= COMMIT \[WORK\]. Also what the parser
    /// gives for a statement it read whole only to refuse at its end (`Parser::refuse_later`), never run.
    Commit,
    /// [X3.135-1986 §8.4] `<delete statement: positioned>` ::=
    ///     DELETE FROM `<table name>` WHERE CURRENT OF `<cursor name>`
    DeletePositioned {
        /// The `<table name>`.
        table: TableName,
        /// [ISO/IEC 9075-2:2023 §14.8] ONLY ( `<table name>` ) was written.
        only: bool,
        /// The `<cursor name>`, and where it is written.
        cursor: (String, usize),
    },
    /// [X3.135-1986 §8.5] `<delete statement: searched>` ::=
    ///     DELETE FROM `<table name>` \[WHERE `<search condition>`\]
    DeleteSearched {
        /// The `<table name>`.
        table: TableName,
        /// [ISO/IEC 9075-2:2023 §14.9] ONLY ( `<table name>` ) was written:
        /// the rows of no proper subtable \[GR3\].
        only: bool,
        /// [ISO/IEC 9075-2:2023 §14.9 SR7a] FOR PORTION OF, if written.
        portion: Option<Box<Portion>>,
        /// The `<search condition>`, if written.
        where_clause: Option<SearchCondition>,
        /// [ISO/IEC 9075-14:2006 draft §14.3] WITH's `<XML lexically scoped
        /// options>`, if written.
        xml: Option<Box<LexicallyScopedOptions>>,
    },
    /// [ISO/IEC 9075-2:2023 §14.12] `<merge statement>`.
    Merge(Box<MergeStatement>),
    /// [ISO/IEC 9075-2:2023 §14.10] `<truncate table statement>` ::=
    ///     TRUNCATE TABLE `<target table>` \[CONTINUE IDENTITY | RESTART IDENTITY\]
    Truncate {
        /// The `<table name>` of the `<target table>`.
        table: TableName,
        /// ONLY ( `<table name>` ) was written: the rows of no proper
        /// subtable \[GR5a\].
        only: bool,
        /// Whether RESTART IDENTITY is written \[SR5\].
        restart: bool,
    },
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §11.10] `<alter table statement>` ::=
    ///     ALTER TABLE `<table name>` `<alter table action>`
    AlterTable {
        /// The `<table name>`.
        table: TableName,
        /// The `<alter table action>`.
        action: AlterAction,
        /// The `<alter table action>` as written, which the log keeps.
        text: String,
    },
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §11.2, §11.18, §11.20, §11.27, §11.35] `<drop schema statement>`, `<drop
    /// table statement>`, `<drop view statement>`, `<drop domain statement>` or `<drop assertion statement>`: DROP
    /// {SCHEMA `<schema name>` | TABLE `<table name>` | VIEW `<table name>` | DOMAIN `<domain name>`} `<drop
    /// behavior>`, or DROP ASSERTION `<constraint name>`, held as RESTRICT
    Drop {
        /// What it names.
        object: DropObject,
        /// The `<drop behavior>`.
        behavior: DropBehavior,
    },
    /// \[EXTENSION\] A table, view, domain, assertion or privileges defined, a domain altered, or privileges revoked,
    /// outside a `<schema definition>` (`SchemaStatement`).
    Schema {
        /// The statement.
        statement: SchemaStatement,
        /// The statement as written, which the log keeps.
        text: String,
    },
    /// [X3.135-1986 §8.6; ISO/IEC 9075:1992 §13.3] `<fetch statement>` ::=
    /// FETCH \[\[`<fetch orientation>`\] FROM\] `<cursor name>` INTO `<fetch
    /// target list>`
    Fetch {
        /// The `<cursor name>`.
        cursor: String,
        /// The `<fetch orientation>`: NEXT if none is written \[SR1\].
        orientation: FetchOrientation,
        /// The `<fetch target list>`.
        targets: Vec<ParameterSpecification>,
    },
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §18.1] `<get diagnostics statement>`
    GetDiagnostics(GetDiagnostics),
    /// [ISO/IEC 9075-4:2006 draft §11.2] An `<SQL control statement>` other
    /// than CALL and RETURN (Feature P002).
    Control(Box<ControlStatement>),
    /// [ISO/IEC 9075-4:2006 draft §17.2, §17.3] `<signal statement>` or
    /// `<resignal statement>`.
    Signal(Box<SignalStatement>),
    /// [ISO/IEC 9075-2:2023 §16.1] `<call statement>`
    Call(CallStatement),
    /// [ISO/IEC 9075-2:2023 §16.2] `<return statement>`
    Return(ReturnStatement),
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §17] An `<SQL dynamic statement>`.
    Dynamic(Box<DynamicStatement>),
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §17.10, §17.11] EXECUTE or EXECUTE
    /// IMMEDIATE.
    Execute(Box<ExecuteStatement>),
    /// \[EXTENSION\] EXECUTE GQL (graph standard §9).
    ExecuteGql(ExecuteGql),
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §13.11, §20.1] `<temporary table
    /// declaration>`: a declared local temporary table, of kind
    /// `TemporaryKind::Declared`.
    DeclareTable(TableDefinition),
    /// [X3.135-1986 §8.7]
    Insert(InsertStatement),
    /// [X3.135-1986 §8.8] `<open statement>` ::= OPEN `<cursor name>`
    Open(String),
    /// [X3.135-1986 §8.9; ISO/IEC 9075:1992 §14.4] `<rollback statement>` ::= ROLLBACK \[WORK\]
    Rollback,
    /// [ISO/IEC 9075-2:2023 §17.5, §17.6, §17.8] SAVEPOINT, RELEASE
    /// SAVEPOINT, or ROLLBACK TO SAVEPOINT.
    Savepoint(SavepointStatement),
    /// [X3.135-1986 §8.10] `<select statement>` ::= SELECT \[ALL | DISTINCT\] `<select list>`
    ///     INTO `<select target list>` `<table expression>`
    Select {
        /// The query, without INTO.
        query: Box<QuerySpecification>,
        /// The `<select target list>`.
        targets: Vec<ParameterSpecification>,
    },
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §14.1] `<set transaction
    /// statement>` ::= SET TRANSACTION `<transaction mode>` \[{,
    /// `<transaction mode>`}...\]
    SetTransaction(TransactionModes),
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §16.1] SET CATALOG `<value specification>`
    SetCatalog(ValueSpecification),
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §16.2] `<set schema statement>` ::=
    /// SET SCHEMA `<value specification>`
    SetSchema(ValueSpecification),
    /// [ISO/IEC 9075:1992 §16.5] `<set local time zone statement>`: SET TIME
    /// ZONE, of an `<interval value expression>`, or LOCAL, none.
    SetTimeZone(Option<ValueExpression>),
    /// [ISO/IEC 9075-2:2023 §14.17, §14.18] FREE or HOLD LOCATOR.
    Locator {
        /// HOLD, not FREE.
        hold: bool,
        /// Its `<locator reference>`s, dynamic parameters.
        references: Vec<ValueSpecification>,
    },
    /// [ISO/IEC 9075:1992 §16.4] `<set session authorization identifier
    /// statement>` ::= SET SESSION AUTHORIZATION `<value specification>`
    SetSessionAuthorization(ValueSpecification),
    /// [ISO/IEC 9075-2:2023 §19.3] `<set role statement>`.
    SetRole(SetRole),
    /// [ISO/IEC 9075:1992 §15] A statement of connections, the SQL-agent's:
    /// a client runs it, never a session.
    Connection(ConnectionStatement),
    /// \[EXTENSION\] SET `<setting name>` {= | TO} {`<value
    /// specification>` | DEFAULT}: the session's own value of a setting
    /// (RULES.md D36), where `<setting name>` ::= `<identifier>` \[{.
    /// `<identifier>`}...\].
    SetSetting {
        /// The setting's name, its parts joined by periods.
        name: String,
        /// Its value, or none for DEFAULT: the database's again.
        value: Option<ValueSpecification>,
    },
    /// \[EXTENSION\] `<set database setting statement>` ::= SET DATABASE
    /// `<setting name>` {`<equals operator>` | TO} {`<value specification>`
    /// | DEFAULT}: the database's value of a setting (RULES.md D36), set by
    /// its owner (D43). ISO/IEC 9075:1992 has no such statement: its SET
    /// statements of a session [§16.1–§16.5] set no database's value.
    SetDatabase {
        /// The setting's name, its parts joined by periods.
        name: String,
        /// Its value, or none for DEFAULT: its default again.
        value: Option<ValueSpecification>,
    },
    /// \[EXTENSION\] CREATE INDEX, DROP INDEX or ALTER INDEX (RULES.md D34,
    /// D41).
    Index(IndexStatement),
    /// \[EXTENSION\] A statement of users (RULES.md D43).
    User(UserStatement),
    /// \[EXTENSION\] A statement that ends a connection a server serves, or
    /// cancels what it runs (RULES.md D42).
    Served(ServedStatement),
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §14.2] `<set constraints mode
    /// statement>` ::= SET CONSTRAINTS `<constraint name list>` {DEFERRED |
    /// IMMEDIATE}
    SetConstraints {
        /// The `<constraint name>`s, or none for ALL.
        names: Option<Vec<TableName>>,
        /// DEFERRED, rather than IMMEDIATE.
        deferred: bool,
    },
    /// [X3.135-1986 §8.11] `<update statement: positioned>` ::= UPDATE `<table name>`
    ///     SET `<set clause: positioned>` \[{,`<set clause: positioned>`}...\] WHERE CURRENT OF `<cursor name>`
    UpdatePositioned {
        /// The `<table name>`.
        table: TableName,
        /// [ISO/IEC 9075-2:2023 §14.13] ONLY ( `<table name>` ) was written.
        only: bool,
        /// The `<set clause>`s, in order.
        set: Vec<SetClause>,
        /// The `<cursor name>`, and where it is written.
        cursor: (String, usize),
        /// [ISO/IEC 9075-14:2006 draft §14.6] WITH's `<XML lexically scoped
        /// options>`, if written.
        xml: Option<Box<LexicallyScopedOptions>>,
    },
    /// [X3.135-1986 §8.12] `<update statement: searched>` ::= UPDATE `<table name>`
    ///     SET `<set clause: searched>` \[{,`<set clause: searched>`}...\] \[WHERE `<search condition>`\]
    UpdateSearched {
        /// The `<table name>`.
        table: TableName,
        /// [ISO/IEC 9075-2:2023 §14.14] ONLY ( `<table name>` ) was written:
        /// the rows of no proper subtable \[GR6\].
        only: bool,
        /// [ISO/IEC 9075-2:2023 §14.14 SR7a] FOR PORTION OF, if written.
        portion: Option<Box<Portion>>,
        /// The `<set clause>`s, in order.
        set: Vec<SetClause>,
        /// The `<search condition>`, if written.
        where_clause: Option<SearchCondition>,
        /// [ISO/IEC 9075-14:2006 draft §14.7] WITH's `<XML lexically scoped
        /// options>`, if written.
        xml: Option<Box<LexicallyScopedOptions>>,
    },
}

/// \[EXTENSION\] A statement of indexes (RULES.md D34, D41), run outside a
/// transaction, as the index is built while transactions write its table.
#[derive(Debug, Clone)]
pub enum IndexStatement {
    /// `<create index statement>` ::= CREATE INDEX `<index name>` ON
    /// `<table name>` ( `<key part>` \[{, `<key part>`}...\] ) \[USING
    /// `<method name>`\] \[WITH `<table options>`\], where `<key part>` ::=
    /// `<column name>` | ( `<value expression>` ) (RULES.md D92)
    Create {
        /// The `<index name>`, an `<identifier>`, and where it begins.
        name: (String, usize),
        /// The `<table name>`.
        table: TableName,
        /// The `<column name>`s, in order, and where each begins.
        columns: Vec<(String, usize)>,
        /// The `<value expression>`s, each its place among the key parts,
        /// its text as written and where it begins.
        expressions: Vec<(usize, String, usize)>,
        /// The method, as an engine is named, and its options.
        storage: TableStorage,
    },
    /// `<drop index statement>` ::= DROP INDEX `<index name>`
    Drop {
        /// The `<index name>`, and where it begins.
        name: (String, usize),
    },
    /// \[EXTENSION\] [GS §13.2] CREATE ADJACENCY INDEX ON GRAPH.
    Adjacency(Box<AdjacencyIndex>),
    /// \[EXTENSION\] CREATE LABEL INDEX ON GRAPH (RULES.md D149).
    Labels(Box<LabelIndex>),
    /// \[EXTENSION\] CREATE REACHABILITY INDEX ON GRAPH (RULES.md D150).
    Reachability(Box<ReachabilityIndex>),
    /// `<alter index statement>` ::= ALTER INDEX `<index name>` SET `<table options>`
    Alter {
        /// The `<index name>`, and where it begins.
        name: (String, usize),
        /// The `<table option>`s, in order.
        options: Vec<TableOptionValue>,
    },
    /// \[EXTENSION\] `ALTER INDEX <index name> <alter index maintenance>`
    /// (the vector design document §10.6, `parser/ddl/vector_index.rs`).
    Maintain {
        /// The `<index name>`, and where it begins.
        name: (String, usize),
        /// What is done.
        action: IndexMaintenance,
    },
}

/// \[EXTENSION\] `<alter index maintenance>` (the vector design document
/// §10.6).
#[derive(Debug, Clone)]
pub enum IndexMaintenance {
    /// REBUILD \[ONLINE\] \[PARAMETERS (...)\]: built again, of these
    /// parameters, each named as a parameter of CREATE VECTOR INDEX.
    Rebuild(Vec<TableOptionValue>),
    /// MERGE DELTA: its maintenance run now (`Index::maintain`).
    MergeDelta,
    /// LOAD: its structure taken into memory (`Index::reside`).
    Load,
    /// UNLOAD: its structure let go of memory.
    Unload,
}

/// [X3.135-1986 §8.11, §8.12] `<set clause>` ::= `<object column>` = `<update source>`
#[derive(Debug, Clone)]
pub struct SetClause {
    /// The `<object column>`.
    pub column: String,
    /// [ISO/IEC 9075-2:2023 §14.15] The `<attribute name>`s of a `<mutated
    /// set clause>` `<object column>` . `<method name>`..., in order: the
    /// attributes the update source is put in, by their mutators \[SR7\].
    pub attributes: Vec<String>,
    /// [ISO/IEC 9075-2:2023 §14.15] The `<simple value specification>` of
    /// an `<update target>` that is an array's element, if written.
    pub element: Option<ValueExpression>,
    /// The `<update source>`.
    pub value: UpdateSource,
    /// Where it begins in the statement: a character offset, for the
    /// positions of errors (`SqlError::or_at`).
    pub at: usize,
}

/// [ISO/IEC 9075:1992 §13.9] `<update source>` ::= `<value expression>` |
/// `<null specification>` | DEFAULT
#[derive(Debug, Clone)]
pub enum UpdateSource {
    /// A `<value expression>`.
    Value(ValueExpression),
    /// NULL.
    Null,
    /// DEFAULT: the column's default.
    Default,
}
