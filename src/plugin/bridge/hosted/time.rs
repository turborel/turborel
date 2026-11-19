//! \[EXTENSION\] A host plug-in's clock and background jobs (RULES.md
//! D247, D253).
//!
//! The clock is read in a commit's critical section, so it is never a
//! call there. The program's time is pushed into a value of the
//! database's own, by a job asking it every EVERY milliseconds (10 if not
//! written), and the clock reads that value, never going back. A job of a
//! host plug-in is one call a run, which says when to run again.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use crate::clock::Clock;
use crate::db::Database;
use crate::error::Result;
use crate::plugin::bridge::{answer, leaked};
use crate::plugin::hosted::HostLink;
use crate::plugin::manifest::HostDeclared;
use crate::plugin::payload::hosted::{CLOCK_NOW, JOB_RUN};
use crate::threads::BackgroundJob;

/// A host plug-in's clock: the program's time, last pushed.
#[derive(Debug)]
pub struct PluginClock {
    link: HostLink,
    name: &'static str,
    now: AtomicI64,
    every: Duration,
}

impl PluginClock {
    /// The clock `d` declares, its time asked once now.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T11 if the program does not answer.
    pub fn new(d: &HostDeclared, link: HostLink) -> Result<Arc<PluginClock>> {
        let every = Duration::from_millis(d.number("EVERY", 10)?.max(1));
        let clock = Arc::new(PluginClock { link, name: leaked(&d.name), now: AtomicI64::new(i64::MIN), every });
        clock.tick()?;
        Ok(clock)
    }

    /// Asks the program its time and keeps it, never going back.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T11 if the program does not answer.
    pub fn tick(&self) -> Result<()> {
        let got = self.link.call(CLOCK_NOW, Vec::new())?;
        let micros = answer(&got, micros)?;
        let now =
            i64::try_from(micros).map_err(|_| crate::plugin::errors::violation("the clock gave a time past 64 bits of microseconds"))?;
        self.now.fetch_max(now, Ordering::AcqRel);
        Ok(())
    }

    /// The job that pushes its time.
    pub fn ticker(self: &Arc<Self>) -> Arc<dyn BackgroundJob> {
        Arc::new(Ticker(Arc::clone(self)))
    }
}

impl Clock for PluginClock {
    fn name(&self) -> &'static str {
        self.name
    }
    fn now(&self) -> i64 {
        self.now.load(Ordering::Acquire)
    }
}

/// The job pushing a host clock's time.
struct Ticker(Arc<PluginClock>);

impl BackgroundJob for Ticker {
    fn label(&self) -> &'static str {
        "turborel-host-clock"
    }
    fn run(&self, _db: &Database) -> Result<Option<Duration>> {
        self.0.tick()?;
        Ok(Some(self.0.every))
    }
}

/// A host plug-in's background job.
#[derive(Debug)]
pub struct PluginJob {
    link: HostLink,
    label: &'static str,
}

impl PluginJob {
    /// The job `d` declares, labelled `turborel-<name>` in lower case.
    pub fn new(d: &HostDeclared, link: HostLink) -> PluginJob {
        PluginJob { link, label: leaked(&format!("turborel-{}", d.name.to_ascii_lowercase())) }
    }
}

impl BackgroundJob for PluginJob {
    fn label(&self) -> &'static str {
        self.label
    }
    fn run(&self, _db: &Database) -> Result<Option<Duration>> {
        let got = self.link.call(JOB_RUN, Vec::new())?;
        answer(&got, |r| Ok(if r.u8()? == 0 { None } else { Some(Duration::from_millis(r.u64()?)) }))
    }
}

/// The microseconds a clock's answer holds.
fn micros(r: &mut crate::bytes::Reader<'_>) -> Result<i128> {
    r.i128()
}
