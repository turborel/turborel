//! The TPROC-C-style load of one warehouse, as the driver the workload
//! follows builds it: 100,000 items, the warehouse with its 100,000 stock
//! rows and ten districts, 3,000 customers a district each with a history
//! row, and 3,000 orders a district, the last 900 of them new, each of 5 to
//! 15 lines; then the two indexes and the procedures. Each value is drawn
//! as the driver draws it, from the bench's seeded sequence. Rows go in by
//! a prepared INSERT given many rows at once, as a bulk copy sends them.

use turborel::db::{Prepared, Session};
use turborel::params::Load;
use turborel::storage::Row;
use turborel::value::{DataType, Decimal, Value, cast};

use crate::random::{Random, last_name};
use crate::tpcc_schema::{INDEXES, TABLES};

/// Items, and stock rows of a warehouse.
pub const ITEMS: i64 = 100_000;
/// Districts of a warehouse.
pub const DISTRICTS: i64 = 10;
/// Customers, and orders, of a district.
pub const CUSTOMERS: i64 = 3_000;
/// The orders of a district delivered at load; those after are new.
const DELIVERED: i64 = 2_100;
/// The customers given last names in order, the rest by NURand.
const NAMED_IN_ORDER: i64 = 1_000;
/// The constant of the loader's NURand of last names.
pub const C_LAST: i64 = 123;
/// The time every row loaded is stamped with.
const NOW: &str = "2026-11-22 00:00:00";
/// The rows an INSERT is given at once.
const BATCH: usize = 4_096;
/// The seed of the load's draws.
const SEED: u64 = 2_026;

/// A whole number.
pub fn int(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// `units` scaled by `scale` decimal digits.
fn dec(units: i64, scale: u32) -> Value {
    Value::Exact(Decimal::new(i128::from(units), scale))
}

fn text(s: String) -> Value {
    Value::Char(s.into())
}

fn now() -> Value {
    Value::Char(NOW.into())
}

/// The rows of one table on their way in.
pub struct Insert<'a> {
    session: &'a Session,
    one: Prepared,
    types: Vec<DataType>,
    rows: Vec<Row>,
}

