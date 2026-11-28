//! \[DESIGN\] The bodies over vectors of built-in scalar functions
//! (RULES.md D363, `ScalarFunction::eval_batch`): UPPER and LOWER of
//! character strings fold a vector's text whole, as each string's letters
//! fold one by one [ISO/IEC 9075:1992 §6.7 GR2c], or a dictionary's
//! entries once; a length counts each string, or each entry once [§6.6
//! GR4, GR5]; ABS of integers or approximate numbers negates those below
//! 0 [ISO/IEC 9075-2:2023 §6.31 GR8]; EXTRACT of a datetime's field and
//! CAST of a datetime to a datetime type compute a vector's microseconds
//! (`datetimes.rs`, RULES.md D1320). Each gives what the function's call
//! gives of each row, a null of a null.

pub(crate) mod datetimes;

use std::sync::Arc;

use crate::storage::mask::index;
use crate::storage::{Coded, Floats, Ints, Mask, Texts};

/// [ISO/IEC 9075:1992 §6.7 GR2c] `texts` folded to upper case, or lower:
/// only the simple Latin letters change, a byte each, so every string
/// keeps its place.
pub(super) fn folded(texts: &Texts, upper: bool) -> Texts {
    let fold = |s: &str| if upper { s.to_ascii_uppercase() } else { s.to_ascii_lowercase() };
    if let Some(coded) = &texts.coded {
        let entries = &coded.entries;
        let made = Texts::new(fold(&entries.text), entries.ends.clone(), entries.nulls.clone());
        return Texts::coded(Coded { codes: coded.codes.clone(), entries: Arc::new(made) }, texts.nulls.clone());
    }
    Texts::new(fold(&texts.text), texts.ends.clone(), texts.nulls.clone())
}

/// The count `count` gives of each of `texts`, an exact number of scale
/// 0, a null of a null: of a dictionary's strings, each entry counted
/// once.
pub(super) fn counted(texts: &Texts, count: impl Fn(&str) -> usize) -> Ints {
    let rows = texts.len();
    let units: Vec<i64> = if let Some(coded) = &texts.coded {
        let entries: Vec<i64> = (0..coded.entries.len()).map(|e| whole(coded.entries.get(e).map_or(0, &count))).collect();
        coded.codes.iter().map(|&c| entries.get(index(c)).copied().unwrap_or(0)).collect()
    } else {
        (0..rows).map(|n| whole(texts.get(n).map_or(0, &count))).collect()
    };
    let mut units = units;
    if let Some(nulls) = &texts.nulls {
        for (n, unit) in units.iter_mut().enumerate() {
            if nulls.get(n) {
                *unit = 0;
            }
        }
    }
    Ints::new(units, 0, texts.nulls.clone())
}

/// `n` as a count's units.
fn whole(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// [ISO/IEC 9075-2:2023 §6.31 GR8] The absolute value of each of `ints`,
/// of their scale: none if one's negation does not fit, which the call
/// then makes past 64 bits.
pub(super) fn absolute_ints(ints: &Ints) -> Option<Ints> {
    let units = ints.units.iter().map(|&u| if u < 0 { u.checked_neg() } else { Some(u) }).collect::<Option<Vec<i64>>>()?;
    Some(Ints::new(units, ints.scale, ints.nulls.clone()))
}

/// [ISO/IEC 9075-2:2023 §6.31 GR8] The absolute value of each of
/// `floats`: a number below 0 negated, any other as it is, -0 too; none
/// if one not null is NaN, which no number compares with, so the call
/// raises an error of it.
pub(super) fn absolute_floats(floats: &Floats) -> Option<Floats> {
    let nulls = floats.nulls.as_ref();
    let nan = floats.numbers.iter().enumerate().any(|(n, x)| x.is_nan() && !nulls.is_some_and(|m: &Mask| m.get(n)));
    if nan {
        return None;
    }
    let numbers = floats.numbers.iter().map(|&x| if x < 0.0_f64 { -x } else { x }).collect();
    Some(Floats::new(numbers, floats.nulls.clone()))
}
