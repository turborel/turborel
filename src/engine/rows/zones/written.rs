//! \[DESIGN\] Of the zones (see the module's documentation), how a table
//! widens the zone of a row it wrote: over the row the write replaced,
//! decoded or as it was stored, or to hold a version put back.

use super::Replaced;
use crate::engine::rows::RowTable;
use crate::engine::rows::version::Version;
use crate::error::Result;
use crate::storage::{Row, RowId};

impl RowTable {
    /// Widens the zone of row `id` to hold `row`, or a deletion, written
    /// over the row's newest version, holding `replaced`, if known
    /// (`Zones::widen`): the row the write replaced, decoded once for it
    /// and the write's caller (RULES.md D835).
    pub(in crate::engine::rows) fn widen_over(&self, id: RowId, row: Option<&Row>, replaced: Option<&Row>) {
        let known = if self.frozen.plain() { Replaced::Row } else { Replaced::Outside };
        self.zones.widen(id, row, replaced.map_or(Replaced::Unknown, known));
    }

    /// As `widen_over`, given the version the write replaced as it was
    /// stored, its header and record, if it found one: only the columns
    /// the zones keep are decoded, none if they keep none, as the zone
    /// reads no other (RULES.md D835). A deletion, or none, is a version
    /// whose values are not known.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `replaced` is not a version or its record is
    /// malformed: the zone is then widened as over a version not known.
    pub(in crate::engine::rows) fn widen_past(&self, id: RowId, row: Option<&Row>, replaced: Option<&[u8]>) -> Result<()> {
        if self.zones.cold() {
            return Ok(());
        }
        let kept = &self.zones.kept;
        let version = replaced.map(Version::parse).transpose().map(|v| v.filter(|v| !v.deleted && !kept.is_empty()));
        let old = version.and_then(|v| v.map(|v| self.decode(id, v.record, Some(kept))).transpose());
        self.widen_over(id, row, old.as_ref().ok().and_then(Option::as_ref));
        old.map(drop)
    }

    /// \[DESIGN\] Widens the zone of row `id` to hold `record`, written
    /// over the version `replaced`, as `widen_past` widens it to a row
    /// (RULES.md D916): the columns the zones keep are decoded of the
    /// record, for an update that holds no value but those it set. Of a
    /// table cold, nothing is read.
    ///
    /// # Errors
    ///
    /// As `widen_past`; SQLCODE -916 if `record` is malformed.
    pub(in crate::engine::rows) fn widen_patched(&self, id: RowId, record: &[u8], replaced: &[u8]) -> Result<()> {
        if self.zones.cold() {
            return Ok(());
        }
        let row = self.decode(id, record, Some(&self.zones.kept))?;
        self.widen_past(id, Some(&row), Some(replaced))
    }

    /// Widens the zone of row `id` to hold `version`, a version of it put
    /// back in its tree, as a write widens it once written: a zone summed
    /// up while the version was not the row's newest holds it again.
    pub(in crate::engine::rows) fn widen_to(&self, id: RowId, version: &[u8]) -> Result<()> {
        if self.zones.cold() {
            return Ok(());
        }
        let version = Version::parse(version)?;
        let row = if version.deleted { None } else { Some(self.decode(id, version.record, Some(&self.zones.kept))?) };
        self.zones.widen(id, row.as_ref(), Replaced::Unknown);
        Ok(())
    }
}

// Unit tests: a zone widened over a version as stored (RULES.md R-TEST-1,
// R-TEST-2).
#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::super::tests::{TYPES, files, row, warmed};
    use crate::engine::rows::{RowTable, TableOptions, key};
    use crate::error::Code;
    use crate::index::Probe;
    use crate::storage::{Condition, Conjunction, TableStorage};
    use crate::txn::NO_TRANSACTION;
    use crate::value::{CompOp, Decimal, Value};

    /// Verifies: [DESIGN] RULES.md D835, D476: a zone widened over a
    /// version as it was stored holds the new row, and is not stale when
    /// the value replaced lay inside its bounds, as when widened over the
    /// row decoded; over no version, or bytes that are no version, which
    /// are refused (SQLCODE -916), it is stale.
    #[test]
    fn a_zone_is_widened_over_a_version_as_stored() {
        let table = warmed(RowTable::create(&files(), &TYPES, TableOptions::default()).unwrap());
        let ids: Vec<u64> = (1..=5).map(|n| table.insert(NO_TRANSACTION, row(n)).unwrap()).collect();
        let stale = || table.zones.stale.load(Ordering::Acquire) > 0;
        let reads = |n: i128| {
            let value = Value::Exact(Decimal::new(n, 0));
            let nine = Conjunction::of(Condition { columns: vec![0], probe: Probe::Compare { op: CompOp::Eq, value } });
            table.zones.ranges(Some(&nine)).iter().any(|&(first, end)| first <= ids[2] && end.is_none_or(|e| ids[2] < e))
        };
        assert!(!reads(9) && !stale());
        let stored = table.tree.get(&key(ids[2])).unwrap().unwrap();
        table.widen_past(ids[2], Some(&row(9)), Some(&stored)).unwrap();
        assert!(reads(9) && !stale(), "3 lay inside 1 to 5");
        assert_eq!(table.widen_past(ids[2], Some(&row(12)), Some(&[0xFF])).err().map(|e| e.code), Some(Code::Corrupt));
        assert!(reads(12) && stale(), "widened all the same, over a version not known");
    }
}
