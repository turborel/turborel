//! LISTAGG, the set function that joins its group's strings in the order
//! its `<within group specification>` gives [ISO/IEC 9075-2:2023 §10.9].
//!
//! Its argument is the row of the value and its sort keys, null where the
//! value is, so the null values are removed, and warned of, as any set
//! function's are [GR11b]. Of a group's values it holds only those its
//! result can show: past the type's length, a value in order after them
//! never can, so without DISTINCT those are let go as they are passed.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use super::{Accumulator, AggregateFunction};
use crate::error::{Code, Result, SqlError, SqlState, fail, fail_with};
use crate::value::collation::CollationRef;
use crate::value::types::composite::array_ordered;
use crate::value::{DataType, MAX_CHARACTER_LENGTH, NormKey, SortOrder, Value};

#[cfg(test)]
mod tests;

/// [ISO/IEC 9075-2:2023 §10.9] LISTAGG's name, no key word of SQL-92.
pub const LISTAGG: &str = "LISTAGG";

/// [ISO/IEC 9075-2:2023 §10.9 SR11d] MLRT, the most characters of its
/// result, a variable-length character string (IA044): the most a
/// character string holds.
pub const MAX_LISTAGG_LENGTH: u32 = MAX_CHARACTER_LENGTH;

/// \[SR11g, GR11g iii\] The value of three consecutive periods, the filler
/// of a result no value fits.
const PERIODS: &str = "...";

/// \[GR11g iii 2) A)\] The characters of the parentheses WITH COUNT
/// writes around the count.
const PARENTHESES: i128 = 2;

/// [ISO/IEC 9075-2:2023 §10.9] ON OVERFLOW TRUNCATE: its `<listagg
/// truncation filler>`, and whether WITH COUNT was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Truncation {
    /// LTF.
    pub filler: String,
    /// WITH COUNT rather than WITHOUT COUNT.
    pub count: bool,
}

/// What a call of LISTAGG was written with.
pub struct ListaggSpec {
    /// LAS, the `<listagg separator>`.
    pub separator: String,
    /// ON OVERFLOW TRUNCATE's choices, or none of ON OVERFLOW ERROR.
    pub truncation: Option<Truncation>,
    /// Whether DISTINCT was written.
    pub distinct: bool,
    /// The collation its values are compared by under DISTINCT \[GR11c\], if
    /// not the default.
    pub collation: Option<CollationRef>,
    /// Per sort key, its direction and null ordering.
    pub orders: Arc<[SortOrder]>,
    /// Per sort key, the collation it orders by, if not the default.
    pub collations: Vec<Option<CollationRef>>,
}

/// [ISO/IEC 9075-2:2023 §10.9 SR11, GR11] LISTAGG of its call's spec.
pub struct Listagg(pub Arc<ListaggSpec>);

impl AggregateFunction for Listagg {
    fn name(&self) -> &str {
        LISTAGG
    }
    /// \[SR11d\] CHARACTER VARYING of MLRT characters, of a character string
    /// value and sort keys each an operand of an ordering operation
    /// [§9.14 SR3].
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        let Some(DataType::Row(r)) = arg else { return Err(not_bound()) };
        let value = r.fields.first().map(|f| &f.data_type).ok_or_else(not_bound)?;
        if !value.is_character() {
            return fail(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §10.9 F] LISTAGG joins character strings, not values of {value}"));
        }
        if let Some(key) = r.fields.iter().skip(1).find(|f| array_ordered(&f.data_type)) {
            return fail(
                Code::SyntaxRule,
                format!(
                    "[ISO/IEC 9075-2:2023 §9.14 SR3] LISTAGG orders by values of {}, and arrays are compared for equality alone",
                    key.data_type
                ),
            );
        }
        Ok(DataType::varying(MAX_LISTAGG_LENGTH))
    }
    fn accumulator(&self, _: DataType) -> Box<dyn Accumulator> {
        Box::new(Joined {
            spec: Arc::clone(&self.0),
            kept: BTreeMap::new(),
            chars: 0,
            count: 0,
            read: 0,
            trimmed: false,
            seen: HashMap::new(),
        })
    }
    /// What it was written with besides its argument, so two calls that
    /// differ only there are computed apart.
    fn label_suffix(&self) -> String {
        let s = &self.0;
        let distinct = if s.distinct { " DISTINCT" } else { "" };
        let collated = s.collation.as_ref().map_or_else(String::new, |c| format!(" COLLATE {}", c.collation.name()));
        let overflow = s.truncation.as_ref().map_or_else(String::new, |t| {
            format!(" ON OVERFLOW TRUNCATE '{}' {} COUNT", t.filler.replace('\'', "''"), if t.count { "WITH" } else { "WITHOUT" })
        });
        let keys = s.orders.iter().zip(&s.collations).enumerate().map(|(i, (order, collation))| {
            let by = collation.as_ref().map_or_else(String::new, |c| format!(" COLLATE {}", c.collation.name()));
            let down = if order.descending { " DESC" } else { "" };
            format!("{}{by}{down}{}", i.saturating_add(2), order.null_suffix())
        });
        let keys: Vec<String> = keys.collect();
        let separator = s.separator.replace('\'', "''");
        format!("{distinct}{collated}, '{separator}'{overflow} WITHIN GROUP ORDER BY {}", keys.join(", "))
    }
}

