//! \[DESIGN\] (RULES.md D1255) A lane's character strings, held in chunks
//! that never move. A chunk is a buffer made once with its room and
//! filled no further than that, so a text written stays where it was
//! written. A lane that grows copies no text it holds, and gives no
//! buffer back to take a larger one. A group's text is told by its chunk
//! and where it lies there, so groups put in another order (`gather`)
//! keep their texts where they are, and no byte of them is copied.
//!
//! A new chunk is made with room for as much text as the lane holds, at
//! least `FIRST` and at most `MOST`. So a lane of few short texts holds
//! little, a lane of many makes few chunks, and no lane holds more room
//! unfilled than its texts fill or than `MOST`. A text longer than `MOST`
//! has a chunk of its own, of its length, and the chunk open before it
//! stays open. Nothing here is sized by an estimate of the groups, which
//! a planner misses by far where the keys are texts.
//!
//! The room of every chunk is counted as the chunk is made (`bytes`), so
//! what holds the lane charges its grant with room before a text fills
//! it.

use crate::storage::mask::index;

/// \[DESIGN\] The room of a lane's first chunk, in bytes. A lane of a few
/// short texts, a table of a handful of groups, holds one small buffer
/// and no more. Measured on lanes of four short texts made and merged by
/// the hundred thousand, a first chunk of 64 bytes to 4 KiB cost the
/// same, so the choice is of memory alone: a kibibyte a lane, which a
/// grouping's partitions hold of each text column while their keys are
/// few.
const FIRST: usize = 1 << 10;

/// \[DESIGN\] The most room a chunk is made with, in bytes, but for a
/// text longer than it. Measured on the lanes of a grouping of tens of
/// millions of long text keys, made by eight threads and merged into
/// their partitions, chunks of at most 32 KiB to 16 MiB cost the same,
/// and of 16 KiB the merge cost more. What chooses among them is the
/// statement that runs next. A grouping's chunks are freed when it ends,
/// and an allocator that keeps freed memory hands the next statement
/// only the blocks its requests fit. Measured in the server, a grouping
/// of integers run after groupings of long texts took more fresh memory
/// and more time after chunks of 64 KiB than after chunks of 1 MiB or
/// 16 MiB, which read alike. So a chunk is a mebibyte at most, the least
/// of those that leave the next statement room it can use. A lane holds
/// one chunk part filled, so a grouping's partitions hold at most a
/// mebibyte a text column each unfilled, and only once each holds that
/// much text.
const MOST: usize = 1 << 20;

/// Where a group's text lies: its chunk, and from where to where in it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Span {
    chunk: u32,
    from: u32,
    to: u32,
}

/// A lane's texts in chunks, and where each group's lies.
#[derive(Debug, Default)]
pub(super) struct Chunks {
    /// The chunks, each made with its room and never grown.
    buffers: Vec<String>,
    /// Each group's text, a null's empty.
    spans: Vec<Span>,
    /// The chunk texts are appended to.
    open: usize,
    /// The bytes of text its chunks hold.
    held: usize,
    /// The bytes of room its chunks were made with.
    room: usize,
}

impl Chunks {
    /// No text yet, with one chunk of room for `bytes` of text, as much as
    /// a chunk tells of, so texts whose whole length is known lie in one
    /// buffer of that length.
    pub(super) fn with_room(bytes: usize) -> Chunks {
        let mut texts = Chunks::default();
        let room = bytes.min(index(u32::MAX));
        texts.buffers.push(String::with_capacity(room));
        texts.room = room;
        texts
    }

    /// Group `g`'s text.
    pub(super) fn get(&self, g: usize) -> Option<&str> {
        let span = self.spans.get(g)?;
        self.buffers.get(index(span.chunk))?.get(index(span.from)..index(span.to))
    }

    /// Appends `text` as a new group's: whether the lane can tell where
    /// it lies, which it cannot of a text of four gibibytes or more.
    pub(super) fn push(&mut self, text: &str) -> bool {
        if u32::try_from(text.len()).is_err() {
            return false;
        }
        let fits = self.buffers.get(self.open).is_some_and(|chunk| chunk.capacity().saturating_sub(chunk.len()) >= text.len());
        let at = if fits { self.open } else { self.opened(text.len()) };
        let Some(chunk) = self.buffers.get_mut(at) else { return false };
        let from = chunk.len();
        chunk.push_str(text);
        let (Ok(place), Ok(start), Ok(end)) = (u32::try_from(at), u32::try_from(from), u32::try_from(chunk.len())) else { return false };
        self.spans.push(Span { chunk: place, from: start, to: end });
        self.held = self.held.saturating_add(text.len());
        true
    }

