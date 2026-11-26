//! The web log's rows (`weblog.rs`): each made from the run's seed and its
//! own number, so any part of the table can be made alone and on any
//! thread. A row is a page view: whose it is and of which counter's site
//! (`layout.rs` lays the rows out in the order of the counter, the day
//! and the user), what was viewed and how the visitor came to it
//! (`keys.rs` draws the page, the referer and the search phrase from the
//! spectra measured of a reference log), and with what browser
//! (`plain.rs`).

use turborel::hash::xxhash::xxh3_64;
use turborel::storage::lanes::{Lane, Lanes};

use super::columns::{PLAIN, WORDED};
use super::keys::{Keys, View};
use super::measured;
use super::plain::By;
use super::queries::Constants;
use super::random::{Random, drawn};
use super::schema::{COLUMNS, WIDTH};

/// The purposes draws are made for, each its own, so no two agree.
const ROW: u64 = 1;
const COLUMN: u64 = 1000;
/// Seconds of a minute, an hour and a day.
const MINUTE: u64 = 60;
const HOUR: u64 = 3_600;
const DAY: u64 = 86_400;

/// What writes a column of a row.
#[derive(Clone, Copy)]
enum Source {
    /// The measured column of whole numbers at this place of `PLAIN`.
    Plain(usize),
    /// The measured column of texts at this place of `WORDED`.
    Worded(usize),
    /// A column of the view itself.
    Key(Key),
}

/// The columns of the view itself.
#[derive(Clone, Copy)]
enum Key {
    WatchId,
    Title,
    EventTime,
    EventDate,
    CounterId,
    ClientIp,
    RegionId,
    UserId,
    Url,
    Referer,
    SearchEngineId,
    SearchPhrase,
    TraficSourceId,
    ClientEventTime,
    LocalEventTime,
    RefererHash,
    UrlHash,
}

/// The view's columns by name.
const KEYS: [(&str, Key); 17] = [
    ("WatchID", Key::WatchId),
    ("Title", Key::Title),
    ("EventTime", Key::EventTime),
    ("EventDate", Key::EventDate),
    ("CounterID", Key::CounterId),
    ("ClientIP", Key::ClientIp),
    ("RegionID", Key::RegionId),
    ("UserID", Key::UserId),
    ("URL", Key::Url),
    ("Referer", Key::Referer),
    ("SearchEngineID", Key::SearchEngineId),
    ("SearchPhrase", Key::SearchPhrase),
    ("TraficSourceID", Key::TraficSourceId),
    ("ClientEventTime", Key::ClientEventTime),
    ("LocalEventTime", Key::LocalEventTime),
    ("RefererHash", Key::RefererHash),
    ("URLHash", Key::UrlHash),
];

/// Makes the rows of HITS.
pub struct Maker {
    rows: u64,
    seed: u64,
    keys: Keys,
    sources: Vec<Source>,
}

/// A row's texts, made once and written to more than one column.
#[derive(Default)]
struct Texts {
    url: Vec<u8>,
    referer: Vec<u8>,
}

impl Maker {
    /// The maker of `rows` rows under `seed`.
    ///
    /// # Panics
    ///
    /// A column of HITS has no source, or a measured column is not one
    /// of HITS.
    pub fn new(rows: u64, seed: u64) -> Maker {
        let source = |name: &str| {
            let key = KEYS.iter().find(|(key, _)| *key == name).map(|(_, key)| Source::Key(*key));
            let plain = PLAIN.iter().position(|plain| plain.column == name).map(Source::Plain);
            let worded = WORDED.iter().position(|worded| worded.column == name).map(Source::Worded);
            key.or(plain).or(worded).unwrap()
        };
        Maker { rows, seed, keys: Keys::new(rows, seed), sources: COLUMNS.iter().map(|(name, _)| source(name)).collect() }
    }

    /// How many rows it makes.
    pub fn rows(&self) -> u64 {
        self.rows
    }

    /// What tells its rows from another maker's: the rows, the seed, and
    /// the day the figures were measured.
    pub fn label(&self) -> String {
        format!("{} rows, seed {}, figures of {}", self.rows, self.seed, measured::MEASURED)
    }

    /// The `count` rows from row `first`, by column, as text.
    pub fn lanes(&self, first: u64, count: u64) -> Lanes {
        let rows = count as usize;
        let empty = || Lane { nulls: vec![0; rows.div_ceil(8)], ends: Vec::with_capacity(rows), bytes: Vec::new() };
        let mut lanes = Lanes { rows, columns: (0..WIDTH).map(|_| empty()).collect() };
        let mut texts = Texts::default();
        for row in first..first + count {
            let mut draws = Random::new(drawn(self.seed, row, ROW));
            let view = self.keys.view(row, &mut draws);
            texts.url.clear();
            texts.referer.clear();
            self.keys.url(&mut texts.url, view.place.cell.counter, view.page);
            if let Some((site, page)) = view.referer {
                self.keys.referer(&mut texts.referer, site, page);
            }
            for (column, source) in self.sources.iter().enumerate() {
                let lane = &mut lanes.columns[column];
                self.cell(&mut lane.bytes, *source, (&view, &texts), draws.next());
                lane.ends.push(lane.bytes.len() as u32);
            }
        }
        lanes
    }

