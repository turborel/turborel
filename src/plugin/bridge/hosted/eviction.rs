//! \[EXTENSION\] A host plug-in's BUFFER EVICTION (RULES.md D283, D284).
//!
//! The pool chooses a victim on every miss, so the program is never asked
//! there. It programs the pool's own evictor (`disk/buffer/policy.rs`)
//! instead: a job of the database sends it the pool's report every EVERY
//! milliseconds (1000 if not written), one call, and pushes the policy it
//! answers into the table every evictor reads with no lock. The first is
//! asked as the plug-in is registered, before any pool is made. A program
//! that fails a call stops the node (RULES.md D249), and the pool goes on
//! choosing by the last policy pushed until the node stops.

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use crate::db::Database;
use crate::disk::buffer::{Eviction, MakeEviction, Report, Table, Weighted};
use crate::error::Result;
use crate::plugin::bridge::answer;
use crate::plugin::hosted::HostLink;
use crate::plugin::manifest::HostDeclared;
use crate::plugin::payload::eviction::{EVICTION_ADVISE, answer as advised, put_report};
use crate::threads::BackgroundJob;

/// The milliseconds between reports when the declaration writes no EVERY.
const DEFAULT_EVERY_MS: u64 = 1000;
/// The frames a report samples when the declaration writes no SAMPLE.
const DEFAULT_SAMPLED: u64 = 256;

/// A host plug-in's eviction policy: the table its evictors read, and the
/// evictors made, for their reports.
#[derive(Debug)]
pub struct PluginEviction {
    link: HostLink,
    table: Arc<Table>,
    made: Mutex<Vec<Weak<Weighted>>>,
    every: Duration,
    sampled: usize,
}

impl PluginEviction {
    /// The policy `d` declares, the program asked its first now.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T11 if the program does not answer, or answers no policy;
    /// 38T06 if EVERY or SAMPLE is written as text.
    pub fn new(d: &HostDeclared, link: HostLink) -> Result<Arc<PluginEviction>> {
        let every = Duration::from_millis(d.number("EVERY", DEFAULT_EVERY_MS)?.max(1));
        let sampled = usize::try_from(d.number("SAMPLE", DEFAULT_SAMPLED)?).unwrap_or(usize::MAX);
        let made = Arc::new(PluginEviction { link, table: Arc::default(), made: Mutex::default(), every, sampled });
        made.advise()?;
        Ok(made)
    }

    /// What makes the evictor of a pool, reading its table.
    pub fn maker(self: &Arc<Self>) -> MakeEviction {
        let me = Arc::clone(self);
        Arc::new(move |frames| {
            let evictor = Arc::new(Weighted::new(frames, Arc::clone(&me.table)));
            crate::sync::lock_builtin(&me.made).push(Arc::downgrade(&evictor));
            Box::new(evictor) as Box<dyn Eviction>
        })
    }

    /// The time between its reports.
    pub fn every(&self) -> Duration {
        self.every
    }

    /// The table its evictors read.
    pub fn table(&self) -> &Arc<Table> {
        &self.table
    }

    /// Sends the program the report of every evictor alive, the first's
    /// if several, and pushes the policy it answers, if it changed one.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T11 if the program does not answer, or answers no policy.
    pub fn advise(&self) -> Result<()> {
        let alive: Vec<Arc<Weighted>> = {
            let mut made = crate::sync::lock_builtin(&self.made);
            made.retain(|w| w.strong_count() > 0);
            made.iter().filter_map(Weak::upgrade).collect()
        };
        let report = alive.first().map_or_else(Report::default, |e| e.report(self.sampled));
        let mut payload = Vec::new();
        put_report(&mut payload, &report);
        let got = self.link.call(EVICTION_ADVISE, payload)?;
        let policy = answer(&got, advised).map_err(|e| self.link.stopped_by(e))?;
        if let Some(policy) = policy {
            self.table.push(&policy);
            alive.iter().for_each(|e| e.reweigh());
        }
        Ok(())
    }

    /// The job that sends its reports.
    pub fn advisor(self: &Arc<Self>) -> Arc<dyn BackgroundJob> {
        Arc::new(Advisor(Arc::clone(self)))
    }
}

/// The job sending a host eviction policy its reports.
struct Advisor(Arc<PluginEviction>);

impl BackgroundJob for Advisor {
    fn label(&self) -> &'static str {
        "turborel-host-eviction"
    }
    fn run(&self, _db: &Database) -> Result<Option<Duration>> {
        self.0.advise()?;
        Ok(Some(self.0.every))
    }
}
