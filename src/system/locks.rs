//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] INFORMATION_SCHEMA.LOCKS: the
//! locks held and sought now, an object "associated with
//! INFORMATION_SCHEMA" this implementation adds (RULES.md D188). A row for
//! each mode a transaction holds a table's lock in, for each lock of a row,
//! for each advisory lock held, and for each lock a transaction waits for,
//! with the session and transaction holding or seeking it. The database's
//! owner sees them; any other, none, as MEMORY_RESERVATIONS.

use super::{Seen, SystemTable, catalog_name, identifier, text, text_column, yes};
use crate::catalog::{ColumnDesc, QualifiedName};
use crate::error::Result;
use crate::storage::Row;
use crate::txn::{AdvisoryKey, LockRow, Sought};
use crate::value::{DataType, Decimal, MAX_EXACT_PRECISION, Spelled, Value};

/// \[DESIGN\] The longest advisory key shown, and the longest mode.
const KEY_LENGTH: u32 = 128;
const MODE_LENGTH: u32 = 32;

/// \[EXTENSION\] INFORMATION_SCHEMA.LOCKS.
pub(super) struct Locks;

/// A column of whole numbers as large as any id, NOT NULL if `not_null`.
fn id_column(name: &str, not_null: bool) -> ColumnDesc {
    ColumnDesc {
        name: name.to_owned(),
        data_type: DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric },
        not_null,
        default: None,
        domain: None,
        collation: None,
        generated: None,
    }
}

/// An id's value.
fn id(n: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

impl SystemTable for Locks {
    fn name(&self) -> &'static str {
        "LOCKS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        vec![
            text_column(("LOCK_TYPE", 8), true),
            identifier("TABLE_CATALOG", false),
            identifier("TABLE_SCHEMA", false),
            identifier("TABLE_NAME", false),
            id_column("ROW_ID", false),
            text_column(("ADVISORY_KEY", KEY_LENGTH), false),
            text_column(("LOCK_MODE", MODE_LENGTH), true),
            id_column("SESSION_ID", true),
            id_column("TRANSACTION_ID", false),
            text_column(("IS_WAITING", 3), true),
        ]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        if !seen.is_owner() {
            return Ok(Vec::new());
        }
        let st = seen.state;
        let mut locks: Vec<LockRow> = Vec::new();
        for (table, lk) in &st.locks {
            locks.extend(lk.held_locks(table));
        }
        locks.extend(st.tx.advisory().held_locks());
        locks.extend(st.tx.awaiting());
        let catalog = catalog_name(seen);
        let mut rows: Vec<Row> = locks.iter().map(|l| row_of(l, &catalog)).collect();
        rows.sort_by_key(|r| format!("{r:?}"));
        Ok(rows)
    }
}

/// The row of lock `l`, the catalog's name `catalog`.
fn row_of(l: &LockRow, catalog: &Value) -> Row {
    let table = |t: &QualifiedName| [catalog.clone(), text(&t.auth), text(&t.ident)];
    let none = || [Value::Null, Value::Null, Value::Null];
    let (kind, names, row, key, mode) = match &l.lock {
        Sought::Table(t, mode) => ("TABLE", table(t), Value::Null, Value::Null, mode.name()),
        Sought::Row(t, at, exclusive) => {
            ("ROW", table(t), at.map_or(Value::Null, id), Value::Null, if *exclusive { "FOR UPDATE" } else { "FOR SHARE" })
        }
        Sought::Advisory(k, exclusive) => {
            ("ADVISORY", none(), Value::Null, text(&key_text(k)), if *exclusive { "EXCLUSIVE" } else { "SHARE" })
        }
    };
    let [cat, schema, name] = names;
    let tx = l.tx.map_or(Value::Null, id);
    vec![text(kind), cat, schema, name, row, key, text(mode), id(l.session), tx, yes(l.waiting)]
}

/// An advisory lock's key, as a value: a number, a pair of numbers in
/// parentheses, or a name in quotes.
fn key_text(k: &AdvisoryKey) -> String {
    let shown = match k {
        AdvisoryKey::Int(n) => n.to_string(),
        AdvisoryKey::Pair(a, b) => format!("({a}, {b})"),
        AdvisoryKey::Name(name) => format!("'{}'", name.replace('\'', "''")),
    };
    shown.chars().take(usize::try_from(KEY_LENGTH).unwrap_or(usize::MAX)).collect()
}
