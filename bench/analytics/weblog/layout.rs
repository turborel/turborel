//! Where each row of the web log falls (`weblog.rs`). The rows come in the
//! order the reference log's do, by the counter, then the day, then the
//! user, then the time, which is the order HITS keeps them in. So the
//! layout is made first, from the seed. Each counter takes its measured
//! share of the rows. The rows of a counter are dealt to the days it has
//! views on, and the rows of a counter's day to users, each given as many
//! views as a user of a measured class has, and an identifier that rises
//! with the user's place. A user so belongs to one counter and one day,
//! as nearly every user of the reference log does. A row's number then
//! tells its user, by a search of where each user's rows begin.

use super::measured as m;
use super::random::{MILLION, Random, Share, below, drawn, pairs};

/// The purposes draws are made for, each its own.
const DAYS: u64 = 21;
const USERS: u64 = 22;
const USER_ID: u64 = 23;
/// The bits a counter's number is moved by to make room for a day.
const DAY_BITS: u32 = 8;

/// The rows of one counter on one day.
#[derive(Clone, Copy)]
pub struct Cell {
    pub counter: i64,
    /// The counter's share of all rows.
    pub share: Share,
    /// The day of the month.
    pub day: u64,
}

/// Where a user's rows begin, with its identifier and its cell.
#[derive(Clone, Copy)]
struct User {
    first: u64,
    id: i64,
    cell: u32,
}

/// The layout of a table's rows.
pub struct Layout {
    cells: Vec<Cell>,
    users: Vec<User>,
    rows: u64,
}

/// Whose row a row is.
#[derive(Clone, Copy)]
pub struct Place {
    /// The user's number among all users, in the rows' order.
    pub user: u64,
    pub user_id: i64,
    pub cell: Cell,
    /// Which of the user's rows it is, from 0, and how many the user has.
    pub nth: u64,
    pub of: u64,
}

impl Layout {
    /// The layout of `rows` rows under `seed`.
    pub fn new(rows: u64, seed: u64) -> Layout {
        let mut layout = Layout { cells: Vec::new(), users: Vec::new(), rows };
        let mut first = 0;
        for (counter, counter_rows) in counters(rows) {
            let share = (counter_rows, rows);
            for (day, cell_rows) in days(counter, counter_rows, seed) {
                layout.cells.push(Cell { counter, share, day });
                layout.fill(first, cell_rows, seed);
                first += cell_rows;
            }
        }
        layout
    }

    /// Deals the `rows` rows from row `first`, the last cell's, to users.
    fn fill(&mut self, first: u64, rows: u64, seed: u64) {
        let cell = self.cells.len() - 1;
        let mut draws = Random::new(drawn(seed, cell as u64, USERS));
        let from = self.users.len();
        let mut dealt = 0;
        while dealt < rows {
            let views = views(&mut draws).min(rows - dealt);
            self.users.push(User { first: first + dealt, id: 0, cell: cell as u32 });
            dealt += views;
        }
        let mut ids: Vec<i64> = (from..self.users.len()).map(|user| drawn(seed, user as u64, USER_ID).cast_signed()).collect();
        ids.sort_unstable();
        for (user, id) in self.users[from..].iter_mut().zip(ids) {
            user.id = id;
        }
    }

    /// Whose row `row` is.
    pub fn place(&self, row: u64) -> Place {
        let user = self.users.partition_point(|user| user.first <= row) - 1;
        let held = self.users[user];
        let next = self.users.get(user + 1).map_or(self.rows, |next| next.first);
        Place { user: user as u64, user_id: held.id, cell: self.cells[held.cell as usize], nth: row - held.first, of: next - held.first }
    }

    /// The identifier of the first user with `views` views at least, the
    /// first user's if none has.
    pub fn user_of(&self, views: u64) -> i64 {
        let firsts = self.users.iter().map(|user| user.first).skip(1).chain([self.rows]);
        let found = self.users.iter().zip(firsts).find(|(user, next)| next - user.first >= views);
        found.or(self.users.first().map(|user| (user, self.rows))).map_or(0, |(user, _)| user.id)
    }
}

/// The views of the next user. Its class is drawn by how many users each
/// has, and it has the class's least views or, in the share of the
/// others, any up to twice that.
fn views(draws: &mut Random) -> u64 {
    let classes = || m::USERS.as_chunks::<3>().0.iter().copied();
    let all: u64 = classes().map(|[_, users, _]| users).sum();
    let mut at = below(draws.next(), all);
    let found = classes().find(|[_, users, _]| {
        let within = at < *users;
        at = at.saturating_sub(*users);
        within
    });
    let [least, _, share] = found.unwrap_or([1, 0, 0]);
    let (low, spread) = (draws.next(), draws.next());
    if below(low, MILLION) < share { least } else { least + below(spread, least) }
}

/// The counters with rows among `rows`, in the order of their numbers,
/// each with its rows. The listed counters and the others are each given
/// their measured share, and the counter of most views takes what rounding
/// leaves.
fn counters(rows: u64) -> Vec<(i64, u64)> {
    let scaled = |views: u64| (u128::from(views) * u128::from(rows)).div_euclid(u128::from(m::REFERENCE_ROWS)) as u64;
    let mut held: Vec<(i64, u64)> = pairs(&m::COUNTERS).map(|(counter, views)| (counter, scaled(views.cast_unsigned()))).collect();
    let (first, apart) = m::COUNTERS_REST_NUMBERS;
    let mut number = first;
    for (counters, views) in pairs(&m::COUNTERS_REST) {
        for _ in 0..counters {
            held.push((number, scaled(views.div_euclid(counters))));
            number += apart;
        }
    }
    let dealt: u64 = held.iter().map(|(_, given)| given).sum();
    held[0].1 += rows.saturating_sub(dealt);
    held.retain(|(_, given)| *given > 0);
    held.sort_unstable();
    held
}

/// The days counter `counter` has views on, in order, each with its rows
/// of the counter's `rows`. The counter of the last queries has its one
/// measured day. Another has each day with the measured chance that a
/// counter has views on a day, and its days share its rows by their own
/// shares, the last taking what rounding leaves.
fn days(counter: i64, rows: u64, seed: u64) -> Vec<(u64, u64)> {
    if counter == m::BUSY.0 {
        return vec![(m::BUSY.1, rows)];
    }
    let (with_views, counters) = m::COUNTER_DAYS;
    let chance = (with_views * MILLION).div_euclid(counters * m::DAYS.len().div_euclid(2) as u64);
    let has = |day: u64| below(drawn(seed, (counter.cast_unsigned() << DAY_BITS) | day, DAYS), MILLION) < chance;
    let mut held: Vec<(u64, u64)> = pairs(&m::DAYS).filter(|(day, _)| has(*day)).collect();
    if held.is_empty() {
        held.extend(pairs(&m::DAYS).max_by_key(|(_, share)| *share));
    }
    let shares: u64 = held.iter().map(|(_, share)| share).sum();
    let mut dealt: Vec<(u64, u64)> = held.iter().map(|(day, share)| (*day, (rows * share).div_euclid(shares))).collect();
    let given: u64 = dealt.iter().map(|(_, of_day)| of_day).sum();
    if let Some(last) = dealt.last_mut() {
        last.1 += rows - given;
    }
    dealt.retain(|(_, of_day)| *of_day > 0);
    dealt
}
