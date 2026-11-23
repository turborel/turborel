//! The sysbench-style workloads: each event of a script, on ten tables of
//! `id`, `k`, `c` and `pad`, keyed by `id` and indexed by `k`. Each
//! statement is the script's, prepared once for each table as the driver
//! prepares it, and each key is drawn uniformly over the table's rows, as
//! the scripts draw them by default. A script of BEGIN and COMMIT runs its
//! statements in one transaction, begun by SET TRANSACTION as the driver
//! begins it; a statement outside one ends its own transaction with it.

use std::collections::BTreeMap;
use std::time::Instant;

use turborel::value::Value;

use crate::client::{Client, Failed};
use crate::random::Random;
use crate::tpcc_load::int;

/// The tables, `sbtest1` to `sbtest10`.
pub const TABLES: i64 = 10;
/// The rows each range query reads.
const RANGE_SIZE: i64 = 100;
/// The point queries of a transaction of the read-only and read-write
/// scripts.
const POINT_SELECTS: usize = 10;
/// The template of `c`, ten groups of eleven digits.
pub const C_TEMPLATE: &str =
    "###########-###########-###########-###########-###########-###########-###########-###########-###########-###########";
/// The template of `pad`, five groups.
pub const PAD_TEMPLATE: &str = "###########-###########-###########-###########-###########";
/// How the driver begins a transaction, at its default isolation level.
const BEGIN: &str = "SET TRANSACTION ISOLATION LEVEL READ COMMITTED";

/// The statements prepared for each table, `%u` its number, `%s` the padding
/// column, which turborel's key words take as `"pad"`.
const STATEMENTS: [&str; 9] = [
    "SELECT c FROM sbtest%u WHERE id=?",
    "SELECT c FROM sbtest%u WHERE id BETWEEN ? AND ?",
    "SELECT SUM(k) FROM sbtest%u WHERE id BETWEEN ? AND ?",
    "SELECT c FROM sbtest%u WHERE id BETWEEN ? AND ? ORDER BY c",
    "SELECT DISTINCT c FROM sbtest%u WHERE id BETWEEN ? AND ? ORDER BY c",
    "UPDATE sbtest%u SET k=k+1 WHERE id=?",
    "UPDATE sbtest%u SET c=? WHERE id=?",
    "DELETE FROM sbtest%u WHERE id=?",
    "INSERT INTO sbtest%u (id, k, c, %s) VALUES (?, ?, ?, ?)",
];
const POINT: usize = 0;
const RANGES: [usize; 4] = [1, 2, 3, 4];
const INDEX_UPDATE: usize = 5;
const NON_INDEX_UPDATE: usize = 6;
const DELETE: usize = 7;
const INSERT: usize = 8;

/// A script's steps, in order.
#[derive(Clone, Copy)]
enum Step {
    Points(usize),
    Ranges,
    IndexUpdate,
    NonIndexUpdate,
    DeleteInsert,
    Delete,
    Insert,
}

/// A script: its name, whether it is one transaction, and its steps.
pub struct Script {
    pub name: &'static str,
    transaction: bool,
    steps: &'static [Step],
}

const WRITES: [Step; 3] = [Step::IndexUpdate, Step::NonIndexUpdate, Step::DeleteInsert];
const READS: [Step; 2] = [Step::Points(POINT_SELECTS), Step::Ranges];
const READ_WRITE: [Step; 5] = [READS[0], READS[1], WRITES[0], WRITES[1], WRITES[2]];

/// The scripts, by name.
pub const SCRIPTS: [Script; 8] = [
    Script { name: "point_select", transaction: false, steps: &[Step::Points(1)] },
    Script { name: "read_only", transaction: true, steps: &READS },
    Script { name: "read_write", transaction: true, steps: &READ_WRITE },
    Script { name: "write_only", transaction: true, steps: &WRITES },
    Script { name: "update_index", transaction: false, steps: &[Step::IndexUpdate] },
    Script { name: "update_non_index", transaction: false, steps: &[Step::NonIndexUpdate] },
    Script { name: "insert", transaction: false, steps: &[Step::Insert] },
    Script { name: "delete", transaction: false, steps: &[Step::Delete] },
];

/// The text of statement `n` for table `t`.
fn statement(n: usize, t: i64) -> String {
    STATEMENTS[n].replace("%u", &t.to_string()).replace("%s", "\"pad\"")
}

