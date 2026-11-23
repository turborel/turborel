//! What a workload's terminal speaks to: a session in this process, or a
//! connection to a server (the protocol mode, an example of the server's
//! crate). Each statement is prepared once and executed with new values,
//! as the drivers the workloads follow use them; one that `ends` the
//! transaction commits it once it succeeds and rolls it back if it fails,
//! in the same request, as the protocol's ends flag does.

use turborel::db::{Database, Direct, Prepared, Session};
use turborel::error::SqlError;
use turborel::value::Value;

/// A statement that failed: its SQLCODE, by which failures are counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Failed(pub i32);

impl From<SqlError> for Failed {
    fn from(e: SqlError) -> Failed {
        Failed(e.sqlcode())
    }
}

/// A terminal's session.
pub trait Client {
    /// A statement prepared.
    type Statement;

    /// Prepares `sql`.
    fn prepare(&mut self, sql: &str) -> Result<Self::Statement, Failed>;

    /// Executes `statement` with `values`, its rows read to the end: how
    /// many rows it gave, 0 for a statement that is no query.
    fn execute(&mut self, statement: &Self::Statement, values: &[Value], ends: bool) -> Result<u64, Failed>;

    /// Runs `sql` as direct SQL, a query's rows returned.
    fn direct(&mut self, sql: &str, ends: bool) -> Result<Vec<Vec<Value>>, Failed>;

    /// Rolls the transaction back.
    fn rollback(&mut self) -> Result<(), Failed>;
}

/// Makes a terminal's client, on the thread that uses it.
pub trait Connect: Sync {
    /// The client it makes.
    type Client: Client;

    /// A new client, as `user`.
    fn connect(&self, user: &str) -> Self::Client;
}

/// A session of a database in this process.
pub struct Embedded(pub Session);

impl Embedded {
    /// The transaction ended as `ends` asks, after `outcome`.
    fn ended<T>(&self, outcome: Result<T, SqlError>, ends: bool) -> Result<T, Failed> {
        match outcome {
            Ok(value) => {
                if ends {
                    self.0.commit()?;
                }
                Ok(value)
            }
            Err(e) => {
                if ends {
                    // The failure is the one reported; a rollback that fails
                    // too fails the next statement.
                    let _ = self.0.rollback();
                }
                Err(e.into())
            }
        }
    }

    /// The rows of `statement` with `values`, read to the end.
    fn rows(&self, statement: &Prepared, values: &[Value]) -> Result<u64, SqlError> {
        let mut cursor = self.0.open_prepared(statement, values)?;
        let mut rows = 0;
        while cursor.fetch()?.is_some() {
            rows += 1;
        }
        Ok(rows)
    }
}

impl Client for Embedded {
    type Statement = Prepared;

    fn prepare(&mut self, sql: &str) -> Result<Prepared, Failed> {
        Ok(self.0.prepare(sql)?)
    }

    fn execute(&mut self, statement: &Prepared, values: &[Value], ends: bool) -> Result<u64, Failed> {
        let outcome = if statement.columns().is_empty() {
            // Told once committed, as a server tells a request that ends
            // its transaction (RULES.md D846).
            self.0.tell_at_end(ends);
            let done = self.0.execute_prepared(statement, values).map(|_| 0);
            self.0.tell_at_end(false);
            done
        } else {
            self.rows(statement, values)
        };
        self.ended(outcome, ends)
    }

    fn direct(&mut self, sql: &str, ends: bool) -> Result<Vec<Vec<Value>>, Failed> {
        let outcome = self.0.direct(sql).and_then(|direct| match direct {
            Direct::Rows(mut cursor) => {
                let mut rows = Vec::new();
                while let Some(row) = cursor.fetch()? {
                    rows.push(row);
                }
                Ok(rows)
            }
            Direct::Done(_) => Ok(Vec::new()),
        });
        self.ended(outcome, ends)
    }

    fn rollback(&mut self) -> Result<(), Failed> {
        Ok(self.0.rollback()?)
    }
}

/// Sessions of a database in this process.
pub struct InProcess(pub Database);

impl Connect for InProcess {
    type Client = Embedded;

    fn connect(&self, user: &str) -> Embedded {
        Embedded(self.0.session(user))
    }
}
