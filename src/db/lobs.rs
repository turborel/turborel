//! [ISO/IEC 9075-2:2023 §4.37.5, §14.17, §14.18] A session's large object
//! locators (RULES.md D79): four-octet integers each naming a large object
//! value of the session's, as a client reaches one it has not read whole.
//! A value is assigned one as a server sends it, too long to send whole,
//! or as a client makes one a piece at a time. A locator is valid until
//! its transaction ends, as T039 and T049 say; FREE LOCATOR ends it
//! sooner, and HOLD LOCATOR (Feature T561) keeps it past the commit, its
//! value copied then to the database's temporary large objects, as what a
//! table keeps may be collected once no transaction reads it. A rollback
//! ends every locator, held or not, and so does the session's end.
//!
//! A value read from a table under READ COMMITTED is read at its
//! statement's snapshot, which the next statement moves on from, so the
//! transaction keeps that snapshot counted open once a locator names such
//! a value (`Txn::pin_snapshot`).

use std::collections::HashMap;

use super::Session;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::lob::{Lob, LobBuilder, LobKind, Part, TempLobs};
use crate::value::{DataType, INTEGER_PRECISION, Spelled};

mod writing;

use writing::Appender;

/// \[DESIGN\] The most octets a locator's read gives at once: a chunk, as
/// a client asks again for the rest.
pub const LOB_READ_OCTETS: u64 = 262_144;

/// [ISO/IEC 9075-2:2023 §4.37.5] The type of a locator as a `<locator
/// reference>` takes it: a four-octet integer, INTEGER, each locator
/// given at most its greatest value (UV065).
pub(super) const LOCATOR_TYPE: DataType = DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer };

/// The greatest locator, INTEGER's greatest four-octet value.
const MOST_LOCATOR: u32 = 2_147_483_647;

/// The locators of a session.
#[derive(Default)]
pub(super) struct Locators {
    /// The last locator given, 0 for none: the next is the one after it
    /// not in use, never 0.
    last: u32,
    entries: HashMap<u32, Entry>,
}

/// What a locator names.
enum Entry {
    /// A value, and whether HOLD LOCATOR made it holdable.
    Value { lob: Lob, holdable: bool },
    /// A value a client is making, a piece at a time.
    Writing(Appender),
}

/// [ISO/IEC 9075-2:2023 §14.17 GR1a, §14.18 GR1a] SQLSTATE 0F001, locator
/// exception, invalid specification: `locator` names no value.
fn invalid(locator: u32) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075-2:2023 §4.37.5] {locator} is not a valid locator"))
        .with_state(SqlState::LOCATOR_INVALID_SPECIFICATION)
}

impl Locators {
    /// A new locator, naming `entry`.
    fn assign(&mut self, entry: Entry) -> u32 {
        let mut next = self.last;
        loop {
            next = if next >= MOST_LOCATOR { 1 } else { next.saturating_add(1) };
            if !self.entries.contains_key(&next) {
                break;
            }
        }
        self.last = next;
        self.entries.insert(next, entry);
        next
    }

    /// The transaction ended, committed or not: every locator but a held
    /// one of a commit invalid [§4.37.5].
    pub(super) fn ended(&mut self, committed: bool) {
        if committed {
            self.entries.retain(|_, e| matches!(e, Entry::Value { holdable: true, .. }));
        } else {
            self.entries.clear();
        }
    }

    /// The value `locator` names, made whole if a client was making it.
    fn value(&mut self, locator: u32) -> Result<&Lob> {
        let entry = self.entries.get_mut(&locator).ok_or_else(|| invalid(locator))?;
        if let Entry::Writing(appender) = entry {
            let lob = appender.finish()?;
            *entry = Entry::Value { lob, holdable: false };
        }
        match entry {
            Entry::Value { lob, .. } => Ok(lob),
            Entry::Writing(_) => Err(invalid(locator)),
        }
    }
}

/// Whether `lob` reads what a table keeps, which is collected once no
/// transaction reads it, as a temporary one is not.
fn table_kept(lob: &Lob) -> bool {
    lob.parts().iter().any(|p| match p {
        Part::Slice(s) => s.source.kept().is_some(),
        Part::Reference(_) => true,
        Part::Inline(..) | Part::Locator(_) => false,
    })
}

