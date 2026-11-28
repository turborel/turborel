//! \[DESIGN\] (RULES.md D356) The keys of a table's groups, a lane each
//! grouping column: each group's value of it as its first row gave it, in
//! a vector of its type, integers of one scale, approximate numbers,
//! datetimes, truth values or texts, and of any other type the values
//! with the bytes `spill::put_norm` gives of each. Two keys are equal as
//! those bytes are, as `exec/group.rs` and the spill key them: integers of
//! the lane's scale by their units, approximate numbers by their bits with
//! -0 as 0, datetimes by their instant, texts without their trailing
//! spaces, nulls equal to each other alone. A value a lane's type cannot
//! hold, an integer of another scale or a value of another type, turns
//! the lane into values once, its groups' rewritten, so the lanes never
//! disagree with those bytes. A group's value is given as its first row
//! gave it: a CHARACTER string with its spaces, a datetime in its zone.
//!
//! \[DESIGN\] (RULES.md D1274) A lane of integers is compared by its
//! words where the key gives words (`words.rs`), and a lane that holds
//! no null reads nothing to learn that a group's value is not one.
//!
//! \[DESIGN\] (RULES.md D1255) A lane's texts lie in chunks that never
//! move (`texts.rs`).

use crate::exec::batch::put_key;
use crate::storage::{Batch, Cell, Row};
use crate::value::datetime::Moment;
use crate::value::{Datetime, Value};

use super::Seed;
use super::hash::canonical;

mod written;
pub(super) use written::written_alone;
mod columns;
mod texts;
mod words;
use texts::Chunks;

/// A key given to a table: of a row of a batch, the columns at these
/// places of the row at this place; of values; or of a group of another
/// table.
#[derive(Clone, Copy)]
pub(in crate::exec) enum Key<'a> {
    Row(&'a Batch, &'a [usize], usize),
    Values(&'a [Value]),
    Group(&'a Keys, usize),
}

impl<'a> Key<'a> {
    /// Its value of the `lane`th grouping column, as a cell.
    fn cell(self, lane: usize) -> Cell<'a> {
        match self {
            Key::Row(batch, at, r) => at.get(lane).map_or(Cell::Null, |&c| batch.cell(c, r)),
            Key::Values(values) => values.get(lane).map_or(Cell::Null, Cell::Value),
            Key::Group(keys, g) => keys.lanes.get(lane).map_or(Cell::Null, |l| l.cell(g)),
        }
    }

    /// Appends to `out` its bytes of `width` columns, as `put_key` gives
    /// them, as a spill partitions it.
    pub(in crate::exec) fn put(self, width: usize, out: &mut Vec<u8>) {
        (0..width).for_each(|lane| put_key(self.cell(lane), out));
    }
}

/// A datetime no group holds, in the place of a null's.
const NO_DATETIME: Datetime = Datetime { moment: Moment::Date, utc: 0, zone: 0, zoned: false, precision: 0 };

/// A lane's values, by type.
#[derive(Debug, Default)]
enum Data {
    /// No value yet, every group's null.
    #[default]
    Unset,
    Ints {
        units: Vec<i64>,
        scale: u32,
    },
    Floats(Vec<f64>),
    Datetimes(Vec<Datetime>),
    Bools(Vec<bool>),
    /// Character strings, in chunks that never move (RULES.md D1255).
    Texts(Chunks),
    Values {
        values: Vec<Value>,
        keys: Vec<u8>,
        ends: Vec<usize>,
    },
}

/// The exact number of `cell`, units and scale, if it is one.
fn exact_of(cell: Cell<'_>) -> Option<(i128, u32)> {
    match cell {
        Cell::Exact(units, scale) => Some((i128::from(units), scale)),
        Cell::Value(Value::Exact(d)) => Some((d.units, d.scale)),
        Cell::Value(_) | Cell::Approx(_) | Cell::Text(_) | Cell::Datetime(_) | Cell::Bool(_) | Cell::Null => None,
    }
}

