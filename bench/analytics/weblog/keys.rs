//! Who and what a row of the web log is of (`weblog.rs`). The row's place
//! gives the user and the counter of the site viewed (`layout.rs`). The
//! page, its title, the referer, the search phrase, the address and
//! region of the visitor and the time are drawn here from what was
//! measured of the reference log (`measured.rs`). A few pages, referers
//! and phrases have most rows and most have one or two, by the spectrum
//! of each. A page belongs to its counter, which has as large a share of
//! the pages as of the rows. A user's address is the user's own and a
//! region the address's. A user's views follow one another by half a
//! minute, and two in a row share whether they came of a search, and the
//! phrase. A view that came of a search has a referer and the phrase's
//! engine. The texts of a page, a title, a referer and a phrase depend
//! only on which one it is.

use super::layout::{Layout, Place};
use super::measured as m;
use super::random::{MILLION, Random, Share, Spectrum, below, drawn};
use super::texts;

/// The purposes draws are made for, each its own.
const ADDRESS: u64 = 12;
const ADDRESS_VALUE: u64 = 13;
const REGION: u64 = 14;
const PAGE: u64 = 15;
const TITLE: u64 = 16;
const REFERER: u64 = 17;
const PHRASE: u64 = 18;
const SEARCH: u64 = 19;
const ENGINE: u64 = 20;
const START: u64 = 24;
/// Seconds of a day.
const DAY: u64 = 86_400;
/// The views the user a query asks for by number has at least.
const MODEST: u64 = 12;
/// The seconds the visitor's clock and local time are apart from the
/// log's at most.
const CLOCKS_APART: u64 = 600;
/// The bits a user's number is moved by to make room for the number of
/// one of its runs of views.
const RUN_BITS: u32 = 20;
/// The bits a site's number is moved by to make room for the number of
/// one of its pages.
const SITE_BITS: u32 = 40;

/// A page view.
pub struct View {
    /// Whose row it is.
    pub place: Place,
    /// The page's number among its counter's pages.
    pub page: u64,
    /// The referer's site and its number among the site's pages, if the
    /// view has a referer.
    pub referer: Option<(u64, u64)>,
    /// The search phrase's number, if the view came of a search.
    pub phrase: Option<u64>,
    pub engine: i64,
    pub source: i64,
    pub watch: i64,
    pub address: i64,
    pub region: i64,
    /// Seconds from the month's first midnight, of the view, by the
    /// visitor's clock and by the visitor's local time.
    pub seconds: u64,
    pub client_seconds: u64,
    pub local_seconds: u64,
}

/// What the keys are drawn from.
pub struct Keys {
    seed: u64,
    layout: Layout,
    pages: Spectrum,
    referers: Spectrum,
    phrases: Spectrum,
    addresses: Spectrum,
    regions: Spectrum,
}

impl Keys {
    /// The keys of `rows` rows under `seed`.
    pub fn new(rows: u64, seed: u64) -> Keys {
        Keys {
            seed,
            layout: Layout::new(rows, seed),
            pages: Spectrum::new(&m::PAGES),
            referers: Spectrum::new(&m::REFERERS),
            phrases: Spectrum::new(&m::PHRASES),
            addresses: Spectrum::new(&m::ADDRESSES),
            regions: Spectrum::new(&m::REGIONS_REST),
        }
    }

