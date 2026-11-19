//! The example's functions, aggregate, table functions and procedures.

use std::io::Write;

use turborel::value::{Decimal, Value};
use turborel_plugin::{Aggregate, Answer, Cx, Plugin, PluginError, Procedure, Scalar, TableFunction};

/// The plug-in with them.
pub fn with(plugin: Plugin) -> Plugin {
    let plugin = ["TWICE", "SHOUT", "PID", "FAIL", "CRASH", "HANG", "GARBAGE", "HUGE"]
        .into_iter()
        .fold(plugin, |p, name| p.scalar(name, Function(name)));
    plugin
        .scalar("SHOUT2", Function("SHOUT"))
        .aggregate("PRODUCT", Product)
        .table_function("SERIES", Series)
        .table_function("ROW_COUNT", RowCount)
        .procedure("NOTE", Note)
        .procedure("PEEK", Note)
}

/// A scalar function, by its name.
struct Function(&'static str);

impl Scalar for Function {
    fn call(&mut self, _cx: &mut Cx<'_>, args: &[Value]) -> Answer<Value> {
        let first = args.first().cloned().unwrap_or(Value::Null);
        let takes = !matches!(self.0, "PID" | "CRASH" | "HANG" | "GARBAGE");
        if takes && first.is_null() {
            return Ok(Value::Null);
        }
        Ok(match (self.0, first) {
            ("TWICE", Value::Exact(d)) => {
                Value::Exact(d.checked_mul(Decimal::new(2, 0)).map_err(|_| PluginError::new("22003", "out of range"))?)
            }
            ("SHOUT", Value::Char(s)) => Value::Char(s.to_uppercase()),
            ("PID", _) => Value::Exact(Decimal::new(i128::from(std::process::id()), 0)),
            ("FAIL", Value::Char(s)) => return Err(PluginError::new("22012", &s)),
            ("CRASH", _) => std::process::abort(),
            ("HANG", _) => loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            },
            ("GARBAGE", _) => {
                let _ = std::io::stdout().write_all(&[1, 0, 0, 0, 99]).and_then(|()| std::io::stdout().flush());
                Value::Null
            }
            ("HUGE", Value::Exact(n)) => Value::Char("x".repeat(usize::try_from(n.units).unwrap_or(0))),
            _ => return Err(PluginError::external("no such function, or an argument of a type it takes not")),
        })
    }
}

/// PRODUCT: its state the product so far, eight bytes, or none.
struct Product;

/// The product a state holds.
fn product(state: &[u8]) -> Option<f64> {
    state.try_into().ok().map(|b: [u8; 8]| f64::from_bits(u64::from_le_bytes(b)))
}

/// `x` as a state.
fn state_of(x: f64) -> Vec<u8> {
    x.to_bits().to_le_bytes().to_vec()
}

impl Aggregate for Product {
    fn step(&mut self, state: &mut Vec<u8>, value: &Value) -> Answer<()> {
        let x = if let Value::Approx(x) = value {
            *x
        } else if let Value::Exact(d) = value {
            d.to_f64()
        } else {
            return Ok(());
        };
        *state = state_of(product(state).unwrap_or(1.0_f64) * x);
        Ok(())
    }
    fn merge(&mut self, state: &mut Vec<u8>, other: &[u8]) -> Answer<()> {
        let merged = match (product(state), product(other)) {
            (Some(a), Some(b)) => Some(a * b),
            (a, b) => a.or(b),
        };
        *state = merged.map(state_of).unwrap_or_default();
        Ok(())
    }
    fn finish(&mut self, state: &[u8]) -> Answer<Value> {
        Ok(product(state).map_or(Value::Null, Value::Approx))
    }
}

/// SERIES(n): the integers from 1 to n.
struct Series;

impl TableFunction for Series {
    fn rows(&mut self, _cx: &mut Cx<'_>, args: &[Value], _read: &[Vec<Vec<Value>>]) -> Answer<Vec<Vec<Value>>> {
        let Some(Value::Exact(n)) = args.first() else { return Ok(Vec::new()) };
        Ok((1..=n.units.min(1_000_000)).map(|i| vec![Value::Exact(Decimal::new(i, 0))]).collect())
    }
}

/// ROW_COUNT(t): the rows of table `t`, read by the database first.
struct RowCount;

impl TableFunction for RowCount {
    fn reads(&mut self, args: &[Value]) -> Answer<Vec<String>> {
        let Some(Value::Char(t)) = args.first() else { return Ok(Vec::new()) };
        Ok(vec![format!("SELECT COUNT(*) FROM {t}")])
    }
    fn rows(&mut self, _cx: &mut Cx<'_>, args: &[Value], read: &[Vec<Vec<Value>>]) -> Answer<Vec<Vec<Value>>> {
        let Some(Value::Char(t)) = args.first() else { return Ok(Vec::new()) };
        let count = read.iter().flatten().flatten().next().cloned().unwrap_or(Value::Null);
        Ok(vec![vec![Value::Char(t.clone()), count]])
    }
}

/// NOTE: inserts its text into the table NOTES, by SQL in the call's
/// transaction.
struct Note;

impl Procedure for Note {
    fn call(&mut self, cx: &mut Cx<'_>, args: &[Value]) -> Answer<()> {
        cx.run("INSERT INTO NOTES VALUES (?)", args)
    }
}