impl<'a> Insert<'a> {
    /// The INSERT of `columns` into `table`, by `session`, each table's rows
    /// committed once they are all in.
    pub fn new(session: &'a Session, table: &str, columns: &str) -> Insert<'a> {
        let marks = vec!["?"; columns.split(',').count()].join(", ");
        let one = session.prepare(&format!("INSERT INTO {table} ({columns}) VALUES ({marks})")).unwrap();
        let types = one.inputs().iter().map(|t| t.clone().data_type()).collect();
        Insert { session, one, types, rows: Vec::with_capacity(BATCH) }
    }

    /// Inserts `row`, with those before it once a batch is full.
    pub fn push(&mut self, row: Row) {
        let row = row.into_iter().zip(&self.types).map(|(v, to)| if v.is_null() { v } else { cast(v, to).unwrap() }).collect();
        self.rows.push(row);
        if self.rows.len() == BATCH {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if !self.rows.is_empty() {
            self.session.execute_prepared_rows(&self.one, &self.rows, Load::Alone).unwrap();
            self.rows.clear();
        }
    }
}

impl Drop for Insert<'_> {
    fn drop(&mut self) {
        self.flush();
        self.session.commit().unwrap();
    }
}

/// A street, a second street, a city, a state and a zip code.
fn address(r: &mut Random) -> [Value; 5] {
    let zip = format!("{:0>4}11111", r.number(0, 9_999));
    [text(r.alpha(10, 20)), text(r.alpha(10, 20)), text(r.alpha(10, 20)), text(r.alpha(2, 2)), text(zip)]
}

/// A tax rate, from 0 to 0.2.
fn tax(r: &mut Random) -> Value {
    dec(r.number(0, 2_000), 4)
}

/// Data of 26 to 50 characters, a tenth of them with ORIGINAL within.
fn data(r: &mut Random, original: bool) -> Value {
    let mut s = r.alpha(26, 50);
    if original {
        let at = r.number(0, s.len() as i64 - 8) as usize;
        s.replace_range(at..at + 8, "ORIGINAL");
    }
    text(s)
}

/// Which of `ITEMS` numbers are marked ORIGINAL: a tenth drawn, as the
/// driver draws them, some twice.
fn originals(r: &mut Random) -> Vec<bool> {
    let mut marked = vec![false; ITEMS as usize + 1];
    for _ in 0..ITEMS / 10 {
        marked[r.number(1, ITEMS) as usize] = true;
    }
    marked
}

fn items(s: &Session, r: &mut Random) {
    let original = originals(r);
    let mut insert = Insert::new(s, "ITEM", "I_ID, I_IM_ID, I_NAME, I_PRICE, I_DATA");
    for i in 1..=ITEMS {
        let mut row = vec![int(i), int(r.number(1, 10_000)), text(r.alpha(14, 24)), dec(r.number(100, 10_000), 2)];
        row.push(data(r, original[i as usize]));
        insert.push(row);
    }
}

fn warehouse(s: &Session, r: &mut Random) {
    let mut insert = Insert::new(s, "WAREHOUSE", "W_ID, W_YTD, W_TAX, W_NAME, W_STREET_1, W_STREET_2, W_CITY, W_STATE, W_ZIP");
    let mut row = vec![int(1), dec(30_000_000, 2), tax(r), text(r.alpha(6, 10))];
    row.extend(address(r));
    insert.push(row);
}

fn stock(s: &Session, r: &mut Random) {
    let original = originals(r);
    let dists: Vec<String> = (1..=DISTRICTS).map(|d| format!("S_DIST_{d:02}")).collect();
    let columns = format!("S_W_ID, S_I_ID, S_QUANTITY, {}, S_YTD, S_ORDER_CNT, S_REMOTE_CNT, S_DATA", dists.join(", "));
    let mut insert = Insert::new(s, "STOCK", &columns);
    for i in 1..=ITEMS {
        let mut row = vec![int(1), int(i), int(r.number(10, 100))];
        row.extend((0..DISTRICTS).map(|_| text(r.alpha(24, 24))));
        row.extend([int(0), int(0), int(0), data(r, original[i as usize])]);
        insert.push(row);
    }
}

fn districts(s: &Session, r: &mut Random) {
    let columns = "D_W_ID, D_ID, D_YTD, D_TAX, D_NEXT_O_ID, D_NAME, D_STREET_1, D_STREET_2, D_CITY, D_STATE, D_ZIP";
    let mut insert = Insert::new(s, "DISTRICT", columns);
    for d in 1..=DISTRICTS {
        let (name, address, tax) = (text(r.alpha(6, 10)), address(r), tax(r));
        let mut row = vec![int(1), int(d), dec(3_000_000, 2), tax, int(CUSTOMERS + 1), name];
        row.extend(address);
        insert.push(row);
    }
}

/// The customers of district `d`, each with its history row.
fn customers(s: &Session, r: &mut Random, d: i64) {
    let columns = "C_W_ID, C_D_ID, C_ID, C_DISCOUNT, C_CREDIT, C_LAST, C_FIRST, C_CREDIT_LIM, C_BALANCE, C_YTD_PAYMENT, \
        C_PAYMENT_CNT, C_DELIVERY_CNT, C_STREET_1, C_STREET_2, C_CITY, C_STATE, C_ZIP, C_PHONE, C_SINCE, C_MIDDLE, C_DATA";
    let mut customer = Insert::new(s, "CUSTOMER", columns);
    let mut history = Insert::new(s, "HISTORY", "H_C_ID, H_C_D_ID, H_C_W_ID, H_D_ID, H_W_ID, H_DATE, H_AMOUNT, H_DATA");
    for c in 1..=CUSTOMERS {
        let first = text(r.alpha(8, 16));
        let last = if c <= NAMED_IN_ORDER { last_name(c - 1) } else { last_name(r.nurand(255, 0, 999, C_LAST)) };
        let address = address(r);
        let phone = text(r.digits(16));
        let credit = text(if r.number(1, 10) == 1 { "BC" } else { "GC" }.to_owned());
        let discount = dec(r.number(0, 5_000), 4);
        let mut row = vec![int(1), int(d), int(c), discount, credit, text(last), first, int(50_000), dec(-100, 1), dec(100, 1)];
        row.extend([int(1), int(0)]);
        row.extend(address);
        row.extend([phone, now(), text("OE".to_owned()), text(r.alpha(300, 500))]);
        customer.push(row);
        history.push(vec![int(c), int(d), int(1), int(d), int(1), now(), dec(100, 1), text(r.alpha(12, 24))]);
    }
}

/// The orders of district `d`, their lines, and the new ones.
fn orders(s: &Session, r: &mut Random, d: i64) {
    let mut customer: Vec<i64> = (0..=CUSTOMERS).collect();
    for i in 1..=CUSTOMERS {
        customer.swap(i as usize, r.number(i, CUSTOMERS) as usize);
    }
    let columns = "O_W_ID, O_D_ID, O_ID, O_C_ID, O_CARRIER_ID, O_OL_CNT, O_ALL_LOCAL, O_ENTRY_D";
    let mut orders = Insert::new(s, "ORDERS", columns);
    let mut new = Insert::new(s, "NEW_ORDER", "NO_W_ID, NO_D_ID, NO_O_ID");
    let line_columns = "OL_W_ID, OL_D_ID, OL_O_ID, OL_NUMBER, OL_I_ID, OL_DELIVERY_D, OL_AMOUNT, OL_SUPPLY_W_ID, OL_QUANTITY, OL_DIST_INFO";
    let mut lines = Insert::new(s, "ORDER_LINE", line_columns);
    for o in 1..=CUSTOMERS {
        let count = r.number(5, 15);
        let delivered = o <= DELIVERED;
        let carrier = if delivered { int(r.number(1, 10)) } else { Value::Null };
        orders.push(vec![int(1), int(d), int(o), int(customer[o as usize]), carrier, int(count), int(1), now()]);
        if !delivered {
            new.push(vec![int(1), int(d), int(o)]);
        }
        for l in 1..=count {
            let item = int(r.number(1, ITEMS));
            let info = text(r.alpha(24, 24));
            let (when, amount) = if delivered { (now(), dec(r.number(1, 999_999), 2)) } else { (Value::Null, dec(0, 2)) };
            lines.push(vec![int(1), int(d), int(o), int(l), item, when, amount, int(1), int(5), info]);
        }
    }
}

/// Creates the schema of `s`'s user, loads it, and creates its indexes and
/// `procedures`.
pub fn load(s: &Session, procedures: &[&str]) {
    for table in TABLES {
        s.run(table).unwrap();
        s.commit().unwrap();
    }
    let r = &mut Random::new(SEED);
    items(s, r);
    warehouse(s, r);
    stock(s, r);
    districts(s, r);
    for d in 1..=DISTRICTS {
        customers(s, r, d);
    }
    for d in 1..=DISTRICTS {
        orders(s, r, d);
    }
    for index in INDEXES {
        s.run(index).unwrap();
        s.commit().unwrap();
    }
    for procedure in procedures {
        s.run(procedure).unwrap();
        s.commit().unwrap();
    }
}