/// The approximate number of `cell`, if it is one.
fn approx_of(cell: Cell<'_>) -> Option<f64> {
    match cell {
        Cell::Approx(f) | Cell::Value(&Value::Approx(f)) => Some(f),
        Cell::Value(_) | Cell::Exact(..) | Cell::Text(_) | Cell::Datetime(_) | Cell::Bool(_) | Cell::Null => None,
    }
}

/// The datetime of `cell`, if it is one.
fn datetime_of(cell: Cell<'_>) -> Option<Datetime> {
    match cell {
        Cell::Datetime(d) | Cell::Value(&Value::Datetime(d)) => Some(d),
        Cell::Value(_) | Cell::Exact(..) | Cell::Approx(_) | Cell::Text(_) | Cell::Bool(_) | Cell::Null => None,
    }
}

/// The truth value of `cell`, if it is one.
fn bool_of(cell: Cell<'_>) -> Option<bool> {
    match cell {
        Cell::Bool(b) | Cell::Value(&Value::Bool(b)) => Some(b),
        Cell::Value(_) | Cell::Exact(..) | Cell::Approx(_) | Cell::Text(_) | Cell::Datetime(_) | Cell::Null => None,
    }
}

/// The text of `cell`, if it is a character string.
fn text_of(cell: Cell<'_>) -> Option<&str> {
    match cell {
        Cell::Text(text) => Some(text),
        Cell::Value(Value::Char(s)) => Some(s),
        Cell::Value(_) | Cell::Exact(..) | Cell::Approx(_) | Cell::Datetime(_) | Cell::Bool(_) | Cell::Null => None,
    }
}

/// `text` as strings compare padded, without its trailing spaces
/// [X3.135-1986 §5.11 GR5].
fn trimmed(text: &str) -> &str {
    text.trim_end_matches(' ')
}

impl Data {
    /// The lane of `cell`'s type, its first `nulls` groups null.
    fn of(cell: Cell<'_>, nulls: usize) -> Data {
        let mut data = if let Some((_, scale)) = exact_of(cell) {
            Data::Ints { units: Vec::new(), scale }
        } else if approx_of(cell).is_some() {
            Data::Floats(Vec::new())
        } else if datetime_of(cell).is_some() {
            Data::Datetimes(Vec::new())
        } else if bool_of(cell).is_some() {
            Data::Bools(Vec::new())
        } else if text_of(cell).is_some() {
            Data::Texts(Chunks::default())
        } else {
            Data::Values { values: Vec::new(), keys: Vec::new(), ends: Vec::new() }
        };
        (0..nulls).for_each(|_| data.push_null());
        data
    }

    /// Appends `cell`, not null: whether its type holds it.
    fn push(&mut self, cell: Cell<'_>) -> bool {
        match self {
            Data::Unset => false,
            Data::Ints { units, scale } => {
                let fits = exact_of(cell).filter(|&(_, s)| s == *scale).and_then(|(u, _)| i64::try_from(u).ok());
                fits.map(|u| units.push(u)).is_some()
            }
            Data::Floats(numbers) => approx_of(cell).map(|f| numbers.push(f)).is_some(),
            Data::Datetimes(datetimes) => datetime_of(cell).map(|d| datetimes.push(d)).is_some(),
            Data::Bools(bools) => bool_of(cell).map(|b| bools.push(b)).is_some(),
            Data::Texts(texts) => text_of(cell).is_some_and(|t| texts.push(t)),
            Data::Values { values, keys, ends } => {
                put_key(cell, keys);
                ends.push(keys.len());
                values.push(cell.to_value());
                true
            }
        }
    }

    /// Appends a null's place.
    fn push_null(&mut self) {
        match self {
            Data::Unset => {}
            Data::Ints { units, .. } => units.push(0),
            Data::Floats(numbers) => numbers.push(0.0_f64),
            Data::Datetimes(datetimes) => datetimes.push(NO_DATETIME),
            Data::Bools(bools) => bools.push(false),
            Data::Texts(texts) => texts.push_null(),
            Data::Values { values, keys, ends } => {
                put_key(Cell::Null, keys);
                ends.push(keys.len());
                values.push(Value::Null);
            }
        }
    }
}