fn not_bound() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] LISTAGG was bound without its row of a value and sort keys")
}

/// Where a value is in the order of its group: by its sort keys, each as
/// its collation orders it, in its direction, its nulls placed by its null
/// ordering [GR11a, §10.10], and of keys alike by the order it was read.
#[derive(Clone)]
struct Place {
    keys: Vec<Value>,
    read: u64,
    orders: Arc<[SortOrder]>,
}

impl Ord for Place {
    fn cmp(&self, other: &Self) -> Ordering {
        let keys = self.keys.iter().zip(&other.keys).zip(self.orders.iter());
        let by_keys = keys.map(|((x, y), order)| order.cmp(x, y)).find(|o| o.is_ne());
        by_keys.unwrap_or(Ordering::Equal).then(self.read.cmp(&other.read))
    }
}

impl PartialOrd for Place {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Place {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Place {}

/// The values of a group its result can show, in order, and how many it
/// has. `chars` counts the characters of those kept; `trimmed` is whether
/// any was let go; `seen` is, under DISTINCT, the place of each value
/// kept, by its value as DISTINCT compares it.
struct Joined {
    spec: Arc<ListaggSpec>,
    kept: BTreeMap<Place, String>,
    chars: usize,
    count: usize,
    read: u64,
    trimmed: bool,
    seen: HashMap<NormKey, Place>,
}

impl Joined {
    /// The length in characters of the values kept joined by the separator.
    fn length(&self) -> usize {
        let separators = self.kept.len().saturating_sub(1).saturating_mul(characters(&self.spec.separator));
        self.chars.saturating_add(separators)
    }

    /// [GR11c i] Of a value that equals one before it, the copy first in
    /// order is kept: so `text`, at `place`, replaces it if before it.
    fn distinct(&mut self, id: NormKey, place: Place, text: String) {
        if let Some(old) = self.seen.get(&id) {
            if place >= *old {
                return;
            }
            if let Some(gone) = self.kept.remove(old) {
                self.chars = self.chars.saturating_sub(characters(&gone));
            }
        } else {
            self.count = self.count.saturating_add(1);
        }
        self.chars = self.chars.saturating_add(characters(&text));
        self.seen.insert(id, place.clone());
        self.kept.insert(place, text);
    }

