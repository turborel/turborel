//! Throwaway experiment, branch `storagebound`, never merged: the `native`
//! procedure set, the five TPROC-C-style transactions written in Rust
//! straight against the storage and transaction API (`Session::native`),
//! with the reads and writes of the `old` set's SQL, row for row and
//! statement by statement: each statement of a procedure renews the
//! snapshot, takes its table locks, reads through the index the plan
//! reads, and writes through the engine's row write, with its row lock,
//! recheck, undo, log record and index upkeep. The commit is the
//! session's.

use std::collections::HashSet;

use turborel::catalog::QualifiedName;
use turborel::db::Session;
use turborel::db::native::NativeTx;
use turborel::error::{Code, Result, SqlError};
use turborel::index::Probe;
use turborel::storage::Row;
use turborel::value::{CompOp, DataType, Decimal, Value, cast};

use crate::tpcc_run::USER;

fn table(ident: &str) -> QualifiedName {
    QualifiedName { auth: USER.to_owned(), ident: ident.to_owned() }
}

fn int(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

fn dec(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

fn no_row(what: &str) -> SqlError {
    SqlError::new(Code::Api, format!("native: no row {what}"))
}

/// A whole number's value.
fn whole(v: &Value) -> Option<i64> {
    match v {
        Value::Exact(d) => Some((d.units / 10_i128.pow(d.scale)) as i64),
        _ => None,
    }
}

/// An exact number's units at `scale`.
fn units(v: &Value, scale: u32) -> Option<i128> {
    match v {
        Value::Exact(d) if d.scale <= scale => Some(d.units * 10_i128.pow(scale - d.scale)),
        Value::Exact(d) => Some(d.units / 10_i128.pow(d.scale - scale)),
        _ => None,
    }
}

fn decimal(v: &Value) -> Option<Decimal> {
    if let Value::Exact(d) = v { Some(*d) } else { None }
}

fn text(v: &Value) -> &str {
    if let Value::Char(s) = v { s } else { "" }
}

/// An array argument's whole numbers.
fn wholes(v: &Value) -> Vec<i64> {
    match v {
        Value::Array(a) => a.iter().map(|e| whole(e).unwrap()).collect(),
        _ => Vec::new(),
    }
}

/// A DECIMAL(6,2) cast to VARCHAR.
fn shown(units: i128) -> String {
    let sign = if units < 0 { "-" } else { "" };
    let u = units.abs();
    format!("{sign}{}.{:02}", u / 100, u % 100)
}

/// The tables, their primary keys' and secondary indexes' names, and the
/// types a value is cast to, found once.
struct Names {
    warehouse: (QualifiedName, String),
    district: (QualifiedName, String),
    customer: (QualifiedName, String),
    customer_by_name: String,
    history: QualifiedName,
    new_order: (QualifiedName, String),
    orders: (QualifiedName, String),
    orders_by_customer: String,
    order_line: (QualifiedName, String),
    item: (QualifiedName, String),
    stock: (QualifiedName, String),
    ol_amount: DataType,
}

/// A terminal's native procedures.
pub struct Native {
    names: Option<Names>,
    /// The time each row written is stamped with: an order's, as stored.
    now: Value,
}

impl Native {
    pub fn new() -> Native {
        Native { names: None, now: Value::Null }
    }

    fn names(&mut self, n: &NativeTx<'_>) -> Result<()> {
        if self.names.is_some() {
            return Ok(());
        }
        let keyed = |ident: &str, columns: &[usize]| -> Result<(QualifiedName, String)> {
            let t = table(ident);
            let ix = n.index_on(&t, columns)?;
            Ok((t, ix))
        };
        let names = Names {
            warehouse: keyed("WAREHOUSE", &[0])?,
            district: keyed("DISTRICT", &[0, 1])?,
            customer: keyed("CUSTOMER", &[0, 1, 2])?,
            customer_by_name: n.index_on(&table("CUSTOMER"), &[0, 1, 5, 6, 2])?,
            history: table("HISTORY"),
            new_order: keyed("NEW_ORDER", &[0, 1, 2])?,
            orders: keyed("ORDERS", &[0, 1, 2])?,
            orders_by_customer: n.index_on(&table("ORDERS"), &[0, 1, 3, 2])?,
            order_line: keyed("ORDER_LINE", &[0, 1, 2, 3])?,
            item: keyed("ITEM", &[0])?,
            stock: keyed("STOCK", &[0, 1])?,
            ol_amount: n.column_type(&table("ORDER_LINE"), 6)?,
        };
        n.read_lock(&names.orders.0)?;
        let (_, order) = n.get((&names.orders.0, &names.orders.1), vec![int(1), int(1), int(1)])?.ok_or_else(|| no_row("order 1"))?;
        self.now = order[7].clone();
        self.names = Some(names);
        Ok(())
    }

    /// Runs transaction `t` with `values`, as its CALL would, then commits
    /// it, or rolls it back if it fails.
    pub fn run(&mut self, session: &Session, t: usize, values: &[Value]) -> Result<()> {
        let ran = session.native(|n| {
            self.names(n)?;
            let names = self.names.as_ref().unwrap();
            let now = &self.now;
            match t {
                0 => new_order(n, names, now, values),
                1 => payment(n, names, now, values),
                2 => delivery(n, names, now, values),
                3 => stock_level(n, names, values),
                _ => order_status(n, names, values),
            }
        });
        match ran {
            Ok(()) => session.commit(),
            Err(e) => {
                let _ = session.rollback();
                Err(e)
            }
        }
    }
}

/// Updates the row of `key` in `(table, index)` to what `change` makes of
/// it, as a searched UPDATE of the whole key does: under the update lock,
/// rechecked over a version committed since the snapshot. The row as
/// written, none if there was none.
fn update_one(
    n: &NativeTx<'_>,
    (table, index): (&QualifiedName, &str),
    key: Vec<Value>,
    change: &mut dyn FnMut(&Row) -> Result<Row>,
) -> Result<Option<Row>> {
    n.statement()?;
    n.update_lock(table)?;
    let Some((id, old)) = n.get((table, index), key)? else { return Ok(None) };
    update_found(n, (table, id), &old, change)
}

/// Updates row `id`, read as `old`, as `update_one` does.
fn update_found(
    n: &NativeTx<'_>,
    (table, id): (&QualifiedName, u64),
    old: &Row,
    change: &mut dyn FnMut(&Row) -> Result<Row>,
) -> Result<Option<Row>> {
    let new = change(old)?;
    let mut recomputed: Option<Row> = None;
    let written = n.update((table, id), &new, &mut |newest| {
        let row = change(newest)?;
        recomputed = Some(row.clone());
        Ok(Some(row))
    })?;
    Ok(written.map(|_| recomputed.unwrap_or(new)))
}

fn arg(values: &[Value], i: usize) -> i64 {
    whole(&values[i]).unwrap()
}

/// NEWORD of the `old` set.
fn new_order(n: &NativeTx<'_>, names: &Names, now: &Value, values: &[Value]) -> Result<()> {
    let (w, d, c, count, all_local) = (arg(values, 0), arg(values, 1), arg(values, 2), arg(values, 3), arg(values, 4));
    let (items, supply, quantity) = (wholes(&values[5]), wholes(&values[6]), wholes(&values[7]));
    // SELECT C_DISCOUNT, C_LAST, C_CREDIT, W_TAX FROM CUSTOMER, WAREHOUSE
    n.statement()?;
    n.read_lock(&names.customer.0)?;
    n.read_lock(&names.warehouse.0)?;
    let (_, customer) = n.get((&names.customer.0, &names.customer.1), vec![int(w), int(d), int(c)])?.ok_or_else(|| no_row("customer"))?;
    let (_, warehouse) = n.get((&names.warehouse.0, &names.warehouse.1), vec![int(w)])?.ok_or_else(|| no_row("warehouse"))?;
    let (c_discount, w_tax) = (decimal(&customer[3]).unwrap(), decimal(&warehouse[2]).unwrap());
    // UPDATE DISTRICT SET D_NEXT_O_ID = D_NEXT_O_ID + 1, its FINAL TABLE read
    let district = update_one(n, (&names.district.0, &names.district.1), vec![int(w), int(d)], &mut |row| {
        let mut row = row.clone();
        row[4] = int(whole(&row[4]).unwrap() + 1);
        Ok(row)
    })?
    .ok_or_else(|| no_row("district"))?;
    let o_id = whole(&district[4]).unwrap() - 1;
    let d_tax = decimal(&district[3]).unwrap();
    // INSERT INTO ORDERS, INSERT INTO NEW_ORDER
    n.statement()?;
    n.insert(&names.orders.0, &vec![int(w), int(d), int(o_id), int(c), Value::Null, int(count), int(all_local), now.clone()])?;
    n.statement()?;
    n.insert(&names.new_order.0, &vec![int(w), int(d), int(o_id)])?;
    // Each stock row, by a statement of its own.
    let mut dists = Vec::with_capacity(items.len());
    for line in 0..count as usize {
        let q = quantity[line];
        let stock = update_one(n, (&names.stock.0, &names.stock.1), vec![int(supply[line]), int(items[line])], &mut |row| {
            let mut row = row.clone();
            let s = whole(&row[2]).unwrap();
            row[2] = int(if s < q + 10 { s + 91 } else { s } - q);
            Ok(row)
        })?;
        let column = 3 + (if (1..=10).contains(&d) { d } else { 10 } - 1) as usize;
        dists.push(stock.map_or(Value::Null, |row| row[column].clone()));
    }
    // INSERT INTO ORDER_LINE SELECT ... FROM UNNEST(...), each line's price
    // by its subquery.
    n.statement()?;
    n.read_lock(&names.item.0)?;
    let one = Decimal::new(1, 0);
    let taxed = one.checked_add(w_tax)?.checked_add(d_tax)?;
    let discounted = one.checked_sub(c_discount)?;
    for line in 0..count as usize {
        let price = n.get((&names.item.0, &names.item.1), vec![int(items[line])])?.and_then(|(_, item)| decimal(&item[3]));
        let amount = match price {
            Some(price) => {
                let a = Decimal::new(i128::from(quantity[line]), 0).checked_mul(price)?.checked_mul(taxed)?.checked_mul(discounted)?;
                cast(Value::Exact(a), &names.ol_amount)?
            }
            None => Value::Null,
        };
        let row = vec![
            int(w),
            int(d),
            int(o_id),
            int(line as i64 + 1),
            int(items[line]),
            Value::Null,
            amount,
            int(supply[line]),
            int(quantity[line]),
            dists[line].clone(),
        ];
        n.insert(&names.order_line.0, &row)?;
    }
    Ok(())
}

/// The customers of district `(w, d)` named `last`, in order of their
/// first names, by the index of their names: how many there are, and
/// the `take`th, counted from one, if `take` is given.
fn by_name(n: &NativeTx<'_>, names: &Names, (w, d, last): (i64, i64, &str), take: Option<usize>) -> Result<(usize, Option<Row>)> {
    let probe = Probe::Prefix { equal: vec![int(w), int(d), Value::Char(last.to_owned())], then: None };
    let (mut seen, mut found) = (0, None);
    n.scan((&names.customer.0, &names.customer_by_name), &probe, false, &mut |_, row| {
        seen += 1;
        if take == Some(seen) {
            found = Some(row);
            return Ok(false);
        }
        Ok(true)
    })?;
    Ok((seen, found))
}

/// PAYMENT, of both sets.
fn payment(n: &NativeTx<'_>, names: &Names, now: &Value, values: &[Value]) -> Result<()> {
    let (w, d, cw, cd, c, byname) = (arg(values, 0), arg(values, 1), arg(values, 2), arg(values, 3), arg(values, 4), arg(values, 5));
    let amount = i128::from(arg(values, 6)) * 100;
    let last = text(&values[7]).to_owned();
    let add = |v: &Value, by: i128| units(v, 2).map_or(Value::Null, |u| dec(u + by, 2));
    let warehouse = update_one(n, (&names.warehouse.0, &names.warehouse.1), vec![int(w)], &mut |row| {
        let mut row = row.clone();
        row[1] = add(&row[1], amount);
        Ok(row)
    })?;
    let w_name = warehouse.as_ref().map_or(String::new(), |r| text(&r[3]).to_owned());
    let district = update_one(n, (&names.district.0, &names.district.1), vec![int(w), int(d)], &mut |row| {
        let mut row = row.clone();
        row[2] = add(&row[2], amount);
        Ok(row)
    })?;
    let d_name = district.as_ref().map_or(String::new(), |r| text(&r[5]).to_owned());
    let (mut c_id, mut credit, mut balance) = (c, "0".to_owned(), Some(0_i128));
    let customer = if byname == 1 {
        // SELECT COUNT(C_LAST), then the cursor's FETCHes, NAMECNT / 2 of them.
        n.statement()?;
        n.read_lock(&names.customer.0)?;
        let (count, _) = by_name(n, names, (cw, cd, &last), None)?;
        let take = count / 2;
        n.statement()?;
        if take == 0 { None } else { by_name(n, names, (cw, cd, &last), Some(take))?.1 }
    } else {
        n.statement()?;
        n.read_lock(&names.customer.0)?;
        n.get((&names.customer.0, &names.customer.1), vec![int(cw), int(cd), int(c)])?.map(|(_, row)| row)
    };
    if let Some(row) = &customer {
        c_id = whole(&row[2]).unwrap();
        credit = text(&row[4]).to_owned();
        balance = units(&row[8], 2);
    }
    let new_balance = balance.map(|b| b - amount);
    let key = vec![int(cw), int(cd), int(c_id)];
    if credit == "BC" {
        let prefix = format!("{c_id} {cd} {cw} {d} {w} {} | ", shown(amount));
        update_one(n, (&names.customer.0, &names.customer.1), key, &mut |row| {
            let mut row = row.clone();
            row[8] = new_balance.map_or(Value::Null, |b| dec(b, 2));
            let data: String = format!("{prefix}{}", text(&row[20])).chars().take(500).collect();
            row[20] = Value::Char(data);
            Ok(row)
        })?;
    } else {
        update_one(n, (&names.customer.0, &names.customer.1), key, &mut |row| {
            let mut row = row.clone();
            row[8] = new_balance.map_or(Value::Null, |b| dec(b, 2));
            Ok(row)
        })?;
    }
    n.statement()?;
    let h_data = Value::Char(format!("{w_name} {d_name}"));
    n.insert(&names.history, &vec![int(c_id), int(cd), int(cw), int(d), int(w), now.clone(), dec(amount, 2), h_data])?;
    Ok(())
}

/// DELIVERY of the `old` set.
fn delivery(n: &NativeTx<'_>, names: &Names, now: &Value, values: &[Value]) -> Result<()> {
    let (w, carrier) = (arg(values, 0), arg(values, 1));
    for d in 1..=10 {
        // SELECT MIN(NO_O_ID) FROM NEW_ORDER
        n.statement()?;
        n.read_lock(&names.new_order.0)?;
        let mut oldest = None;
        let probe = Probe::Prefix { equal: vec![int(w), int(d)], then: None };
        n.scan((&names.new_order.0, &names.new_order.1), &probe, false, &mut |_, row| {
            oldest = whole(&row[2]);
            Ok(false)
        })?;
        let Some(o_id) = oldest else { continue };
        // DELETE FROM NEW_ORDER, its OLD TABLE read
        n.statement()?;
        n.update_lock(&names.new_order.0)?;
        let Some((id, _)) = n.get((&names.new_order.0, &names.new_order.1), vec![int(w), int(d), int(o_id)])? else { continue };
        if n.delete((&names.new_order.0, id))?.is_none() {
            continue;
        }
        // UPDATE ORDERS SET O_CARRIER_ID, its FINAL TABLE read
        let order = update_one(n, (&names.orders.0, &names.orders.1), vec![int(w), int(d), int(o_id)], &mut |row| {
            let mut row = row.clone();
            row[4] = int(carrier);
            Ok(row)
        })?;
        let c_id = order.as_ref().and_then(|r| whole(&r[3]));
        // UPDATE ORDER_LINE SET OL_DELIVERY_D, SUM(OL_AMOUNT) of its FINAL TABLE
        n.statement()?;
        n.update_lock(&names.order_line.0)?;
        let mut lines = Vec::new();
        let probe = Probe::Prefix { equal: vec![int(w), int(d), int(o_id)], then: None };
        n.scan((&names.order_line.0, &names.order_line.1), &probe, false, &mut |id, row| {
            lines.push((id, row));
            Ok(true)
        })?;
        let mut total: Option<i128> = None;
        for (id, old) in &lines {
            let written = update_found(n, (&names.order_line.0, *id), old, &mut |row| {
                let mut row = row.clone();
                row[5] = now.clone();
                Ok(row)
            })?;
            if let Some(amount) = written.as_ref().and_then(|r| units(&r[6], 2)) {
                total = Some(total.unwrap_or(0) + amount);
            }
        }
        // UPDATE CUSTOMER SET C_BALANCE = COALESCE(C_BALANCE, 0) + D_OL_TOTAL
        let Some(c_id) = c_id else { continue };
        update_one(n, (&names.customer.0, &names.customer.1), vec![int(w), int(d), int(c_id)], &mut |row| {
            let mut row = row.clone();
            let b = units(&row[8], 2).unwrap_or(0);
            row[8] = total.map_or(Value::Null, |t| dec(b + t, 2));
            Ok(row)
        })?;
    }
    Ok(())
}

/// SLEV of the `old` set.
fn stock_level(n: &NativeTx<'_>, names: &Names, values: &[Value]) -> Result<()> {
    let (w, d, threshold) = (arg(values, 0), arg(values, 1), arg(values, 2));
    // SELECT D_NEXT_O_ID FROM DISTRICT
    n.statement()?;
    n.read_lock(&names.district.0)?;
    let (_, district) = n.get((&names.district.0, &names.district.1), vec![int(w), int(d)])?.ok_or_else(|| no_row("district"))?;
    let next = whole(&district[4]).unwrap();
    // SELECT COUNT(DISTINCT S_I_ID) FROM ORDER_LINE, STOCK: the lines of
    // the last 20 orders, each one's stock row by its key.
    n.statement()?;
    n.read_lock(&names.order_line.0)?;
    n.read_lock(&names.stock.0)?;
    let probe = Probe::Range { equal: vec![int(w), int(d)], low: Box::new((CompOp::Ge, int(next - 20))), high: Box::new((CompOp::Lt, int(next))) };
    let mut lines = Vec::new();
    n.scan((&names.order_line.0, &names.order_line.1), &probe, false, &mut |_, row| {
        lines.push(row[4].clone());
        Ok(true)
    })?;
    let mut low = HashSet::new();
    for item in lines {
        let Some(i) = whole(&item) else { continue };
        if let Some((_, stock)) = n.get((&names.stock.0, &names.stock.1), vec![int(w), int(i)])?
            && whole(&stock[2]).is_some_and(|q| q < threshold)
        {
            low.insert(i);
        }
    }
    std::hint::black_box(low.len());
    Ok(())
}

/// OSTAT, of both sets.
fn order_status(n: &NativeTx<'_>, names: &Names, values: &[Value]) -> Result<()> {
    let (w, d, c, byname) = (arg(values, 0), arg(values, 1), arg(values, 2), arg(values, 3));
    let last = text(&values[4]).to_owned();
    let mut c_id = Some(c);
    if byname == 1 {
        n.statement()?;
        n.read_lock(&names.customer.0)?;
        let (count, _) = by_name(n, names, (w, d, &last), None)?;
        let take = ((count + count % 2) / 2 + 1).min(count);
        n.statement()?;
        if take > 0
            && let (_, Some(row)) = by_name(n, names, (w, d, &last), Some(take))?
        {
            c_id = whole(&row[2]);
        }
    } else {
        n.statement()?;
        n.read_lock(&names.customer.0)?;
        let found = n.get((&names.customer.0, &names.customer.1), vec![int(w), int(d), int(c)])?;
        std::hint::black_box(&found);
    }
    // The customer's last order, by the index of orders by customer.
    n.statement()?;
    n.read_lock(&names.orders.0)?;
    let mut o_id = None;
    if let Some(c_id) = c_id {
        let probe = Probe::Prefix { equal: vec![int(w), int(d), int(c_id)], then: None };
        n.scan((&names.orders.0, &names.orders_by_customer), &probe, true, &mut |_, row| {
            o_id = whole(&row[2]);
            Ok(false)
        })?;
    }
    // FOR OL AS SELECT ... FROM ORDER_LINE
    n.statement()?;
    n.read_lock(&names.order_line.0)?;
    let (mut lines, mut amount) = (0, 0_i128);
    if let Some(o_id) = o_id {
        let probe = Probe::Prefix { equal: vec![int(w), int(d), int(o_id)], then: None };
        n.scan((&names.order_line.0, &names.order_line.1), &probe, false, &mut |_, row| {
            lines += 1;
            amount += units(&row[6], 2).unwrap_or(0);
            Ok(true)
        })?;
    }
    std::hint::black_box((lines, amount));
    Ok(())
}