    /// Appends to `out` the text of the cell `source` writes of `view`,
    /// `draw` the row's draw for the column.
    fn cell(&self, out: &mut Vec<u8>, source: Source, (view, texts): (&View, &Texts), draw: u64) {
        match source {
            Source::Plain(at) => {
                let plain = &PLAIN[at];
                number(out, plain.value(self.drawn_by(plain.by, view, at as u64, draw)));
            }
            Source::Worded(at) => {
                let worded = &WORDED[at];
                worded.write(out, self.drawn_by(worded.by, view, (at + PLAIN.len()) as u64, draw), self.seed);
            }
            Source::Key(key) => self.key(out, key, view, texts),
        }
    }

    /// The draw of a column, the `column`th measured one, drawn by `by`:
    /// the row's own `draw`, or the one that belongs to the view's user or
    /// counter.
    fn drawn_by(&self, by: By, view: &View, column: u64, draw: u64) -> u64 {
        match by {
            By::Row => draw,
            By::User => drawn(self.seed, view.place.user, COLUMN + column),
            By::Counter => drawn(self.seed, view.place.cell.counter.cast_unsigned(), COLUMN + column),
        }
    }

    /// Appends to `out` the text of the view's own column `key`.
    fn key(&self, out: &mut Vec<u8>, key: Key, view: &View, texts: &Texts) {
        match key {
            Key::WatchId => number(out, view.watch),
            Key::Title => self.keys.title(out, view.place.cell.counter, view.page),
            Key::EventTime => stamp(out, view.seconds),
            Key::EventDate => date(out, view.seconds + measured::BEHIND),
            Key::CounterId => number(out, view.place.cell.counter),
            Key::ClientIp => number(out, view.address),
            Key::RegionId => number(out, view.region),
            Key::UserId => number(out, view.place.user_id),
            Key::Url => out.extend_from_slice(&texts.url),
            Key::Referer => out.extend_from_slice(&texts.referer),
            Key::SearchEngineId => number(out, view.engine),
            Key::SearchPhrase => view.phrase.into_iter().for_each(|phrase| self.keys.phrase(out, phrase)),
            Key::TraficSourceId => number(out, view.source),
            Key::ClientEventTime => stamp(out, view.client_seconds),
            Key::LocalEventTime => stamp(out, view.local_seconds),
            Key::RefererHash => number(out, hashed(&texts.referer)),
            Key::UrlHash => number(out, hashed(&texts.url)),
        }
    }

    /// The constants the queries' texts take for these rows: a user of a
    /// few views, the hashes of the most viewed page of the counter the
    /// last queries group and of the most frequent referer, and the
    /// original's thresholds and skipped rows as the same shares of these
    /// rows, one at least.
    pub fn constants(&self) -> Constants {
        let (mut url, mut referer) = (Vec::new(), Vec::new());
        self.keys.url(&mut url, measured::BUSY.0, 0);
        self.keys.referer(&mut referer, 0, 0);
        let share = |asked: u64| (asked * self.rows).div_euclid(measured::ASKED_ROWS).max(1);
        Constants {
            user: self.keys.modest_user(),
            referer: hashed(&referer),
            url: hashed(&url),
            having: share(measured::KEPT_OVER),
            skips: measured::SKIPPED.map(share),
        }
    }
}

/// The hash a text is known by: XXH3 of its octets, as a whole number of
/// 64 bits; of the empty text, 0.
fn hashed(text: &[u8]) -> i64 {
    if text.is_empty() { 0 } else { xxh3_64(text).cast_signed() }
}

/// Appends `value` to `out` in decimal digits.
fn number(out: &mut Vec<u8>, value: i64) {
    let mut digits = [0_u8; 20];
    let mut at = digits.len();
    let mut left = value.unsigned_abs();
    loop {
        at -= 1;
        digits[at] = b'0' + (left % 10) as u8;
        left = left.div_euclid(10);
        if left == 0 {
            break;
        }
    }
    if value < 0 {
        out.push(b'-');
    }
    out.extend_from_slice(&digits[at..]);
}

/// Appends `value` to `out` in two decimal digits.
fn two(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&[b'0' + (value.div_euclid(10) % 10) as u8, b'0' + (value % 10) as u8]);
}

/// Appends to `out` the date `seconds` after the log's first midnight.
fn date(out: &mut Vec<u8>, seconds: u64) {
    out.extend_from_slice(measured::MONTH.as_bytes());
    two(out, 1 + seconds.div_euclid(DAY));
}

/// Appends to `out` the timestamp `seconds` after the log's first
/// midnight.
fn stamp(out: &mut Vec<u8>, seconds: u64) {
    date(out, seconds);
    for (between, part) in [(b' ', seconds.div_euclid(HOUR) % 24), (b':', seconds.div_euclid(MINUTE) % 60), (b':', seconds % MINUTE)] {
        out.push(between);
        two(out, part);
    }
}
