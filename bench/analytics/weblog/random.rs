//! The web log's draws (`weblog.rs`). A row draws from SplitMix64 (Steele,
//! Lea and Flood, OOPSLA 2014), the generator the benches draw from
//! (`oltp/random.rs`), seeded from the run's seed and the row's number by
//! XXH3, so a row is the same whatever rows were made before it and on
//! whatever thread. A draw that depends only on what it is of, such as a
//! user's region, is XXH3 of that and its purpose. Values are drawn in
//! two shapes, a list of values with their shares and a spectrum of how
//! often values occur.

use turborel::hash::xxhash::xxh3_64_words;

/// SplitMix64's increment, the golden ratio's 64-bit fraction.
const GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;
/// SplitMix64's mixing multipliers.
const MIX_1: u64 = 0xbf58_476d_1ce4_e5b9;
const MIX_2: u64 = 0x94d0_49bb_1331_11eb;
/// The shares of a list are parts of this many.
pub const MILLION: u64 = 1_000_000;

/// A SplitMix64 sequence.
pub struct Random {
    state: u64,
}

impl Random {
    /// The sequence of `seed`.
    pub fn new(seed: u64) -> Random {
        Random { state: seed }
    }

    /// The next number.
    pub fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(GAMMA);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(MIX_1);
        z = (z ^ (z >> 27)).wrapping_mul(MIX_2);
        z ^ (z >> 31)
    }
}

/// The draw that belongs to `of`, for the purpose `purpose`, under `seed`,
/// the same each time it is asked.
pub fn drawn(seed: u64, of: u64, purpose: u64) -> u64 {
    xxh3_64_words(&[of, purpose], seed)
}

/// `draw`, a number of 64 bits, as one below `count`, each as likely.
pub fn below(draw: u64, count: u64) -> u64 {
    ((u128::from(draw) * u128::from(count)) >> 64) as u64
}

/// The pairs a measured table holds, two numbers after two.
pub fn pairs<T: Copy>(table: &[T]) -> impl Iterator<Item = (T, T)> + '_ {
    table.as_chunks::<2>().0.iter().map(|[first, second]| (*first, *second))
}

/// A share of a key's rows, so many of so many.
pub type Share = (u64, u64);

/// Values with their shares, drawn by share. They are a table of pairs,
/// each a value and its share in parts of a million. The share the list
/// leaves out of a million belongs to no value of it.
pub struct Listed {
    pairs: &'static [i64],
}

impl Listed {
    /// The list of the table `pairs`.
    pub const fn new(pairs: &'static [i64]) -> Listed {
        Listed { pairs }
    }

    /// How many values it lists.
    pub const fn len(&self) -> usize {
        self.pairs.len().div_euclid(2)
    }

    /// The value `draw` falls on, none if it falls past the list's shares.
    pub fn pick(&self, draw: u64) -> Option<i64> {
        self.shared(draw).map(|(value, _)| value)
    }

    /// The value `draw` falls on, and its share.
    pub fn shared(&self, draw: u64) -> Option<(i64, Share)> {
        self.placed(draw).map(|(_, value, share)| (value, (share.cast_unsigned(), MILLION)))
    }

    /// The value `draw` falls on, with its place in the list before it
    /// and its share after it.
    pub fn placed(&self, draw: u64) -> Option<(usize, i64, i64)> {
        let mut left = below(draw, MILLION).cast_signed();
        pairs(self.pairs).enumerate().map(|(place, (value, share))| (place, value, share)).find(|(_, _, share)| {
            let within = left < *share;
            left -= share;
            within
        })
    }

    /// The parts of a million the list leaves to other values.
    pub fn left(&self) -> u64 {
        let listed: i64 = pairs(self.pairs).map(|(_, share)| share).sum();
        MILLION.saturating_sub(listed.cast_unsigned())
    }
}

/// How often the values of a key occur, as classes of values, the most
/// frequent first, each with how many values it has and the rows they
/// have together. A draw falls in a class by its rows, then on one of its
/// values, each as likely, so a value is drawn as often as the class's
/// values occur on average. The values are numbered from 0, the most
/// frequent class's first.
pub struct Spectrum {
    classes: Vec<Class>,
    rows: u64,
}

/// A class of a spectrum.
#[derive(Clone, Copy)]
pub struct Class {
    /// The number of its first value.
    pub first: u64,
    /// How many values it has.
    pub values: u64,
    /// The rows of it and of the classes before it.
    upto: u64,
}

impl Spectrum {
    /// The spectrum of the table `classes`, pairs of a class's values and
    /// its rows.
    pub fn new(classes: &[u64]) -> Spectrum {
        let (mut first, mut upto) = (0, 0);
        let mut held = Vec::with_capacity(classes.len());
        for (values, rows) in pairs(classes) {
            upto += rows;
            held.push(Class { first, values, upto });
            first += values;
        }
        Spectrum { classes: held, rows: upto }
    }

    /// The class `draw` falls in.
    pub fn class(&self, draw: u64) -> Class {
        let at = below(draw, self.rows);
        let class = self.classes.partition_point(|class| class.upto <= at);
        self.classes[class.min(self.classes.len() - 1)]
    }

    /// The number of the value the draws fall on, `draw` on its class and
    /// `within` on the value of the class.
    pub fn pick(&self, draw: u64, within: u64) -> u64 {
        let class = self.class(draw);
        class.first + below(within, class.values)
    }
}
