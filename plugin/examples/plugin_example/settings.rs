//! The example's settings and eviction policy (RULES.md D280 to D284):
//! SETTING_OF gives the value a call runs with of the setting it names,
//! RUN runs the statement it is given, as a program writing a setting
//! would, and the host object KEEP is a buffer eviction policy keeping
//! the pages of the data file its plug-in's settings KEEP_FROM and
//! KEEP_PAGES say, noting what the pool reports.

use turborel::value::Value;
use turborel_plugin::host::{EvictionAdvisor, Policy, Report, policies};
use turborel_plugin::{Answer, Cx, Plugin, Procedure, Scalar};

use super::host_small::{crash_if, note};

/// The settings KEEP reads, after its plug-in's name: the first page it
/// keeps, and how many.
const KEEP_FROM: &str = ".KEEP_FROM";
/// See `KEEP_FROM`.
const KEEP_PAGES: &str = ".KEEP_PAGES";

/// The plug-in, with these objects.
pub fn with(plugin: Plugin) -> Plugin {
    plugin.scalar("SETTING_OF", SettingOf).procedure("RUN", Run).host_eviction("KEEP", Keep)
}

/// The value of the setting its argument names, as text; NULL of one the
/// call is not told.
struct SettingOf;

impl Scalar for SettingOf {
    fn call(&mut self, cx: &mut Cx<'_>, args: &[Value]) -> Answer<Value> {
        let Some(Value::Char(name)) = args.first() else { return Ok(Value::Null) };
        Ok(cx.setting(name).map_or(Value::Null, |v| Value::Char(v.to_string())))
    }
}

/// Runs the statement it is given.
struct Run;

impl Procedure for Run {
    fn call(&mut self, cx: &mut Cx<'_>, args: &[Value]) -> Answer<()> {
        let Some(Value::Char(text)) = args.first() else { return Ok(()) };
        cx.run(text, &[])
    }
}

/// Keeps KEEP_PAGES pages of file 0, the data file, from page KEEP_FROM,
/// as least recently used weighs the rest; with none to keep, leaves the
/// pool's policy as it is. Each report's pages evicted are appended to
/// `evicted.log`, its frames sampled written to `resident.log`, and each
/// policy pushed noted in `policy.log`.
struct Keep;

/// The whole number of the setting of `cx` whose name ends in `suffix`, 0
/// if none.
fn of(cx: &Cx<'_>, suffix: &str) -> u64 {
    let name = cx.settings().iter().find(|(n, _)| n.ends_with(suffix)).map(|(n, _)| n.clone());
    name.and_then(|n| cx.integer(&n)).and_then(|k| u64::try_from(k).ok()).unwrap_or(0)
}

impl EvictionAdvisor for Keep {
    fn advise(&mut self, cx: &mut Cx<'_>, report: &Report) -> Answer<Option<Policy>> {
        crash_if("advise");
        for (file, page) in &report.evicted {
            note("evicted.log", &format!("{file} {page}"));
        }
        let resident: Vec<String> = report.sampled.iter().map(|s| format!("{} {}", s.file, s.page)).collect();
        // Whole or not at all, as a test reads it while it is written.
        if std::fs::write("resident.tmp", resident.join("\n")).is_ok() {
            drop(std::fs::rename("resident.tmp", "resident.log"));
        }
        let (from, pages) = (of(cx, KEEP_FROM), of(cx, KEEP_PAGES));
        if pages == 0 {
            return Ok(None);
        }
        note("policy.log", &format!("keep {from} {pages}"));
        Ok(Some(policies::weighing(policies::lru(), 0, (from, from.saturating_add(pages).saturating_sub(1)), 0)))
    }
}