/// `lob` as a value of its own, kept in `temp` if it reads a table.
fn held_copy(lob: &Lob, temp: &dyn TempLobs) -> Result<Lob> {
    if !table_kept(lob) {
        return Ok(lob.clone());
    }
    let mut copy = LobBuilder::new(lob.kind(), Some(temp));
    lob.reader().each(0, lob.octets(), &mut |piece| copy.push_bytes(piece).map(|()| true))?;
    copy.finish()
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §4.37.5] A new locator of `lob`, valid until
    /// the transaction ends: of a value the session read, as a server
    /// sends one too long to send whole.
    pub fn locator_of(&self, lob: Lob) -> u32 {
        if table_kept(&lob)
            && let Some(tx) = self.inner.borrow().tx.as_ref()
        {
            tx.pin_snapshot();
        }
        self.locators.borrow_mut().assign(Entry::Value { lob, holdable: false })
    }

    /// The value `locator` names.
    ///
    /// # Errors
    ///
    /// SQLSTATE 0F001 if it names none; 22021 if a value a client made of
    /// a character large object is not UTF-8.
    pub fn located(&self, locator: u32) -> Result<Lob> {
        self.locators.borrow_mut().value(locator).cloned()
    }

    /// \[DESIGN\] A new locator of an empty large object of `kind`, which
    /// `append_lob` adds to, as a client makes one a piece at a time.
    pub fn create_lob(&self, kind: LobKind) -> u32 {
        self.locators.borrow_mut().assign(Entry::Writing(Appender::new(kind)))
    }

    /// \[DESIGN\] Adds `bytes` to the large object `locator` names, made by
    /// `create_lob` and not yet used: octets, or UTF-8 of a character one,
    /// a character's octets possibly split between two calls.
    ///
    /// # Errors
    ///
    /// SQLSTATE 0F001 if it names no large object being made; 22001 if it
    /// grows past the longest; 22021 if a character one is not UTF-8.
    pub fn append_lob(&self, locator: u32, bytes: &[u8]) -> Result<()> {
        let temp = self.db.snapshot()?.temp_lobs.clone();
        match self.locators.borrow_mut().entries.get_mut(&locator) {
            Some(Entry::Writing(appender)) => appender.append(bytes, &*temp),
            Some(Entry::Value { .. }) | None => Err(invalid(locator)),
        }
    }

    /// \[DESIGN\] At most `count` of the octets of the value `locator`
    /// names from octet `from`, from 0, or of a character large object its
    /// characters, as UTF-8, if `characters`; at most `LOB_READ_OCTETS`
    /// octets, whole characters, at once.
    ///
    /// # Errors
    ///
    /// SQLSTATE 0F001 if it names none; reading the value's.
    pub fn read_lob(&self, locator: u32, (from, count): (u64, u64), characters: bool) -> Result<Vec<u8>> {
        let lob = self.located(locator)?;
        if !characters || lob.kind() == LobKind::Binary {
            return lob.reader().read(from, count.min(LOB_READ_OCTETS));
        }
        let mut reader = lob.reader();
        let start = reader.octet_of_char(from)?;
        let end = reader.octet_of_char(from.saturating_add(count))?.min(start.saturating_add(LOB_READ_OCTETS));
        let mut bytes = reader.read(start, end.saturating_sub(start))?;
        let whole = crate::value::lob::char_cut(&bytes, bytes.len());
        bytes.truncate(whole);
        Ok(bytes)
    }

    /// [ISO/IEC 9075-2:2023 §14.17 GR1] FREE LOCATOR: `locator` no longer
    /// valid.
    ///
    /// # Errors
    ///
    /// SQLSTATE 0F001 if it is not valid \[GR1a\].
    pub fn free_locator(&self, locator: u32) -> Result<()> {
        self.locators.borrow_mut().entries.remove(&locator).map(drop).ok_or_else(|| invalid(locator))
    }

    /// [ISO/IEC 9075-2:2023 §14.18 GR1] HOLD LOCATOR: `locator` holdable,
    /// valid past the commit of its transaction, its value copied now if a
    /// table keeps it.
    ///
    /// # Errors
    ///
    /// SQLSTATE 0F001 if it is not valid \[GR1a\]; making the copy's.
    pub fn hold_locator(&self, locator: u32) -> Result<()> {
        let temp = self.db.snapshot()?.temp_lobs.clone();
        let mut locators = self.locators.borrow_mut();
        let copy = held_copy(locators.value(locator)?, &*temp)?;
        locators.entries.insert(locator, Entry::Value { lob: copy, holdable: true });
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §14.17, §14.18] FREE LOCATOR, or HOLD LOCATOR
    /// if `hold`, of the locator each of `references` gives.
    ///
    /// # Errors
    ///
    /// SQLSTATE 0F001 if one is not valid \[GR1a\]; the null value too,
    /// which is no locator.
    pub(super) fn locator_statement(
        &self,
        st: &super::State,
        hold: bool,
        references: &[crate::ast::ValueSpecification],
        params: &crate::params::Params,
    ) -> Result<super::Completion> {
        for r in references {
            let (value, _) = self.value_in(st, r, params, Some(crate::params::ParameterType::Of(LOCATOR_TYPE)))?;
            let locator = if let crate::value::Value::Exact(d) = value { u32::try_from(d.units).unwrap_or(0) } else { 0 };
            if hold { self.hold_locator(locator)? } else { self.free_locator(locator)? }
        }
        Ok(super::Completion::Success)
    }

    /// The session's transaction ended, committed or not: its locators
    /// ended with it but those held through a commit.
    pub(super) fn locators_ended(&self, committed: bool) {
        self.locators.borrow_mut().ended(committed);
    }

    /// `values`, each large object a client sent by its locator the value
    /// it names, in arrays and rows too.
    ///
    /// # Errors
    ///
    /// SQLSTATE 0F001 if a locator names none.
    pub(super) fn unlocated(&self, values: &[crate::value::Value]) -> Result<Vec<crate::value::Value>> {
        values.iter().map(|v| writing::resolved(v, &mut |l| self.located(l))).collect()
    }
}

// Unit tests: locators assigned, freed, held and ended, and values made a
// piece at a time (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