/// A grouping column's values of a table's groups, and which are null.
#[derive(Debug, Default)]
pub(super) struct Lane {
    /// [ISO/IEC 9075-2:2023 §4.3.2] Whether its character strings are
    /// equal only if their octets are, no space padding either, as a
    /// column grouped under a collation of NO PAD that orders strings as
    /// their octets do (RULES.md D1183).
    unpadded: bool,
    data: Data,
    nulls: Vec<bool>,
    /// \[DESIGN\] (RULES.md D1274) Whether a group's may be null. While
    /// none was, `nulls` is not read to tell that a group's is not.
    nulled: bool,
    /// What its values hold apart from themselves (`spill::heap_bytes`).
    heap: usize,
    /// The groups its vector is given room for when its first value
    /// tells its type.
    room: usize,
}

impl Lane {
    /// Whether group `g`'s is null.
    fn is_null(&self, g: usize) -> bool {
        self.nulled && self.nulls.get(g).copied().unwrap_or(true)
    }

    /// Group `g`'s value, as a cell.
    fn cell(&self, g: usize) -> Cell<'_> {
        if self.is_null(g) {
            return Cell::Null;
        }
        match &self.data {
            Data::Unset => None,
            Data::Ints { units, scale } => units.get(g).map(|&u| Cell::Exact(u, *scale)),
            Data::Floats(numbers) => numbers.get(g).map(|&f| Cell::Approx(f)),
            Data::Datetimes(datetimes) => datetimes.get(g).map(|&d| Cell::Datetime(d)),
            Data::Bools(bools) => bools.get(g).map(|&b| Cell::Bool(b)),
            Data::Texts(texts) => texts.get(g).map(Cell::Text),
            Data::Values { values, .. } => values.get(g).map(Cell::Value),
        }
        .unwrap_or(Cell::Null)
    }

    /// Whether group `g`'s equals `cell`, as their keys' bytes are
    /// (`put_key`), the bytes made in `scratch` only where the lane cannot
    /// tell by its own values.
    fn equal(&self, g: usize, cell: Cell<'_>, scratch: &mut Vec<u8>) -> bool {
        let (null, other) = (self.is_null(g), cell.is_null());
        if null || other {
            return null && other;
        }
        if self.unpadded
            && let (Some(mine), Some(theirs)) = (text_of(self.cell(g)), text_of(cell))
        {
            return mine == theirs;
        }
        if let Some(equal) = self.equal_typed(g, cell) {
            return equal;
        }
        scratch.clear();
        put_key(cell, scratch);
        let split = scratch.len();
        if let Data::Values { keys, ends, .. } = &self.data {
            let from = g.checked_sub(1).map_or(Some(0), |before| ends.get(before).copied());
            return from.zip(ends.get(g)).and_then(|(from, &to)| keys.get(from..to)) == Some(scratch.as_slice());
        }
        put_key(self.cell(g), scratch);
        scratch.get(..split) == scratch.get(split..)
    }

    /// Whether group `g`'s, not null, equals `cell`, not null, if the lane
    /// tells by its values: of its own type, an integer of its scale.
    fn equal_typed(&self, g: usize, cell: Cell<'_>) -> Option<bool> {
        match &self.data {
            Data::Ints { units, scale } => {
                let (u, s) = exact_of(cell)?;
                (s == *scale).then(|| units.get(g).map(|&x| i128::from(x)) == Some(u))
            }
            Data::Floats(numbers) => Some(numbers.get(g).map(|&f| canonical(f)) == Some(canonical(approx_of(cell)?))),
            Data::Datetimes(datetimes) => Some(datetimes.get(g).map(|d| d.utc) == Some(datetime_of(cell)?.utc)),
            Data::Bools(bools) => Some(bools.get(g).copied() == Some(bool_of(cell)?)),
            Data::Texts(texts) => Some(texts.get(g).map(trimmed) == Some(trimmed(text_of(cell)?))),
            Data::Unset | Data::Values { .. } => None,
        }
    }

    /// Appends `cell` as a new group's, the lane turned into values first
    /// if its type cannot hold it.
    fn push(&mut self, cell: Cell<'_>) {
        if cell.is_null() {
            self.data.push_null();
            self.nulls.push(true);
            self.nulled = true;
            return;
        }
        if let Data::Unset = self.data {
            self.data = Data::of(cell, self.nulls.len());
            self.reserve_data(self.room);
        }
        if !self.data.push(cell) {
            self.widened();
            self.data.push(cell);
        }
        if let Data::Values { values, .. } = &self.data {
            self.heap = self.heap.saturating_add(values.last().map_or(0, crate::spill::heap_bytes));
        }
        self.nulls.push(false);
    }

    /// \[DESIGN\] (RULES.md D1110, D1255) Appends the values at `places`
    /// of `other` as new groups': integers of its scale in one loop,
    /// texts each copied once into its chunks, any other a value at a
    /// time.
    fn extend(&mut self, other: &Lane, places: &[u32]) {
        self.reserve(other, places.len());
        if let (Data::Texts(texts), Data::Texts(theirs)) = (&mut self.data, &other.data) {
            texts.extend(theirs, places);
            self.nulls.extend(places.iter().map(|&i| other.is_null(crate::storage::mask::index(i))));
            self.nulled |= other.nulled;
            return;
        }
        if let (Data::Ints { units, scale }, Data::Ints { units: theirs, scale: other_scale }) = (&mut self.data, &other.data)
            && scale == other_scale
        {
            units.extend(places.iter().map(|&i| theirs.get(crate::storage::mask::index(i)).copied().unwrap_or(0)));
            self.nulls.extend(places.iter().map(|&i| other.is_null(crate::storage::mask::index(i))));
            self.nulled = self.nulled || other.nulled;
            return;
        }
        for &i in places {
            self.push(other.cell(crate::storage::mask::index(i)));
        }
    }

    /// Room for `more` groups more: a lane of no group yet takes the
    /// type of `like`, of integers or of texts, so its vector is made
    /// once and its values are appended as that type's.
    fn reserve(&mut self, like: &Lane, more: usize) {
        self.nulls.reserve(more);
        if let (Data::Unset, true) = (&self.data, self.nulls.is_empty()) {
            self.data = match &like.data {
                Data::Ints { scale, .. } => Data::Ints { units: Vec::new(), scale: *scale },
                Data::Texts(_) => Data::Texts(Chunks::default()),
                Data::Unset | Data::Floats(_) | Data::Datetimes(_) | Data::Bools(_) | Data::Values { .. } => Data::Unset,
            };
        }
        self.reserve_data(more);
    }

    /// \[DESIGN\] (RULES.md D1255) Its groups in the order of `places`,
    /// each the number of a group: a lane of texts with its texts where
    /// they are, only what tells where each lies put in that order; any
    /// other lane made anew of its values in that order. No group turns
    /// null by it, so whether one may be null is as it was (RULES.md
    /// D1274).
    fn gathered(mut self, places: &[u32]) -> Lane {
        if let Data::Texts(texts) = &mut self.data {
            texts.gather(places);
            self.nulls = places.iter().map(|&i| self.nulls.get(crate::storage::mask::index(i)).copied().unwrap_or(false)).collect();
            return self;
        }
        let mut lane = Lane { unpadded: self.unpadded, ..Lane::default() };
        lane.extend(&self, places);
        lane
    }

    /// Room for `more` values more in its vector, of a type.
    fn reserve_data(&mut self, more: usize) {
        match &mut self.data {
            Data::Ints { units, .. } => units.reserve(more),
            Data::Floats(numbers) => numbers.reserve(more),
            Data::Datetimes(datetimes) => datetimes.reserve(more),
            Data::Bools(bools) => bools.reserve(more),
            Data::Texts(texts) => texts.reserve(more),
            Data::Values { ends, .. } => ends.reserve(more),
            Data::Unset => {}
        }
    }

    /// Turns the lane into values, each group's as it was.
    fn widened(&mut self) {
        let mut data = Data::Values { values: Vec::new(), keys: Vec::new(), ends: Vec::new() };
        for g in 0..self.nulls.len() {
            let cell = self.cell(g);
            if cell.is_null() {
                data.push_null();
            } else {
                data.push(cell);
            }
        }
        self.data = data;
        self.heap = self.held();
    }

    /// Group `g`'s value.
    fn value(&self, g: usize) -> Value {
        self.cell(g).to_value()
    }

    /// Keeps its first `len` groups, the room of the others given back.
    fn truncate(&mut self, len: usize) {
        self.shorten(len);
        self.nulls.shrink_to_fit();
        match &mut self.data {
            Data::Unset => {}
            Data::Ints { units, .. } => units.shrink_to_fit(),
            Data::Floats(numbers) => numbers.shrink_to_fit(),
            Data::Datetimes(datetimes) => datetimes.shrink_to_fit(),
            Data::Bools(bools) => bools.shrink_to_fit(),
            Data::Texts(texts) => texts.shrink(),
            Data::Values { values, keys, ends } => {
                values.shrink_to_fit();
                keys.shrink_to_fit();
                ends.shrink_to_fit();
            }
        }
    }

    /// Keeps its first `len` groups.
    fn shorten(&mut self, len: usize) {
        self.nulls.truncate(len);
        let end = |ends: &[usize]| len.checked_sub(1).and_then(|last| ends.get(last)).copied().unwrap_or(0);
        match &mut self.data {
            Data::Unset => {}
            Data::Ints { units, .. } => units.truncate(len),
            Data::Floats(numbers) => numbers.truncate(len),
            Data::Datetimes(datetimes) => datetimes.truncate(len),
            Data::Bools(bools) => bools.truncate(len),
            Data::Texts(texts) => texts.shorten(len),
            Data::Values { values, keys, ends } => {
                keys.truncate(end(ends));
                values.truncate(len);
                ends.truncate(len);
            }
        }
        self.heap = self.held();
    }

    /// What its values hold apart from themselves, counted anew.
    fn held(&self) -> usize {
        let Data::Values { values, .. } = &self.data else { return 0 };
        values.iter().map(crate::spill::heap_bytes).sum()
    }

    /// The bytes it holds: its vectors' room, and what its values hold.
    fn bytes(&self) -> usize {
        let held = match &self.data {
            Data::Unset => 0,
            Data::Ints { units, .. } => units.capacity().saturating_mul(size_of::<i64>()),
            Data::Floats(numbers) => numbers.capacity().saturating_mul(size_of::<f64>()),
            Data::Datetimes(datetimes) => datetimes.capacity().saturating_mul(size_of::<Datetime>()),
            Data::Bools(bools) => bools.capacity(),
            Data::Texts(texts) => texts.bytes(),
            Data::Values { values, keys, ends } => {
                let made = values.capacity().saturating_mul(size_of::<Value>()).saturating_add(self.heap);
                made.saturating_add(keys.capacity()).saturating_add(ends.capacity().saturating_mul(size_of::<usize>()))
            }
        };
        held.saturating_add(self.nulls.capacity())
    }
}