    /// The view of row `row`, by the row's draws.
    pub fn view(&self, row: u64, draws: &mut Random) -> View {
        let place = self.layout.place(row);
        let phrase = self.phrase_of(&place);
        let referer = self.referer_of(phrase.is_some(), draws);
        let other = draws.next();
        let engine = match phrase {
            Some(phrase) => m::ENGINES_SEARCHING.pick(drawn(self.seed, phrase, ENGINE)),
            None => m::ENGINES_OTHER.pick(other),
        };
        let sources = match (phrase, referer) {
            (Some(_), _) => &m::SOURCES_SEARCHING,
            (None, Some(_)) => &m::SOURCES_REFERRED,
            (None, None) => &m::SOURCES_DIRECT,
        };
        let seconds = self.seconds(&place, draws);
        let address = self.addresses.pick(drawn(self.seed, place.user, ADDRESS), drawn(self.seed, place.user, ADDRESS_VALUE));
        View {
            place,
            page: Self::within(&self.pages, place.cell.share, draws),
            referer,
            phrase,
            engine: engine.unwrap_or(0),
            source: sources.pick(draws.next()).unwrap_or(0),
            watch: (draws.next() >> 1).cast_signed(),
            address: i64::from(drawn(self.seed, address, ADDRESS_VALUE) as i32),
            region: self.region(address),
            seconds,
            client_seconds: seconds + below(draws.next(), CLOCKS_APART),
            local_seconds: seconds + below(draws.next(), CLOCKS_APART),
        }
    }

    /// When the view at `place` was, on its day, by the log's clock. The
    /// user's first view is at a time of the day the user's own, and each
    /// later one the measured seconds after, or sooner if the user has too
    /// many for the day.
    fn seconds(&self, place: &Place, draws: &mut Random) -> u64 {
        let apart = m::SECONDS_APART.min(DAY.div_euclid(place.of.max(1))).max(1);
        let start = below(drawn(self.seed, place.user, START), DAY.saturating_sub((place.of - 1) * apart).max(1));
        let within = (start + place.nth * apart + below(draws.next(), apart)).min(DAY - 1);
        (place.cell.day - 1) * DAY - m::BEHIND + within
    }

    /// The search phrase of the view at `place`, none if it came of no
    /// search, the same for the user's views of one run.
    fn phrase_of(&self, place: &Place) -> Option<u64> {
        let run = (place.user << RUN_BITS) | place.nth.div_euclid(m::SEARCH_RUN);
        let mut draws = Random::new(drawn(self.seed, run, SEARCH));
        (below(draws.next(), MILLION) >= u64::from(m::PHRASE.empty)).then(|| self.phrases.pick(draws.next(), draws.next()))
    }

    /// The number of a thing among the things of a part that has the
    /// share `part` of `whole` of the rows of `spectrum`'s key, and so that
    /// share of each class's values, one at least.
    fn within(spectrum: &Spectrum, (part, whole): Share, draws: &mut Random) -> u64 {
        let class = spectrum.class(draws.next());
        let values = (u128::from(class.values) * u128::from(part)).div_euclid(u128::from(whole.max(1))) as u64;
        class.first + below(draws.next(), values.clamp(1, class.values))
    }

    /// The referer of a view, a site by its share and one of the site's
    /// pages. A view that came of a search, `sought`, always has one, and
    /// the measured share of the others has none.
    fn referer_of(&self, sought: bool, draws: &mut Random) -> Option<(u64, u64)> {
        let (direct, site, rest) = (draws.next(), draws.next(), draws.next());
        if !sought && below(direct, MILLION) < u64::from(m::DIRECT) {
            return None;
        }
        let (site, share) = match m::SITES.shared(site) {
            Some((site, share)) => (site.cast_unsigned(), share),
            None => (m::SITES.len() as u64 + below(rest, m::SITES_REST), (m::SITES.left(), MILLION * m::SITES_REST)),
        };
        Some((site, Self::within(&self.referers, share, draws)))
    }

    /// The region of the address numbered `address`, one of the listed by
    /// its share or else one of the others by their spectrum.
    fn region(&self, address: u64) -> i64 {
        let mut draws = Random::new(drawn(self.seed, address, REGION));
        let (first, apart) = m::REGIONS_REST_NUMBERS;
        m::REGIONS.pick(draws.next()).unwrap_or_else(|| first + apart * self.regions.pick(draws.next(), draws.next()).cast_signed())
    }

