//! Abstract syntax of dynamic SQL [ISO/IEC 9075:1992 §17]: its statements,
//! the names of prepared statements, cursors and descriptor areas they
//! use [§5.4], their `<using clause>`s [§17.9] and the `<descriptor item
//! name>`s [§17.4]. Split from ast.rs (RULES.md R-SIZE-2).

use super::{FetchOrientation, ParameterSpecification, SetClause, TableName, ValueSpecification};

/// [ISO/IEC 9075:1992 §5.4] `<scope option>` ::= GLOBAL | LOCAL: of an
/// extended name, LOCAL if none is written \[SR17\].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    /// LOCAL: of the module, here the session that plays its part.
    Local,
    /// GLOBAL: of the SQL-session.
    Global,
}

/// [ISO/IEC 9075:1992 §5.4] An `<extended statement name>`, `<extended
/// cursor name>` or `<descriptor name>` ::= \[`<scope option>`\] `<simple
/// value specification>`.
#[derive(Debug, Clone)]
pub struct ExtendedName {
    /// Its scope.
    pub scope: Scope,
    /// The `<simple value specification>` whose value is the name.
    pub value: ValueSpecification,
    /// Where it begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075:1992 §5.4] An `<SQL statement name>` ::= `<statement
/// name>` | `<extended statement name>`; or a `<dynamic cursor name>` ::=
/// `<cursor name>` | `<extended cursor name>`.
#[derive(Debug, Clone)]
pub enum DynamicName {
    /// A `<statement name>` or `<cursor name>`: an `<identifier>`, and
    /// where it is written.
    Named(String, usize),
    /// An extended name.
    Extended(ExtendedName),
}

/// [ISO/IEC 9075:1992 §17.9] `<using clause>` ::= `<using arguments>` |
/// `<using descriptor>`; USING or INTO as its Syntax Rule 1 allows where
/// it stands.
#[derive(Debug, Clone)]
pub enum UsingClause {
    /// `<using arguments>`: each `<argument>`, a `<target specification>`.
    Arguments(Vec<ParameterSpecification>),
    /// `<using descriptor>` ::= {USING | INTO} SQL DESCRIPTOR `<descriptor
    /// name>`.
    Descriptor(ExtendedName),
}

/// [ISO/IEC 9075:1992 §17.4] `<descriptor item name>`: the fields of an
/// item descriptor area [§17.1, Table 17].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DescriptorItem {
    /// TYPE.
    Type,
    /// LENGTH.
    Length,
    /// OCTET_LENGTH.
    OctetLength,
    /// RETURNED_LENGTH.
    ReturnedLength,
    /// RETURNED_OCTET_LENGTH.
    ReturnedOctetLength,
    /// PRECISION.
    Precision,
    /// SCALE.
    Scale,
    /// DATETIME_INTERVAL_CODE.
    DatetimeIntervalCode,
    /// DATETIME_INTERVAL_PRECISION.
    DatetimeIntervalPrecision,
    /// NULLABLE.
    Nullable,
    /// INDICATOR.
    Indicator,
    /// DATA.
    Data,
    /// NAME.
    Name,
    /// UNNAMED.
    Unnamed,
    /// COLLATION_CATALOG.
    CollationCatalog,
    /// COLLATION_SCHEMA.
    CollationSchema,
    /// COLLATION_NAME.
    CollationName,
    /// CHARACTER_SET_CATALOG.
    CharacterSetCatalog,
    /// CHARACTER_SET_SCHEMA.
    CharacterSetSchema,
    /// CHARACTER_SET_NAME.
    CharacterSetName,
}