/// The keys of a table's groups: a lane of each grouping column.
#[derive(Debug, Default)]
pub(in crate::exec) struct Keys {
    lanes: Vec<Lane>,
}

impl Keys {
    /// No group's keys yet, of `width` columns.
    pub(in crate::exec) fn new(width: usize) -> Keys {
        Keys { lanes: (0..width).map(|_| Lane::default()).collect() }
    }

    /// As `new`, each lane with room for `room` groups once its first
    /// value tells its type.
    pub(in crate::exec) fn with_room(width: usize, room: usize) -> Keys {
        Keys { lanes: (0..width).map(|_| Lane { nulls: Vec::with_capacity(room), room, ..Lane::default() }).collect() }
    }

    /// How many columns.
    pub(super) fn width(&self) -> usize {
        self.lanes.len()
    }

    /// \[DESIGN\] (RULES.md D1110, D1255) Its groups' keys in the order
    /// of `places`, each the number of a group, each column compared as
    /// it was: a lane at a time, texts left where they are.
    pub(super) fn gathered(self, places: &[u32]) -> Keys {
        Keys { lanes: self.lanes.into_iter().map(|lane| lane.gathered(places)).collect() }
    }

    /// These keys, each column `seed` says is grouped unpadded compared so
    /// (`Seed::is_unpadded`).
    pub(super) fn of_seed(mut self, seed: &Seed) -> Keys {
        for (place, lane) in self.lanes.iter_mut().enumerate() {
            lane.unpadded = seed.is_unpadded(place);
        }
        self
    }

