//! \[DESIGN\] (RULES.md D1185) A sort by a key made of a column under a
//! collation whose sort key is the text itself (`CollationRef::is_octets`,
//! D1183) reads the column in place. ORDER BY under a collation sorts by
//! a column its input's projection makes, the sort key of each row
//! [ISO/IEC 9075-2:2023 §10.10], which under such a collation is the
//! string's own octets, made a binary string a row. Where none above the
//! sort reads that column, the sort is keyed by the column the key was made
//! of, marked to order its strings by their octets (`SortKey::octets`),
//! and the projection moves the column in the key's place, so it makes
//! no value. If the projection gives the column already, the key reads
//! that one, and the key's own place is left unread, a vector of nulls
//! (D1167).
//!
//! A column read above the sort is left as it is, as its rows would hold
//! strings where binary strings are read. FETCH FIRST WITH TIES reads its
//! sort's keys to find the peers of the last row, so its sort keeps the
//! key made.

use super::super::super::Plan;
use crate::logical::{Expr, SortKey};

/// The text EXPLAIN writes between a key's expression and its collation.
const COLLATE: &str = " COLLATE ";

/// Of a sort by `keys` of `input`, those above it reading its columns
/// `needed`: each key that reads a sort key the projection `input` makes
/// of a column under a collation by octets, read above by none, read of
/// the column in place, by octets.
pub(super) fn in_place(keys: &mut [SortKey], input: &mut Plan, needed: &[usize]) {
    let Plan::Project { exprs, detail, .. } = input else { return };
    let mut pieces: Vec<String> = detail.split(", ").map(str::to_owned).collect();
    let named = pieces.len() == exprs.len();
    for n in 0..keys.len() {
        let Some(made) = keys.get(n).map(|k| k.column).filter(|k| !needed.contains(k)) else { continue };
        let Some((of, collation)) = made_of(exprs.get(made)) else { continue };
        if let Some(e) = exprs.get_mut(made) {
            *e = Expr::Column(of);
        }
        if let Some(piece) = pieces.get_mut(made) {
            let end = piece.rfind(COLLATE).filter(|at| piece.get(at.saturating_add(COLLATE.len())..) == Some(collation.as_str()));
            piece.truncate(end.unwrap_or(piece.len()));
        }
        // The first column that moves it: the key's own place at the latest.
        let read = exprs.iter().position(|e| if let Expr::Column(c) = e { *c == of } else { false }).unwrap_or(made);
        for key in keys.iter_mut().filter(|k| k.column == made) {
            *key = SortKey { column: read, octets: true, ..*key };
        }
    }
    if named {
        *detail = pieces.join(", ");
    }
}

/// The column `e` makes a sort key of and the name of its collation, if
/// `e` is the sort key of a column under a collation by octets.
fn made_of(e: Option<&Expr>) -> Option<(usize, String)> {
    let Some(Expr::SortKey(of, collation)) = e else { return None };
    let Expr::Column(c) = **of else { return None };
    collation.is_octets().then(|| (c, collation.collation.name().to_owned()))
}

// Unit tests: which keys are read in place, and which are left as made
// (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::collation::{CollationRef, Pad, unpadded_scalar};

    /// A projection of `exprs`, shown as `detail`, of no rows.
    fn project(exprs: Vec<Expr>, detail: &str) -> Plan {
        let input = Box::new(Plan::Values { rows: Vec::new(), detail: String::new() });
        Plan::Project { exprs, detail: detail.to_owned(), input }
    }

    /// The sort key of column `c` under `collation`.
    fn keyed(c: usize, collation: &CollationRef) -> Expr {
        Expr::SortKey(Box::new(Expr::Column(c)), collation.clone())
    }

    /// The keys and the projection's expressions and text after `in_place`
    /// of a sort by `keys` of a projection of `exprs`, those above reading
    /// `needed`.
    fn placed(keys: &[SortKey], exprs: Vec<Expr>, needed: &[usize]) -> (Vec<SortKey>, String, String) {
        let (mut keys, mut input) = (keys.to_vec(), project(exprs, "T.A, T.S, T.S COLLATE UCS_BASIC"));
        in_place(&mut keys, &mut input, needed);
        let Plan::Project { exprs: given, detail, .. } = input else { panic!("a projection") };
        (keys, format!("{given:?}"), detail)
    }

    /// Verifies: [DESIGN] RULES.md D1185: a sort by the key a projection
    /// makes of a column under a collation by octets, read above by none,
    /// reads the first column that moves it, by octets, direction and null
    /// ordering kept, and the projection moves the column in the key's
    /// place; with no column that moves it, the key's own place does.
    #[test]
    fn a_key_of_a_columns_octets_reads_the_column() {
        let octets = unpadded_scalar();
        let made = SortKey { column: 2, descending: true, nulls_first: true, octets: false };
        let exprs = vec![Expr::Column(0), Expr::Column(1), keyed(1, &octets)];
        let (keys, shown, detail) = placed(&[made, SortKey::ascending(0)], exprs, &[0, 1]);
        assert_eq!(keys, [SortKey { column: 1, octets: true, ..made }, SortKey::ascending(0)]);
        assert_eq!(shown, format!("{:?}", [Expr::Column(0), Expr::Column(1), Expr::Column(1)]));
        assert_eq!(detail, "T.A, T.S, T.S");
        let alone = vec![Expr::Column(0), Expr::Column(0), keyed(1, &octets)];
        let (own, moved, _) = placed(&[made], alone, &[0]);
        assert_eq!(own, [SortKey { column: 2, octets: true, ..made }]);
        assert_eq!(moved, format!("{:?}", [Expr::Column(0), Expr::Column(0), Expr::Column(1)]));
    }

    /// Verifies: [DESIGN] RULES.md D1185: a key is left as made, the
    /// projection untouched, if its column is read above the sort; if its
    /// collation pads, or orders otherwise than by octets; if it is made
    /// of an expression that is no column; if it is a column moved; and of
    /// an input that is no projection.
    ///
    /// MC/DC of the conditions that keep a key made: each case differs
    /// from the one read in place above in one of them alone.
    #[test]
    fn a_key_read_above_or_not_of_octets_stays_made() {
        let octets = unpadded_scalar();
        let made = [SortKey::ascending(2)];
        let exprs = |key: Expr| vec![Expr::Column(0), Expr::Column(1), key];
        let same = |key: Expr, needed: &[usize]| {
            let (keys, shown, detail) = placed(&made, exprs(key.clone()), needed);
            assert_eq!(
                (keys.as_slice(), shown, detail.as_str()),
                (made.as_slice(), format!("{:?}", exprs(key)), "T.A, T.S, T.S COLLATE UCS_BASIC")
            );
        };
        same(keyed(1, &octets), &[0, 2]);
        same(keyed(1, &CollationRef { pad: Pad::Space, ..octets.clone() }), &[0]);
        same(Expr::SortKey(Box::new(Expr::Neg(Box::new(Expr::Column(1)))), octets.clone()), &[0]);
        same(Expr::Column(1), &[0]);
        let (mut keys, mut values) = (made.to_vec(), Plan::Values { rows: Vec::new(), detail: String::new() });
        in_place(&mut keys, &mut values, &[0]);
        assert_eq!(keys, made);
    }
}