/// [ISO/IEC 9075:1992 §17.4] Each `<descriptor item name>`, as written.
const ITEM_NAMES: [(&str, DescriptorItem); 20] = [
    ("TYPE", DescriptorItem::Type),
    ("LENGTH", DescriptorItem::Length),
    ("OCTET_LENGTH", DescriptorItem::OctetLength),
    ("RETURNED_LENGTH", DescriptorItem::ReturnedLength),
    ("RETURNED_OCTET_LENGTH", DescriptorItem::ReturnedOctetLength),
    ("PRECISION", DescriptorItem::Precision),
    ("SCALE", DescriptorItem::Scale),
    ("DATETIME_INTERVAL_CODE", DescriptorItem::DatetimeIntervalCode),
    ("DATETIME_INTERVAL_PRECISION", DescriptorItem::DatetimeIntervalPrecision),
    ("NULLABLE", DescriptorItem::Nullable),
    ("INDICATOR", DescriptorItem::Indicator),
    ("DATA", DescriptorItem::Data),
    ("NAME", DescriptorItem::Name),
    ("UNNAMED", DescriptorItem::Unnamed),
    ("COLLATION_CATALOG", DescriptorItem::CollationCatalog),
    ("COLLATION_SCHEMA", DescriptorItem::CollationSchema),
    ("COLLATION_NAME", DescriptorItem::CollationName),
    ("CHARACTER_SET_CATALOG", DescriptorItem::CharacterSetCatalog),
    ("CHARACTER_SET_SCHEMA", DescriptorItem::CharacterSetSchema),
    ("CHARACTER_SET_NAME", DescriptorItem::CharacterSetName),
];

impl DescriptorItem {
    /// The item `word` names, if it names one.
    pub fn named(word: &str) -> Option<DescriptorItem> {
        ITEM_NAMES.iter().find(|(name, _)| *name == word).map(|(_, item)| *item)
    }

    /// Its name, as written.
    pub fn name(self) -> &'static str {
        ITEM_NAMES.iter().find(|(_, item)| *item == self).map_or("", |(name, _)| *name)
    }
}

/// [ISO/IEC 9075:1992 §17.4] `<get descriptor information>`.
#[derive(Debug, Clone)]
pub enum GetDescriptor {
    /// `<get count>` ::= `<simple target specification 1>` = COUNT.
    Count(ParameterSpecification),
    /// VALUE `<item number>` `<get item information>` \[{, ...}...\].
    Value {
        /// The `<item number>`.
        number: ValueSpecification,
        /// Each `<simple target specification 2>` = `<descriptor item name>`.
        items: Vec<(ParameterSpecification, DescriptorItem)>,
    },
}

/// [ISO/IEC 9075:1992 §17.5] `<set descriptor information>`.
#[derive(Debug, Clone)]
pub enum SetDescriptor {
    /// `<set count>` ::= COUNT = `<simple value specification 1>`.
    Count(ValueSpecification),
    /// VALUE `<item number>` `<set item information>` \[{, ...}...\].
    Value {
        /// The `<item number>`.
        number: ValueSpecification,
        /// Each `<descriptor item name>` = `<simple value specification 2>`.
        items: Vec<(DescriptorItem, ValueSpecification)>,
    },
}

/// [ISO/IEC 9075:1992 §17.10, §17.11] An `<execute statement>` or an
/// `<execute immediate statement>`, which runs a statement prepared as a
/// statement of its own.
#[derive(Debug)]
pub enum ExecuteStatement {
    /// [§17.10] EXECUTE `<SQL statement name>` \[`<result using clause>`\]
    /// \[`<parameter using clause>`\].
    Prepared {
        /// The `<SQL statement name>`.
        name: DynamicName,
        /// The `<result using clause>`, of INTO, if written.
        result: Option<UsingClause>,
        /// The `<parameter using clause>`, of USING, if written.
        parameters: Option<UsingClause>,
    },
    /// [§17.11] EXECUTE IMMEDIATE `<SQL statement variable>`.
    Immediate(ValueSpecification),
}