    /// Whether group `g`'s key equals `key`. A lane of integers tells by
    /// its words where `key` gives words (RULES.md D1274), any other by
    /// its cell, bytes made in `scratch` where a lane cannot tell by its
    /// values.
    pub(in crate::exec) fn equal(&self, g: usize, key: Key<'_>, scratch: &mut Vec<u8>) -> bool {
        let by_cell = |at: usize, lane: &Lane, bytes: &mut Vec<u8>| lane.equal(g, key.cell(at), bytes);
        self.lanes.iter().enumerate().all(|(at, lane)| lane.equal_words(g, key, at).unwrap_or_else(|| by_cell(at, lane, scratch)))
    }

    /// Appends `key` as a new group's.
    pub(in crate::exec) fn push(&mut self, key: Key<'_>) {
        for (at, lane) in self.lanes.iter_mut().enumerate() {
            lane.push(key.cell(at));
        }
    }

    /// Appends the keys at `places` of `other` as new groups', a lane
    /// at a time.
    pub(super) fn extend(&mut self, other: &Keys, places: &[u32]) {
        for (lane, theirs) in self.lanes.iter_mut().zip(&other.lanes) {
            lane.extend(theirs, places);
        }
    }

    /// Room for `more` groups more, in lanes as `like`'s.
    pub(super) fn reserve(&mut self, like: &Keys, more: usize) {
        for (lane, theirs) in self.lanes.iter_mut().zip(&like.lanes) {
            lane.reserve(theirs, more);
        }
    }

