//! [ISO/IEC 9075-11:2006 draft §6.43 to §6.46] The Definition Schema's
//! tables of what this implementation supports: SQL_CONFORMANCE,
//! SQL_IMPLEMENTATION_INFO, SQL_SIZING and SQL_SIZING_PROFILES, which the
//! views SQL_FEATURES, SQL_PACKAGES, SQL_PARTS, SQL_IMPLEMENTATION_INFO,
//! SQL_SIZING and SQL_SIZING_PROFILES read (`information/conformance.rs`).
//!
//! SQL_CONFORMANCE has a row of each part, feature and subfeature of the
//! standards' texts, as `conformance.tsv` holds them: `cargo xtask
//! features` writes that file from the feature taxonomies and from
//! CONFORMANCE.md's table of claimed features, and `cargo xtask verify`
//! fails if the two differ, so IS_SUPPORTED is YES exactly where
//! CONFORMANCE.md claims it (RULES.md D37). IS_VERIFIED_BY is null, as no
//! claim has been verified by a conformance test [§6.43 Description 6].
//!
//! SQL_SIZING has a row of each sizing item (`definition/sizing.rs`).
//! SQL_IMPLEMENTATION_INFO has none: ISO/IEC 9075-11 and ISO/IEC 9075-2
//! name no implementation information item, and those ISO/IEC 9075-3 adds
//! [§6.44] are not defined here. SQL_SIZING_PROFILES has none either, as
//! no profile is defined [§6.46 Description 1].

use std::sync::Arc;

use super::{Columns, defined, none, sizing, text};
use crate::catalog::ColumnDesc;
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable};
use crate::value::Value;

/// SQL_CONFORMANCE's rows, a line of each, its columns separated by
/// `SEPARATOR`, an empty one the null value, as `cargo xtask features`
/// writes them.
const CONFORMANCE: &str = include_str!("conformance.tsv");
/// What separates a line's columns.
const SEPARATOR: char = '\t';

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("SQL_CONFORMANCE", conformance_columns, Rows::Infallible(conformance)),
        defined("SQL_IMPLEMENTATION_INFO", implementation_columns, Rows::Infallible(none)),
        defined("SQL_SIZING", sizing_columns, Rows::Infallible(sizing::items)),
        defined("SQL_SIZING_PROFILES", profiles_columns, Rows::Infallible(none)),
    ]
}

/// [§6.43] Each column is NOT NULL that the draft's constraints make so,
/// TYPE, ID and SUB_ID by its primary key.
fn conformance_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .data("TYPE", true)
        .data("ID", true)
        .data("SUB_ID", true)
        .data("NAME", true)
        .data("SUB_NAME", false)
        .data("IS_SUPPORTED", true)
        .data("IS_VERIFIED_BY", false)
        .data("COMMENTS", false)
        .done()
}

/// [§6.43] A row of each part, feature and subfeature: of a feature,
/// SUB_ID and SUB_NAME a single space [Description 4].
fn conformance(_: &Seen<'_>) -> Vec<Row> {
    let value = |field: &str| if field.is_empty() { Value::Null } else { text(field) };
    CONFORMANCE.lines().map(|line| line.split(SEPARATOR).map(value).collect()).collect()
}

/// [§6.44]
fn implementation_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .data("IMPLEMENTATION_INFO_ID", true)
        .data("IMPLEMENTATION_INFO_NAME", true)
        .number("INTEGER_VALUE", false)
        .data("CHARACTER_VALUE", false)
        .data("COMMENTS", false)
        .done()
}

/// [§6.45] SUPPORTED_VALUE as wide as BIGINT, as a large object's length
/// is (RULES.md D79).
fn sizing_columns() -> Vec<ColumnDesc> {
    Columns::of().number("SIZING_ID", true).data("SIZING_NAME", true).length("SUPPORTED_VALUE", false).data("COMMENTS", false).done()
}

/// [§6.46]
fn profiles_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .number("SIZING_ID", true)
        .data("PROFILE_ID", true)
        .data("PROFILE_NAME", true)
        .number("REQUIRED_VALUE", false)
        .data("COMMENTS", false)
        .done()
}