/// [ISO/IEC 9075:1992 §17] An `<SQL dynamic statement>` [§12.5] but
/// EXECUTE and EXECUTE IMMEDIATE, and the `<dynamic declare cursor>`'s
/// statements: each as its Subclause gives it.
#[derive(Debug)]
pub enum DynamicStatement {
    /// [§17.2] ALLOCATE DESCRIPTOR `<descriptor name>` \[WITH MAX
    /// `<occurrences>`\].
    AllocateDescriptor {
        /// The `<descriptor name>`.
        name: ExtendedName,
        /// The `<occurrences>`, if written.
        max: Option<ValueSpecification>,
    },
    /// [§17.3] DEALLOCATE DESCRIPTOR `<descriptor name>`.
    DeallocateDescriptor(ExtendedName),
    /// [§17.4] GET DESCRIPTOR `<descriptor name>` `<get descriptor
    /// information>`.
    GetDescriptor {
        /// The `<descriptor name>`.
        name: ExtendedName,
        /// What it gets.
        info: GetDescriptor,
    },
    /// [§17.5] SET DESCRIPTOR `<descriptor name>` `<set descriptor
    /// information>`.
    SetDescriptor {
        /// The `<descriptor name>`.
        name: ExtendedName,
        /// What it sets.
        info: SetDescriptor,
    },
    /// [§17.6] PREPARE `<SQL statement name>` FROM `<SQL statement
    /// variable>`.
    Prepare {
        /// The `<SQL statement name>`.
        name: DynamicName,
        /// The `<SQL statement variable>`.
        source: ValueSpecification,
    },
    /// [§17.7] DEALLOCATE PREPARE `<SQL statement name>`.
    DeallocatePrepare(DynamicName),
    /// [§17.8] DESCRIBE \[INPUT | OUTPUT\] `<SQL statement name>` `<using
    /// descriptor>`.
    Describe {
        /// INPUT, rather than OUTPUT, written or implicit.
        input: bool,
        /// The `<SQL statement name>`.
        name: DynamicName,
        /// The `<descriptor name>` of its `<using descriptor>`.
        descriptor: ExtendedName,
    },
    /// [§17.13] ALLOCATE `<extended cursor name>` \[INSENSITIVE\] \[SCROLL\]
    /// CURSOR FOR `<extended statement name>`.
    AllocateCursor {
        /// The `<extended cursor name>`.
        cursor: ExtendedName,
        /// INSENSITIVE was written.
        insensitive: bool,
        /// SCROLL was written.
        scroll: bool,
        /// The `<extended statement name>`.
        statement: ExtendedName,
    },
    /// [§17.14] OPEN `<dynamic cursor name>` \[`<using clause>`\]: of an
    /// extended name, or with a `<using clause>`.
    Open {
        /// The `<dynamic cursor name>`.
        cursor: DynamicName,
        /// The `<using clause>`, of USING, if written.
        using: Option<UsingClause>,
    },
    /// [§17.15] FETCH \[\[`<fetch orientation>`\] FROM\] `<dynamic cursor
    /// name>` `<using clause>`, of INTO: of an extended name, or into a
    /// descriptor.
    Fetch {
        /// The `<dynamic cursor name>`.
        cursor: DynamicName,
        /// The `<fetch orientation>`: NEXT if none is written \[SR1\].
        orientation: FetchOrientation,
        /// The `<using clause>`.
        using: UsingClause,
    },
    /// [§17.16] CLOSE `<extended cursor name>`.
    Close(ExtendedName),
    /// [§17.17] DELETE FROM `<table name>` WHERE CURRENT OF `<extended
    /// cursor name>`.
    DeletePositioned {
        /// The `<table name>`.
        table: TableName,
        /// The `<extended cursor name>`.
        cursor: ExtendedName,
    },
    /// [§17.18] UPDATE `<table name>` SET `<set clause>` \[{, `<set
    /// clause>`}...\] WHERE CURRENT OF `<extended cursor name>`.
    UpdatePositioned {
        /// The `<table name>`.
        table: TableName,
        /// The `<set clause>`s, in order.
        set: Vec<SetClause>,
        /// The `<extended cursor name>`.
        cursor: ExtendedName,
    },
}
