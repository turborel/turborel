//! Abstract syntax of sequence generators [ISO/IEC 9075-2:2023 §11.72,
//! §11.73]: their definition and alteration, and the options of each.

use super::TableName;
use crate::value::{DataType, Decimal};

/// [ISO/IEC 9075-2:2023 §11.72] `<sequence generator definition>` ::=
/// CREATE SEQUENCE `<sequence generator name>` \[`<sequence generator
/// options>`\]
#[derive(Debug)]
pub struct SequenceDefinition {
    /// The `<sequence generator name>`.
    pub name: TableName,
    /// The `<data type>` of its `<sequence generator data type option>`,
    /// if written, and where it begins.
    pub data_type: Option<(DataType, usize)>,
    /// Its `<common sequence generator options>`.
    pub options: SequenceOptions,
}

/// [ISO/IEC 9075-2:2023 §11.73] `<alter sequence generator statement>` ::=
/// ALTER SEQUENCE `<sequence generator name>` `<alter sequence generator
/// options>`
#[derive(Debug)]
pub struct AlterSequence {
    /// The `<sequence generator name>`.
    pub name: TableName,
    /// Its `<alter sequence generator options>`: no START WITH.
    pub options: SequenceOptions,
}

/// [ISO/IEC 9075-2:2023 §11.72, §11.73] The options of a sequence
/// generator, each written at most once [§9.36 SR3, §9.37 SR4].
#[derive(Debug, Default)]
pub struct SequenceOptions {
    /// START WITH `<sequence generator start value>`, if written: never of
    /// an alteration.
    pub start: Option<SignedNumber>,
    /// INCREMENT BY `<sequence generator increment>`, if written.
    pub increment: Option<SignedNumber>,
    /// MAXVALUE `<sequence generator max value>`, or none for NO MAXVALUE,
    /// if either is written.
    pub maximum: Option<Option<SignedNumber>>,
    /// MINVALUE `<sequence generator min value>`, or none for NO MINVALUE,
    /// if either is written.
    pub minimum: Option<Option<SignedNumber>>,
    /// CYCLE, or NO CYCLE, if either is written.
    pub cycle: Option<bool>,
    /// RESTART \[WITH `<sequence generator restart value>`\], if written:
    /// only of an alteration.
    pub restart: Option<Option<SignedNumber>>,
}

/// [ISO/IEC 9075-2:2023 §5.3] A `<signed numeric literal>`: its value,
/// its sign applied, and where it begins.
#[derive(Debug, Clone, Copy)]
pub struct SignedNumber {
    /// Its value, of an `<exact numeric literal>`; of an `<approximate
    /// numeric literal>`, none if no exact number is it.
    pub value: Option<Decimal>,
    /// Where it begins in the statement.
    pub at: usize,
}