    /// Makes the chunk a text of `len` bytes is appended to, and gives
    /// its place. A text longer than `MOST` takes a chunk of its own and
    /// the open chunk stays open. Any other opens a new chunk, with room
    /// for as much text as the lane holds, between `FIRST` and `MOST`.
    fn opened(&mut self, len: usize) -> usize {
        let at = self.buffers.len();
        let room = if len > MOST { len } else { self.held.clamp(FIRST, MOST).max(len) };
        self.buffers.push(String::with_capacity(room));
        self.room = self.room.saturating_add(room);
        if len <= MOST {
            self.open = at;
        }
        at
    }

    /// Appends a null's place, an empty text where the next would begin.
    pub(super) fn push_null(&mut self) {
        let end = self.buffers.get(self.open).map_or(0, String::len);
        let told = u32::try_from(self.open).ok().zip(u32::try_from(end).ok());
        self.spans.push(told.map_or_else(Span::default, |(chunk, at)| Span { chunk, from: at, to: at }));
    }

    /// Appends the texts of `other`'s groups at `places` as new groups',
    /// in that order, a null's empty. Each is copied once, into a chunk
    /// that is not grown.
    pub(super) fn extend(&mut self, other: &Chunks, places: &[u32]) {
        self.spans.reserve(places.len());
        for &i in places {
            // A text another lane tells is one this lane can tell.
            if !self.push(other.get(index(i)).unwrap_or("")) {
                self.push_null();
            }
        }
    }

    /// Puts its groups in the order of `places`, each the number of a
    /// group. Their texts stay where they are.
    pub(super) fn gather(&mut self, places: &[u32]) {
        self.spans = places.iter().map(|&i| self.spans.get(index(i)).copied().unwrap_or_default()).collect();
    }

    /// Room for `more` groups more. Their texts' room is made as they
    /// come, a chunk at a time.
    pub(super) fn reserve(&mut self, more: usize) {
        self.spans.reserve(more);
    }

    /// Gives back the room no text fills. That is the open chunk's alone,
    /// as every other chunk is full or one text's.
    pub(super) fn shrink(&mut self) {
        self.spans.shrink_to_fit();
        self.buffers.shrink_to_fit();
        if let Some(chunk) = self.buffers.get_mut(self.open) {
            let before = chunk.capacity();
            chunk.shrink_to_fit();
            self.room = self.room.saturating_sub(before).saturating_add(chunk.capacity());
        }
    }

    /// Keeps its first `len` groups. The chunks past the last text kept
    /// are let go, and the chunk that holds it ends where it ends.
    pub(super) fn shorten(&mut self, len: usize) {
        if len >= self.spans.len() {
            return;
        }
        self.spans.truncate(len);
        let Some((chunk, to)) = self.spans.iter().map(|span| (span.chunk, span.to)).max() else {
            *self = Chunks::default();
            return;
        };
        self.buffers.truncate(index(chunk).saturating_add(1));
        if let Some(last) = self.buffers.last_mut() {
            last.truncate(index(to));
        }
        self.open = self.open.min(self.buffers.len().saturating_sub(1));
        self.held = self.buffers.iter().map(String::len).sum();
        self.room = self.buffers.iter().map(String::capacity).sum();
    }

    /// The bytes it holds: its chunks' room, and what tells where each
    /// group's text lies.
    pub(super) fn bytes(&self) -> usize {
        let told = self.spans.capacity().saturating_mul(size_of::<Span>());
        self.room.saturating_add(told).saturating_add(self.buffers.capacity().saturating_mul(size_of::<String>()))
    }

    /// The bytes of text of the groups at `places`.
    pub(super) fn held_at(&self, places: &[u32]) -> usize {
        let len = |&i: &u32| self.spans.get(index(i)).map_or(0, |span| index(span.to.saturating_sub(span.from)));
        places.iter().map(len).fold(0, usize::saturating_add)
    }

    /// Its texts one after another in one buffer, and where each ends
    /// there, as a column holds them. Texts that lie so in one chunk are
    /// that chunk as it is. Any others are copied once, into a buffer
    /// made with their whole length.
    pub(super) fn joined(mut self) -> (String, Vec<usize>) {
        let follow = |end: u32, span: &Span| (span.from == end).then_some(span.to);
        if self.buffers.len() == 1 && self.spans.iter().try_fold(0_u32, follow).is_some() {
            let mut text = self.buffers.pop().unwrap_or_default();
            text.truncate(self.spans.last().map_or(0, |span| index(span.to)));
            return (text, self.spans.iter().map(|span| index(span.to)).collect());
        }
        let whole = self.spans.iter().map(|span| index(span.to.saturating_sub(span.from))).fold(0, usize::saturating_add);
        let (mut text, mut ends) = (String::with_capacity(whole), Vec::with_capacity(self.spans.len()));
        for g in 0..self.spans.len() {
            text.push_str(self.get(g).unwrap_or(""));
            ends.push(text.len());
        }
        (text, ends)
    }
}

// Unit tests: texts stay where they were written (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