    /// The identifier of a user with a few views, for the query that asks
    /// for one user's views.
    pub fn modest_user(&self) -> i64 {
        self.layout.user_of(MODEST)
    }

    /// Appends to `out` the address of page `page` of counter `counter`,
    /// made of the scheme, the counter's site and words, with `google` in
    /// the measured share of the less viewed pages.
    pub fn url(&self, out: &mut Vec<u8>, counter: i64, page: u64) {
        let of = drawn(self.seed, counter.cast_unsigned(), PAGE).wrapping_add(page);
        let mut draws = Random::new(drawn(self.seed, of, PAGE));
        let (empty, length) = (draws.next(), m::URL.length(draws.next()));
        let (secure, named, dotted) = (draws.next(), draws.next(), draws.next());
        if below(empty, MILLION) < u64::from(m::URL.empty) {
            return;
        }
        let mut head = Vec::with_capacity(64);
        head.extend_from_slice(if below(secure, MILLION) < u64::from(m::URL_SECURE) { b"https://" } else { b"http://" });
        head.extend_from_slice(b"site");
        texts::numbered(&mut head, counter.cast_unsigned());
        head.extend_from_slice(b".example/");
        if page >= m::URL_NAMED_FROM && below(named, MILLION) < u64::from(m::URL_NAMED) {
            head.extend_from_slice(if below(dotted, MILLION) < u64::from(m::URL_DOTTED) { b"?from=www.google.com/" } else { b"google/" });
        }
        texts::text(out, &head, (length, page), (b'/', m::URL.wide), &mut draws);
    }

    /// Appends to `out` the referer that is page `page` of site `site`,
    /// the measured share of referers with no path.
    pub fn referer(&self, out: &mut Vec<u8>, site: u64, page: u64) {
        let mut draws = Random::new(drawn(self.seed, (site << SITE_BITS) ^ page, REFERER));
        let (length, secure, bare) = (m::REFERER.length(draws.next()), draws.next(), draws.next());
        let mut head = Vec::with_capacity(48);
        head.extend_from_slice(if below(secure, MILLION) < u64::from(m::REFERER_SECURE) { b"https://" } else { b"http://" });
        head.extend_from_slice(b"from");
        texts::numbered(&mut head, site);
        // A bare referer has no path, so nothing of it is after a `/`.
        let between = if below(bare, MILLION) < u64::from(m::REFERER_BARE) { b'.' } else { b'/' };
        head.extend_from_slice(b".example");
        head.push(between);
        texts::text(out, &head, (length, page), (between, m::REFERER.wide), &mut draws);
    }

    /// Appends to `out` the title of page `page` of counter `counter`.
    /// Pages share titles in the measured ratio, the measured share of
    /// titles is empty, and the measured share holds `Google`.
    pub fn title(&self, out: &mut Vec<u8>, counter: i64, page: u64) {
        let title = (u128::from(page) * u128::from(m::TITLES_OF_PAGES.0)).div_euclid(u128::from(m::TITLES_OF_PAGES.1)) as u64;
        let of = drawn(self.seed, counter.cast_unsigned(), TITLE).wrapping_add(title);
        let mut draws = Random::new(drawn(self.seed, of, TITLE));
        if below(draws.next(), MILLION) < u64::from(m::TITLE.empty) {
            return;
        }
        let length = m::TITLE.length(draws.next());
        let head: &[u8] = if below(draws.next(), MILLION) < u64::from(m::TITLE_NAMED) { b"Google " } else { b"" };
        texts::text(out, head, (length, title), (b' ', m::TITLE.wide), &mut draws);
    }

    /// Appends to `out` the search phrase numbered `phrase`.
    pub fn phrase(&self, out: &mut Vec<u8>, phrase: u64) {
        let mut draws = Random::new(drawn(self.seed, phrase, PHRASE));
        let length = m::PHRASE.length(draws.next());
        texts::text(out, b"", (length, phrase), (b' ', m::PHRASE.wide), &mut draws);
    }
}
