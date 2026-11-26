//! The web log's texts (`weblog.rs`): an address of a page, a title, a
//! search phrase and the other texts of a row, each made of words from a
//! short vocabulary until it has the octets its column's measured lengths
//! ask, with the share of two-octet characters measured for the column,
//! and ending in the number of what it is the text of, so two things have
//! two texts. A text depends only on the draws it is given, so one page
//! has one address wherever it occurs.

use super::random::{MILLION, Random, below, pairs};

/// How the texts of a column are shaped, as measured.
pub struct Shape {
    /// The texts that are empty, in parts of a million.
    pub empty: u32,
    /// Of the others, classes of lengths, a table of pairs: the least
    /// octets of each, and its share in parts of a million; a class ends
    /// where the next begins, the last at `longest`.
    pub lengths: &'static [u32],
    /// The octets the last class ends before.
    pub longest: u32,
    /// The words that are of two-octet characters, in parts of a million.
    pub wide: u32,
}

impl Shape {
    /// The octets of a text that is not empty, as `draw` falls.
    pub fn length(&self, draw: u64) -> usize {
        let mut left = below(draw, MILLION);
        let ends = pairs(self.lengths).skip(1).map(|(low, _)| low).chain([self.longest]);
        let (low, high) = pairs(self.lengths)
            .zip(ends)
            .find(|((_, share), _)| {
                let within = left < u64::from(*share);
                left = left.saturating_sub(u64::from(*share));
                within
            })
            .map_or((self.longest, self.longest + 1), |((low, _), high)| (low, high));
        (u64::from(low) + below(draw.rotate_left(32), u64::from(high.max(low + 1) - low))) as usize
    }
}

/// Words of one-octet characters, as the addresses of pages are made of.
const NARROW: [&str; 48] = [
    "news", "view", "page", "index", "catalog", "item", "search", "forum", "topic", "user", "photo", "video", "article", "product", "list",
    "category", "shop", "auto", "sport", "music", "games", "weather", "map", "city", "price", "sale", "order", "cart", "profile", "login",
    "blog", "post", "comment", "tag", "archive", "media", "file", "download", "info", "about", "contact", "help", "service", "online",
    "free", "best", "new", "top",
];
/// Words of two-octet characters, as titles and phrases are made of.
const WIDE: [&str; 32] = [
    "новости",
    "погода",
    "купить",
    "цена",
    "фото",
    "видео",
    "скачать",
    "бесплатно",
    "онлайн",
    "смотреть",
    "игры",
    "работа",
    "авто",
    "продажа",
    "магазин",
    "отзывы",
    "карта",
    "город",
    "форум",
    "музыка",
    "фильмы",
    "сайт",
    "официальный",
    "каталог",
    "недвижимость",
    "квартиры",
    "отдых",
    "туры",
    "рецепты",
    "одежда",
    "телефон",
    "интернет",
];
/// The bits of a draw passed over before those that choose a word.
const WORD_BITS: u32 = 40;
/// The base a thing's number is written in.
const BASE: u64 = 36;
/// The digits a thing's number is written in.
const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
/// The octet that fills a text out to its length where no word fits.
const FILL: u8 = b'x';

/// Appends `number` to `out` in digits of base 36.
pub fn numbered(out: &mut Vec<u8>, mut number: u64) {
    let from = out.len();
    loop {
        out.push(DIGITS[(number % BASE) as usize]);
        number = number.div_euclid(BASE);
        if number == 0 {
            break;
        }
    }
    out[from..].reverse();
}

/// Appends words to `out` until the text that began at `from` has `length`
/// octets, less the room `kept` for what follows them: each word apart by
/// `between`, one in a million `wide` of them of two-octet characters.
pub fn worded(out: &mut Vec<u8>, (from, length): (usize, usize), (between, wide): (u8, u32), draws: &mut Random) {
    while out.len() - from < length {
        let draw = draws.next();
        let word = if below(draw, MILLION) < u64::from(wide) {
            WIDE[(draw >> WORD_BITS) as usize % WIDE.len()]
        } else {
            NARROW[(draw >> WORD_BITS) as usize % NARROW.len()]
        };
        if out.len() - from + word.len() + 1 > length {
            break;
        }
        out.extend_from_slice(word.as_bytes());
        out.push(between);
    }
    out.resize(from + length, FILL);
}

/// Appends to `out` a text of `length` octets that ends in `number`: a
/// start `head`, then words as `worded` appends them.
pub fn text(out: &mut Vec<u8>, head: &[u8], (length, number): (usize, u64), shape: (u8, u32), draws: &mut Random) {
    let from = out.len();
    let mut tail = Vec::with_capacity(16);
    numbered(&mut tail, number);
    out.extend_from_slice(&head[..head.len().min(length)]);
    let room = length.saturating_sub(tail.len()).max(out.len() - from);
    worded(out, (from, room), shape, draws);
    let left = (from + length).saturating_sub(out.len());
    out.extend_from_slice(&tail[tail.len() - left.min(tail.len())..]);
    out.resize(from + length, FILL);
}
