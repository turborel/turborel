//! The bench's random draws: SplitMix64 (Steele, Lea and Flood, OOPSLA
//! 2014) from a fixed seed, so a run makes the same data and the same
//! transactions each time, and the draws a TPROC-C-style terminal and loader
//! make from it: a whole number in a range, NURand, and the strings of
//! letters and digits the tables are filled with.

/// SplitMix64's increment, the golden ratio's 64-bit fraction.
const GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;
/// SplitMix64's mixing multipliers.
const MIX_1: u64 = 0xbf58_476d_1ce4_e5b9;
const MIX_2: u64 = 0x94d0_49bb_1331_11eb;
/// The letters and digits strings are made of, as the loader's alphabet.
const ALPHANUMERIC: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
/// The syllables of a customer's last name, by digit.
const SYLLABLES: [&str; 10] = ["BAR", "OUGHT", "ABLE", "PRI", "PRES", "ESE", "ANTI", "CALLY", "ATION", "EING"];

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

    /// A whole number from `low` to `high`, both included, drawn uniformly.
    pub fn number(&mut self, low: i64, high: i64) -> i64 {
        let span = (high - low + 1) as u64;
        low + (self.next() % span) as i64
    }

    /// NURand(A, x, y) with the constant `c`, the skewed draw of a customer's
    /// last name and number.
    pub fn nurand(&mut self, a: i64, x: i64, y: i64, c: i64) -> i64 {
        (((self.number(0, a) | self.number(x, y)) + c) % (y - x + 1)) + x
    }

    /// A string of letters and digits, of a length from `low` to `high`.
    pub fn alpha(&mut self, low: i64, high: i64) -> String {
        let len = self.number(low, high);
        (0..len).map(|_| char::from(ALPHANUMERIC[(self.next() % ALPHANUMERIC.len() as u64) as usize])).collect()
    }

    /// A string of `len` decimal digits.
    pub fn digits(&mut self, len: usize) -> String {
        (0..len).map(|_| char::from(b'0' + (self.next() % 10) as u8)).collect()
    }

    /// A string of `template`, each `#` replaced by a random digit.
    pub fn template(&mut self, template: &str) -> String {
        template.chars().map(|c| if c == '#' { char::from(b'0' + (self.next() % 10) as u8) } else { c }).collect()
    }
}

/// The last name of number `n`, three syllables by its digits.
pub fn last_name(n: i64) -> String {
    let digit = |d: i64| SYLLABLES[(n / d % 10) as usize];
    format!("{}{}{}", digit(100), digit(10), digit(1))
}
