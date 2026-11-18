//! [ISO/IEC 9075:1992 §21.3.23 to §21.3.25] The Definition Schema's
//! tables of character sets, collations and translations: CHARACTER_SETS,
//! of the one character set, SQL_TEXT [§10.4 SR3]; COLLATIONS, of the
//! built-in collations and those CREATE COLLATION makes; TRANSLATIONS, of
//! none, as there are no translations; and, of ISO/IEC 9075-11:2006 draft
//! §6.17, COLLATION_CHARACTER_SET_APPLICABILITY, of each collation and the
//! character set it is applicable to, SQL_TEXT, the one there is.

use std::sync::Arc;

use super::{Columns, SQL_TEXT, catalog_name, defined, ids, information, none, number, text};
use crate::catalog::{ColumnDesc, QualifiedName};
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable};
use crate::value::collation::{Pad, default_pad};

/// [ISO/IEC 9075:1992 §21.3.23 Description 5b, 5c] Its form-of-use and
/// number of characters, implementation-defined: a character string is
/// Unicode, every scalar value of which is a character.
const FORM_OF_USE: &str = "UTF-8";
const NUMBER_OF_CHARACTERS: usize = 1_112_064;

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("CHARACTER_SETS", character_sets_columns, Rows::Infallible(character_sets)),
        defined("COLLATIONS", collations_columns, Rows::Infallible(collations)),
        defined("COLLATION_CHARACTER_SET_APPLICABILITY", applicability_columns, Rows::Infallible(applicability)),
        defined("TRANSLATIONS", translations_columns, Rows::Infallible(none)),
    ]
}

fn character_sets_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("CHARACTER_SET", true)
        .id("CHARACTER_REPERTOIRE", true)
        .id("FORM_OF_USE", false)
        .number("NUMBER_OF_CHARACTERS", false)
        .ids("DEFAULT_COLLATE", true)
        .done()
}

/// [§21.3.23 Description 5] SQL_TEXT, its default collation SQL_TEXT, of
/// the character repertoire SQL_TEXT [ISO/IEC 9075-2:2023 §4.3.7, Table 1;
/// ISO/IEC 9075-11:2006 draft §6.11 Description 2].
fn character_sets(seen: &Seen<'_>) -> Vec<Row> {
    let (catalog, sql_text) = (catalog_name(seen), information(SQL_TEXT));
    let form = vec![text(SQL_TEXT), text(FORM_OF_USE), number(NUMBER_OF_CHARACTERS)];
    vec![[ids(&catalog, &sql_text).to_vec(), form, ids(&catalog, &sql_text).to_vec()].concat()]
}

fn collations_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("COLLATION", true).ids("CHARACTER_SET", true).data("PAD_ATTRIBUTE", false).done()
}

/// Each collation: the built-in ones, of INFORMATION_SCHEMA, then those
/// definitions made, each its name and its pad attribute.
fn every(seen: &Seen<'_>) -> Vec<(QualifiedName, Pad)> {
    let built = seen.state.collations.keys().map(|name| (information(name), default_pad(name)));
    built.chain(seen.state.catalog.collations().into_iter().map(|c| (c.name.clone(), c.pad))).collect()
}

/// [§21.3.24 Description 4] The built-in collations and those definitions
/// made, each of SQL_TEXT, and its pad attribute.
fn collations(seen: &Seen<'_>) -> Vec<Row> {
    let (catalog, sql_text) = (catalog_name(seen), information(SQL_TEXT));
    let row = |(name, pad): (QualifiedName, Pad)| {
        let pad = if pad == Pad::Space { "PAD SPACE" } else { "NO PAD" };
        [ids(&catalog, &name).to_vec(), ids(&catalog, &sql_text).to_vec(), vec![text(pad)]].concat()
    };
    every(seen).into_iter().map(row).collect()
}

fn applicability_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("COLLATION", true).ids("CHARACTER_SET", true).done()
}

/// [ISO/IEC 9075-11:2006 draft §6.17 Description 1, 2] Each collation,
/// applicable to SQL_TEXT, the character set of every character string
/// [ISO/IEC 9075:1992 §10.4 SR3].
fn applicability(seen: &Seen<'_>) -> Vec<Row> {
    let (catalog, sql_text) = (catalog_name(seen), information(SQL_TEXT));
    every(seen).into_iter().map(|(name, _)| [ids(&catalog, &name), ids(&catalog, &sql_text)].concat()).collect()
}

fn translations_columns() -> Vec<ColumnDesc> {
    let columns = Columns::of().ids("TRANSLATION", true).ids("SOURCE_CHARACTER_SET", true).ids("TARGET_CHARACTER_SET", true);
    columns.ids("TRANSLATION_SOURCE", false).done()
}

/// [ISO/IEC 9075:1992 §10.4 SR2, §11.30 SR4] The usage privileges on the
/// character set and collation: the character set's PUBLIC's WITH GRANT
/// OPTION, the collation's PUBLIC's, both of the grantor "_SYSTEM".
pub(in crate::system) fn usage() -> [(&'static str, bool); 2] {
    [("CHARACTER SET", true), ("COLLATION", false)]
}

/// The name of the character set and collation.
pub(in crate::system) fn sql_text() -> QualifiedName {
    information(SQL_TEXT)
}