/// What a terminal did: events, and failures by SQLCODE.
#[derive(Default)]
pub struct Tally {
    pub events: u64,
    pub failed: BTreeMap<Failed, u64>,
}

/// A thread of a script: its client, its statements and its draws.
pub struct Terminal<'a, C: Client> {
    client: C,
    script: &'a Script,
    begin: Option<C::Statement>,
    commit: Option<C::Statement>,
    statements: Vec<Vec<C::Statement>>,
    random: Random,
    rows: i64,
}

impl<'a, C: Client> Terminal<'a, C> {
    /// The terminal of `client` running `script` on tables of `rows` rows.
    pub fn new(mut client: C, script: &'a Script, rows: i64, seed: u64) -> Terminal<'a, C> {
        let statements = (1..=TABLES).map(|t| (0..STATEMENTS.len()).map(|n| client.prepare(&statement(n, t)).unwrap()).collect()).collect();
        let (begin, commit) =
            if script.transaction { (Some(client.prepare(BEGIN).unwrap()), Some(client.prepare("COMMIT").unwrap())) } else { (None, None) };
        Terminal { client, script, begin, commit, statements, random: Random::new(seed), rows }
    }

    /// Runs events until `until`.
    pub fn run(&mut self, until: Instant) -> Tally {
        let mut tally = Tally::default();
        while Instant::now() < until {
            tally.events += 1;
            if let Err(failed) = self.event() {
                *tally.failed.entry(failed).or_default() += 1;
                if self.script.transaction {
                    let _ = self.client.rollback();
                }
            }
        }
        tally
    }

    fn event(&mut self) -> Result<(), Failed> {
        let ends = !self.script.transaction;
        if let Some(begin) = &self.begin {
            self.client.execute(begin, &[], false)?;
        }
        for step in self.script.steps {
            self.step(*step, ends)?;
        }
        if let Some(commit) = &self.commit {
            self.client.execute(commit, &[], false)?;
        }
        Ok(())
    }

    fn id(&mut self) -> i64 {
        self.random.number(1, self.rows)
    }

    fn run_on(&mut self, (t, n): (usize, usize), values: &[Value], ends: bool) -> Result<(), Failed> {
        self.client.execute(&self.statements[t][n], values, ends).map(drop)
    }

    fn step(&mut self, step: Step, ends: bool) -> Result<(), Failed> {
        let t = (self.random.number(1, TABLES) - 1) as usize;
        match step {
            Step::Points(n) => (0..n).try_for_each(|_| {
                let id = self.id();
                self.run_on((t, POINT), &[int(id)], ends)
            }),
            // Each kind of range query draws a table of its own.
            Step::Ranges => RANGES.iter().enumerate().try_for_each(|(i, &n)| {
                let table = if i == 0 { t } else { (self.random.number(1, TABLES) - 1) as usize };
                let id = self.id();
                self.run_on((table, n), &[int(id), int(id + RANGE_SIZE - 1)], ends)
            }),
            Step::IndexUpdate => {
                let id = self.id();
                self.run_on((t, INDEX_UPDATE), &[int(id)], ends)
            }
            Step::NonIndexUpdate => {
                let c = Value::Char(self.random.template(C_TEMPLATE).into());
                let id = self.id();
                self.run_on((t, NON_INDEX_UPDATE), &[c, int(id)], ends)
            }
            Step::DeleteInsert => {
                let (id, k) = (self.id(), self.id());
                self.run_on((t, DELETE), &[int(id)], ends)?;
                let (c, pad) = (self.random.template(C_TEMPLATE), self.random.template(PAD_TEMPLATE));
                self.run_on((t, INSERT), &[int(id), int(k), Value::Char(c.into()), Value::Char(pad.into())], ends)
            }
            Step::Delete => {
                let id = self.id();
                self.run_on((t, DELETE), &[int(id)], ends)
            }
            Step::Insert => self.insert(t as i64 + 1),
        }
    }

    /// The insert script's INSERT, as direct SQL text, its id the table's
    /// identity's.
    fn insert(&mut self, table: i64) -> Result<(), Failed> {
        let k = self.id();
        let (c, pad) = (self.random.template(C_TEMPLATE), self.random.template(PAD_TEMPLATE));
        let sql = format!("INSERT INTO sbtest{table} (k, c, \"pad\") VALUES ({k}, '{c}', '{pad}')");
        self.client.direct(&sql, true).map(drop)
    }
}