    /// Lets go of the last values while those kept are longer than MLRT:
    /// no value from such a one on is in LIR's prefixes GR11g iii takes.
    /// Of ON OVERFLOW ERROR, LIR is then longer than MLRT, whatever the
    /// order of the values after: SQLSTATE 22001 [GR11g ii].
    fn trim(&mut self) -> Result<()> {
        let most = usize::try_from(MAX_LISTAGG_LENGTH).unwrap_or(usize::MAX);
        if self.length() > most && self.spec.truncation.is_none() {
            return too_long();
        }
        while self.length() > most {
            let Some((_, gone)) = self.kept.pop_last() else { break };
            self.chars = self.chars.saturating_sub(characters(&gone));
            self.trimmed = true;
        }
        Ok(())
    }
}

impl Accumulator for Joined {
    /// Takes in the row of a value not null and its sort keys, each key as
    /// its collation orders it.
    fn update(&mut self, v: &Value) -> Result<()> {
        let Value::Row(row) = v else { return Err(not_bound()) };
        let Some(Value::Char(text)) = row.first() else { return Err(not_bound()) };
        let keyed = row
            .iter()
            .skip(1)
            .enumerate()
            .map(|(k, part)| self.spec.collations.get(k).and_then(Option::as_ref).map_or_else(|| part.clone(), |c| c.sort_key(part)));
        let place = Place { keys: keyed.collect(), read: self.read, orders: Arc::clone(&self.spec.orders) };
        self.read = self.read.saturating_add(1);
        if self.spec.distinct {
            let id = self.spec.collation.as_ref().map_or_else(|| Value::Char(text.clone()), |c| c.sort_key(&Value::Char(text.clone())));
            self.distinct(id.norm(), place, text.clone());
            return Ok(());
        }
        self.count = self.count.saturating_add(1);
        self.chars = self.chars.saturating_add(characters(text));
        self.kept.insert(place, text.clone());
        self.trim()
    }
    /// [GR11e, GR11f, GR11g] Null of no values; else LIR, the values joined
    /// by the separator in order, if it is no longer than MLRT; else
    /// SQLSTATE 22001 of ON OVERFLOW ERROR, or LR of TRUNCATE.
    fn finish(&self) -> Result<Value> {
        if self.count == 0 {
            return Ok(Value::Null);
        }
        let values: Vec<&str> = self.kept.values().map(String::as_str).collect();
        let most = usize::try_from(MAX_LISTAGG_LENGTH).unwrap_or(usize::MAX);
        if !self.trimmed && self.length() <= most {
            return Ok(Value::Char(values.join(&self.spec.separator)));
        }
        match &self.spec.truncation {
            None => too_long(),
            Some(t) => Ok(Value::Char(truncated(&values, self.count, &self.spec.separator, t))),
        }
    }
}

/// [ISO/IEC 9075-2:2023 §10.9 GR11g ii] data exception, string data,
/// right truncation.
fn too_long<T>() -> Result<T> {
    fail_with(
        Code::Data,
        SqlState::STRING_DATA_RIGHT_TRUNCATION,
        format!("[ISO/IEC 9075-2:2023 §10.9 GR11g ii] LISTAGG's values joined are longer than its type's {MAX_LISTAGG_LENGTH} characters"),
    )
}

/// The length of `s` in characters.
fn characters(s: &str) -> usize {
    s.chars().count()
}

/// [ISO/IEC 9075-2:2023 §10.9 GR11g iii] LR of the `n` values whose first
/// ones, in order, are `values`, each one after them past MLRT, joined by
/// `separator`, truncated as `t` says.
fn truncated(values: &[&str], n: usize, separator: &str, t: &Truncation) -> String {
    let most = i128::from(MAX_LISTAGG_LENGTH);
    // [1)] LTFP1 is LAS || LTF.
    let ltfp1 = format!("{separator}{}", t.filler);
    let ltfp1l = count(&ltfp1);
    // [2) A), B)] the greatest i, 1 <= i < N, whose LIRPi is short enough.
    let (mut length, mut fit) = (0_i128, None);
    for (i, value) in (1..n).zip(values) {
        let joined = if i > 1 { count(separator) } else { 0 };
        length = length.saturating_add(joined).saturating_add(count(value));
        let room = most.saturating_sub(ltfp1l);
        let room = if t.count { room.saturating_sub(count(&n.saturating_sub(i).to_string())).saturating_sub(PARENTHESES) } else { room };
        if length <= room {
            fit = Some(i);
        }
    }
    let prefix = |i: usize| values.iter().take(i).copied().collect::<Vec<_>>().join(separator);
    match (fit, t.count) {
        // [A) II)] LIRPi || LTFP1 || '(' || N - i || ')'.
        (Some(i), true) => format!("{}{ltfp1}({})", prefix(i), n.saturating_sub(i)),
        // [B) II)] LIRPi || LTFP1.
        (Some(i), false) => format!("{}{ltfp1}", prefix(i)),
        // [A) I)] LIR2, LTFP1 || '(' || N || ')', or TFS || '(' || N || ')'
        // if that is longer than MLRT.
        (None, true) => {
            let lir2 = format!("{ltfp1}({n})");
            if count(&lir2) > most { format!("{PERIODS}({n})") } else { lir2 }
        }
        // [B) I)] three periods if LTFP1 is longer than MLRT, else LTFP1.
        (None, false) => {
            if ltfp1l > most {
                PERIODS.to_owned()
            } else {
                ltfp1
            }
        }
    }
}

/// The length of `s` in characters, as a number of the arithmetic of
/// GR11g iii.
fn count(s: &str) -> i128 {
    i128::try_from(characters(s)).unwrap_or(i128::MAX)
}
