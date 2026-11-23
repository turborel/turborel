//! The TPROC-C-style terminal: the driver's timed loop, with no keying or
//! thinking time. Each transaction is drawn from 1 to 23: New Order up to
//! 10, Payment up to 20, then Delivery, Stock Level and Order Status at one
//! each, so 10/23, 10/23 and 1/23 of each of the others. Each is a CALL of
//! its procedure, prepared once, its arguments drawn as the driver draws
//! them, and sent so that the transaction ends with it: committed once it
//! succeeds, rolled back if it fails. A failure counts as a transaction,
//! by its SQLCODE, as the driver counts it when it does not raise errors.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use turborel::value::Value;

use crate::client::{Client, Failed};
use crate::random::{Random, last_name};
use crate::tpcc_load::{C_LAST, CUSTOMERS, DISTRICTS, ITEMS, int};

/// The transaction types, in the order they are reported.
pub const TYPES: [&str; 5] = ["New Order", "Payment", "Delivery", "Stock Level", "Order Status"];
/// The draw a transaction is chosen by: 1 to `CHOICES`.
const CHOICES: i64 = 23;
/// The highest draws of New Order and Payment.
const NEW_ORDER_UP_TO: i64 = 10;
const PAYMENT_UP_TO: i64 = 20;
/// The warehouses: one, as on the measurement box.
const WAREHOUSES: i64 = 1;
/// The calls prepared, as the driver prepares them.
const CALLS: [&str; 5] =
    ["CALL NEWORD(?,?,?,?,?,?,?,?)", "CALL PAYMENT(?,?,?,?,?,?,?,?)", "CALL DELIVERY(?,?)", "CALL SLEV(?,?,?)", "CALL OSTAT(?,?,?,?,?)"];

/// What a terminal did, by transaction type.
#[derive(Default, Clone)]
pub struct Tally {
    /// Transactions ended, committed or not.
    pub done: [u64; 5],
    /// Their time, in all.
    pub time: [Duration; 5],
    /// Failures, by type and SQLCODE.
    pub failed: BTreeMap<(usize, Failed), u64>,
}

impl Tally {
    /// This tally and `other`'s, together.
    pub fn add(&mut self, other: &Tally) {
        for t in 0..TYPES.len() {
            self.done[t] += other.done[t];
            self.time[t] += other.time[t];
        }
        for (key, n) in &other.failed {
            *self.failed.entry(*key).or_default() += n;
        }
    }

    /// Transactions ended, of every type.
    pub fn total(&self) -> u64 {
        self.done.iter().sum()
    }
}

/// A virtual user: its session, its statements, its draws, and its
/// district for Stock Level, fixed for the terminal as the driver fixes it.
pub struct Terminal<C: Client> {
    client: C,
    calls: Vec<C::Statement>,
    random: Random,
    w_id: i64,
    stock_level_d_id: i64,
}

fn array(values: Vec<i64>) -> Value {
    Value::Array(values.into_iter().map(int).collect::<Vec<_>>().into())
}

fn name(last: String) -> Value {
    Value::Char(last.into())
}

impl<C: Client> Terminal<C> {
    /// The terminal of `client`, its draws seeded by `seed`.
    pub fn new(mut client: C, seed: u64) -> Terminal<C> {
        let calls = CALLS.iter().map(|sql| client.prepare(sql).unwrap()).collect();
        let mut random = Random::new(seed);
        let w_id = random.number(1, WAREHOUSES);
        let stock_level_d_id = random.number(1, DISTRICTS);
        Terminal { client, calls, random, w_id, stock_level_d_id }
    }

    /// Runs transactions until `until`, tallied.
    pub fn run(&mut self, until: Instant) -> Tally {
        let mut tally = Tally::default();
        while Instant::now() < until {
            let choice = self.random.number(1, CHOICES);
            let t = if choice <= NEW_ORDER_UP_TO {
                0
            } else if choice <= PAYMENT_UP_TO {
                1
            } else {
                (choice - PAYMENT_UP_TO + 1) as usize
            };
            let values = self.arguments(t);
            let started = Instant::now();
            let outcome = self.client.execute(&self.calls[t], &values, true);
            tally.time[t] += started.elapsed();
            tally.done[t] += 1;
            if let Err(failed) = outcome {
                *tally.failed.entry((t, failed)).or_default() += 1;
            }
        }
        tally
    }

    /// The arguments of a transaction of type `t`, drawn.
    fn arguments(&mut self, t: usize) -> Vec<Value> {
        match t {
            0 => self.new_order(),
            1 => self.payment(),
            2 => vec![int(self.w_id), int(self.random.number(1, 10))],
            3 => vec![int(self.w_id), int(self.stock_level_d_id), int(self.random.number(10, 20))],
            _ => self.order_status(),
        }
    }

    /// New Order's: 5 to 15 items, one order in a hundred naming an unused
    /// item last, one line in a hundred from a warehouse drawn at random.
    fn new_order(&mut self) -> Vec<Value> {
        let r = &mut self.random;
        let (d_id, c_id, count, rollback) = (r.number(1, 10), r.number(1, CUSTOMERS), r.number(5, 15), r.number(1, 100));
        let (mut items, mut supply, mut quantity, mut all_local) = (Vec::new(), Vec::new(), Vec::new(), 1);
        for line in 1..=count {
            items.push(if line == count && rollback == 1 { ITEMS + 1 } else { r.number(1, ITEMS) });
            if r.number(1, 100) > 1 {
                supply.push(self.w_id);
            } else {
                all_local = 0;
                supply.push(1 + (self.w_id + r.number(0, WAREHOUSES - 1)) % WAREHOUSES);
            }
            quantity.push(r.number(1, 10));
        }
        vec![int(self.w_id), int(d_id), int(c_id), int(count), int(all_local), array(items), array(supply), array(quantity)]
    }

    /// Payment's: the customer of the home district 85 times in a hundred,
    /// by last name 60 times in a hundred.
    fn payment(&mut self) -> Vec<Value> {
        let r = &mut self.random;
        let d_id = r.number(1, 10);
        let (x, y) = (r.number(1, 100), r.number(1, 100));
        let (customer_d, customer_w) = if x <= 85 { (d_id, self.w_id) } else { (r.number(1, 10), r.number(1, WAREHOUSES)) };
        let last = last_name(r.nurand(255, 0, 999, C_LAST));
        let c_id = r.number(1, CUSTOMERS);
        let by_name = i64::from(y <= 60);
        let amount = r.number(1, 5_000);
        vec![int(self.w_id), int(d_id), int(customer_w), int(customer_d), int(c_id), int(by_name), int(amount), name(last)]
    }

    /// Order Status's: by last name 60 times in a hundred.
    fn order_status(&mut self) -> Vec<Value> {
        let r = &mut self.random;
        let d_id = r.number(1, 10);
        let last = last_name(r.nurand(255, 0, 999, C_LAST));
        let c_id = r.number(1, CUSTOMERS);
        let by_name = i64::from(r.number(1, 100) <= 60);
        vec![int(self.w_id), int(d_id), int(c_id), int(by_name), name(last)]
    }
}

/// The orders the districts have taken: the sum of their next order
/// numbers, whose growth the driver counts as New Orders committed.
pub fn orders_taken(client: &mut impl Client) -> i64 {
    let rows = client.direct("SELECT SUM(D_NEXT_O_ID) FROM DISTRICT", true).unwrap();
    rows[0][0].as_f64().unwrap() as i64
}
