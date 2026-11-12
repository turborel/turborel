//! Identity columns and generated columns [ISO/IEC 9075-2:2023 §11.4,
//! §11.20 to §11.22, §14.11]: how a `<column definition>` gives a column's
//! values, how ALTER COLUMN changes what it gives, and an INSERT's
//! `<override clause>`.

use super::{DefaultOption, SequenceOptions, ValueExpression};

/// [ISO/IEC 9075-2:2023 §11.4] An `<identity column specification>` or a
/// `<generation clause>`, in place of a `<default clause>`.
#[derive(Debug)]
pub enum ColumnGeneration {
    /// `<identity column specification>` ::= GENERATED { ALWAYS | BY
    /// DEFAULT } AS IDENTITY \[ ( `<common sequence generator options>` )
    /// \]
    Identity {
        /// ALWAYS, rather than BY DEFAULT.
        always: bool,
        /// The `<common sequence generator options>`: no RESTART.
        options: SequenceOptions,
        /// Where GENERATED begins in the statement.
        at: usize,
    },
    /// `<generation clause>` ::= GENERATED ALWAYS AS ( `<generation
    /// expression>` )
    Expression {
        /// The `<value expression>`.
        value: ValueExpression,
        /// It as written, as the column descriptor keeps it.
        text: String,
        /// Where GENERATED begins in the statement.
        at: usize,
    },
}

impl ColumnGeneration {
    /// Where it begins in the statement.
    pub fn at(&self) -> usize {
        match self {
            ColumnGeneration::Identity { at, .. } | ColumnGeneration::Expression { at, .. } => *at,
        }
    }
}

/// [ISO/IEC 9075-2:2023 §11.12] The `<alter column action>`s implemented.
#[derive(Debug)]
pub enum AlterColumnAction {
    /// [§11.13] `<set column default clause>` ::= SET `<default clause>`
    SetDefault(DefaultOption),
    /// [§11.14] `<drop column default clause>` ::= DROP DEFAULT
    DropDefault,
    /// [§11.20] `<alter identity column specification>`: SET GENERATED {
    /// ALWAYS | BY DEFAULT }, if written, and the options.
    Identity {
        /// `<set identity column generation clause>`: whether ALWAYS, if
        /// written.
        always: Option<bool>,
        /// The `<alter identity column option>`s: RESTART, and the
        /// `<basic sequence generator option>`s each SET writes.
        options: Box<SequenceOptions>,
    },
    /// [§11.21] `<drop identity property clause>` ::= DROP IDENTITY
    DropIdentity,
    /// [§11.22] `<drop column generation expression clause>` ::= DROP
    /// EXPRESSION
    DropExpression,
}

/// [ISO/IEC 9075-2:2023 §14.11] `<override clause>` ::= OVERRIDING USER
/// VALUE | OVERRIDING SYSTEM VALUE
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overriding {
    /// OVERRIDING USER VALUE: an identity column's values given are
    /// ignored, and its generator's taken \[GR7e\].
    User,
    /// OVERRIDING SYSTEM VALUE: the values given an identity column whose
    /// values are always generated are taken \[GR7e iii\].
    System,
}