    /// Group `g`'s grouping values.
    pub(in crate::exec) fn values(&self, g: usize) -> Row {
        self.lanes.iter().map(|lane| lane.value(g)).collect()
    }

    /// Appends group `g`'s grouping values to `row`.
    pub(super) fn values_into(&self, g: usize, row: &mut Row) {
        row.extend(self.lanes.iter().map(|lane| lane.value(g)));
    }

    /// \[DESIGN\] (RULES.md D1113) The hash of group `g`'s key of the
    /// lanes `lanes` alone, by `seed`, as `hash_values` hashes those values
    /// of any vector: so the groups of every morsel that agree on them
    /// agree on it.
    pub(super) fn hash_of(&self, g: usize, lanes: &[usize], seed: &Seed) -> u64 {
        let mut hashes = lanes.iter().map(|&at| self.lanes.get(at).map_or_else(|| seed.null(), |lane| lane.hash(g, at, seed)));
        let first = hashes.next().unwrap_or_else(|| seed.null());
        hashes.fold(first, |so_far, next| seed.joined(so_far, next))
    }

    /// Keeps the first `len` groups.
    pub(in crate::exec) fn truncate(&mut self, len: usize) {
        self.lanes.iter_mut().for_each(|lane| lane.truncate(len));
    }

    /// The bytes its lanes hold.
    pub(super) fn bytes(&self) -> usize {
        self.lanes.iter().map(Lane::bytes).sum()
    }
}

// Unit tests: lanes equal as the keys' bytes are (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
