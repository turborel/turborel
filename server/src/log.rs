//! \[DESIGN\] The server's diagnostics sink (RULES.md D40, D42): each event
//! a line of JSON, stamped with the time it was taken, in UTC to the
//! millisecond (RFC 3339), with its level and its kind, then its fields:
//! a program routes and filters the lines, a person reads them. They go to
//! standard error, or are appended to a file.
//!
//! ```text
//! {"time":"2026-10-27T20:15:56.701Z","level":"warning","event":"slow_statement","user":"ANN","millis":2400,"statement":"SELECT ..."}
//! ```
//!
//! A line that cannot be written is lost: the database never waits on, nor
//! fails for, its diagnostics.

use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use turborel::db::Opened;
use turborel::diagnostics::{Diagnostics, Event, Level};
use turborel::files::Noted;
use turborel::threads::ThreadChange;

/// \[DESIGN\] A sink writing each event as a line of JSON.
pub struct Lines {
    out: Mutex<Box<dyn Write + Send>>,
    /// The file it appends to, shown among the database's (RULES.md D56).
    noted: Option<Noted>,
}

impl Lines {
    /// Lines written to `out`.
    pub fn new(out: Box<dyn Write + Send>) -> Lines {
        Lines { out: Mutex::new(out), noted: None }
    }

    /// The lines, each counted a write of the file `noted` shows (RULES.md
    /// D56).
    #[must_use]
    pub fn noting(self, noted: Noted) -> Lines {
        Lines { noted: Some(noted), ..self }
    }

    /// Lines written to standard error.
    pub fn to_stderr() -> Lines {
        Lines::new(Box::new(io::stderr()))
    }

    /// Lines appended to the file at `path`, made if it is not there.
    ///
    /// # Errors
    ///
    /// The file cannot be opened for appending.
    pub fn appending(path: &Path) -> io::Result<Lines> {
        let mut options = OpenOptions::new();
        // \[DESIGN\] Made its user's alone, as the database's files are
        // (RULES.md D56).
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        Ok(Lines::new(Box::new(options.create(true).append(true).open(path)?)))
    }
}

impl Diagnostics for Lines {
    fn event(&self, event: &Event) {
        let text = line(SystemTime::now(), event);
        // Nothing panics while it is held, so a poisoned lock is whole.
        let mut out = self.out.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = writeln!(out, "{text}").and_then(|()| out.flush());
        if let Some(noted) = &self.noted {
            noted.wrote();
        }
    }
}

/// `event`, taken at `at`, as a line of JSON.
pub fn line(at: SystemTime, event: &Event) -> String {
    let mut json = Json::default();
    json.text("time", &timestamp(at));
    json.text("level", level(event.level()));
    fields(&mut json, event);
    json.done()
}

fn level(level: Level) -> &'static str {
    match level {
        Level::Error => "error",
        Level::Warning => "warning",
        Level::Info => "info",
    }
}

/// A thread's event (RULES.md D55): its label and what happened, written
/// to `json`.
fn thread(json: &mut Json, label: &str, change: &ThreadChange) {
    json.text("event", "thread");
    json.text("label", label);
    match change {
        ThreadChange::Started => json.text("change", "started"),
        ThreadChange::Stopped => json.text("change", "stopped"),
        ThreadChange::Restarted => json.text("change", "restarted"),
        ThreadChange::Failed(e) => {
            json.text("change", "failed");
            json.raw("sqlcode", &e.sqlcode().to_string());
            json.text("message", &e.to_string());
        }
    }
}

/// `event`'s kind and fields, written to `json`.
fn fields(json: &mut Json, event: &Event) {
    match event {
        Event::Opened(how) => opened(json, how),
        Event::Closed => json.text("event", "closed"),
        Event::CheckpointTaken(taken) => {
            json.text("event", "checkpoint_taken");
            json.number("taken", *taken);
        }
        Event::SettingChanged { name, value } => {
            json.text("event", "setting_changed");
            json.text("name", name);
            match value {
                Some(value) => json.text("value", &value.to_literal()),
                None => json.raw("value", "null"),
            }
        }
        Event::UserChanged { name, dropped } => {
            json.text("event", "user_changed");
            json.text("name", name);
            json.raw("dropped", if *dropped { "true" } else { "false" });
        }
        Event::OwnerChanged { name } => {
            json.text("event", "owner_changed");
            json.text("name", name);
        }
        Event::Registered { kind, name } => {
            json.text("event", "registered");
            json.text("kind", &format!("{kind:?}"));
            json.text("name", name);
        }
        Event::Failed(e) => {
            json.text("event", "failed");
            json.raw("sqlcode", &e.sqlcode().to_string());
            json.text("sqlstate", e.sqlstate().as_str());
            json.text("message", &e.to_string());
        }
        Event::Thread { label, change } => thread(json, label, change),
        Event::SlowStatement { user, millis, statement } => slow(json, user, *millis, statement.as_deref()),
        Event::FilesReopened { reopened, limit } => {
            json.text("event", "files_reopened");
            json.number("reopened", *reopened);
            json.number("limit", *limit);
        }
        // An event of a later database than this sink knows: kept whole.
        other => {
            json.text("event", "other");
            json.text("debug", &format!("{other:?}"));
        }
    }
}

/// The fields of a statement of `user` whose work took `millis`, its text
/// `statement` if it may be told.
fn slow(json: &mut Json, user: &str, millis: u64, statement: Option<&str>) {
    json.text("event", "slow_statement");
    json.text("user", user);
    json.number("millis", millis);
    match statement {
        Some(text) => json.text("statement", text),
        None => json.raw("statement", "null"),
    }
}

/// The fields of a database opened `how`.
fn opened(json: &mut Json, how: &Opened) {
    json.text("event", "opened");
    match how {
        Opened::Created => json.text("how", "created"),
        Opened::Reopened => json.text("how", "reopened"),
        Opened::Recovered(report) => {
            json.text("how", "recovered");
            json.number("records", report.records);
            json.number("definitions", report.definitions);
            json.number("committed", report.committed);
            json.number("not_committed", report.not_committed);
            json.number("redone", report.redone);
            json.number("undone", report.undone);
        }
    }
}

/// A JSON object, written a field at a time.
#[derive(Default)]
struct Json(String);

impl Json {
    /// A field whose value is written as it is: a number or a literal.
    fn raw(&mut self, name: &str, value: &str) {
        self.0.push(if self.0.is_empty() { '{' } else { ',' });
        quoted(&mut self.0, name);
        self.0.push(':');
        self.0.push_str(value);
    }

    fn text(&mut self, name: &str, value: &str) {
        let mut string = String::new();
        quoted(&mut string, value);
        self.raw(name, &string);
    }

    fn number(&mut self, name: &str, value: u64) {
        self.raw(name, &value.to_string());
    }

    fn done(mut self) -> String {
        self.0.push('}');
        self.0
    }
}

/// `s` as a JSON string, appended to `out` [RFC 8259 §7].
fn quoted(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0}'..='\u{1f}' => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `at` in UTC, to the millisecond, as RFC 3339 writes it.
fn timestamp(at: SystemTime) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = since.as_secs();
    let (days, of_day) = (seconds.div_euclid(SECONDS_PER_DAY), seconds.rem_euclid(SECONDS_PER_DAY));
    let (year, month, day) = civil(days);
    let (hour, minute, second) = (of_day.div_euclid(3600), of_day.rem_euclid(3600).div_euclid(60), of_day.rem_euclid(60));
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z", since.subsec_millis())
}

const SECONDS_PER_DAY: u64 = 86_400;

/// The year, month and day of `days` since 1970-01-01, in the proleptic
/// Gregorian calendar: Howard Hinnant's `civil_from_days`, for days on or
/// after the epoch.
fn civil(days: u64) -> (u64, u64, u64) {
    // Days since 0000-03-01, in eras of 400 years of 146 097 days.
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = day_of_era
        .saturating_sub(day_of_era.div_euclid(1_460))
        .saturating_add(day_of_era.div_euclid(36_524))
        .saturating_sub(day_of_era.div_euclid(146_096))
        .div_euclid(365);
    let day_of_year = day_of_era.saturating_sub(
        year_of_era.saturating_mul(365).saturating_add(year_of_era.div_euclid(4)).saturating_sub(year_of_era.div_euclid(100)),
    );
    // Months from March: 0 is March, 11 February.
    let from_march = day_of_year.saturating_mul(5).saturating_add(2).div_euclid(153);
    let day = day_of_year.saturating_sub(from_march.saturating_mul(153).saturating_add(2).div_euclid(5)).saturating_add(1);
    let month = if from_march < 10 { from_march.saturating_add(3) } else { from_march.saturating_sub(9) };
    let year = era.saturating_mul(400).saturating_add(year_of_era).saturating_add(u64::from(month <= 2));
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use turborel::db::RecoveryReport;
    use turborel::diagnostics::Extension;
    use turborel::error::{Code, SqlError};
    use turborel::value::Value;

    use super::*;

    fn at(seconds: u64, millis: u64) -> SystemTime {
        UNIX_EPOCH.checked_add(Duration::from_secs(seconds).saturating_add(Duration::from_millis(millis))).unwrap()
    }

    /// Verifies: [DESIGN] RULES.md D40: times are UTC to the millisecond,
    /// as RFC 3339 writes them: the epoch, the leap day of 2000, the last
    /// moment of 1999, and a day of 2026.
    #[test]
    fn times_are_utc_as_rfc_3339_writes_them() {
        assert_eq!(timestamp(at(0, 0)), "1970-01-01T00:00:00.000Z");
        assert_eq!(timestamp(at(951_782_400, 5)), "2000-02-29T00:00:00.005Z");
        assert_eq!(timestamp(at(946_684_799, 999)), "1999-12-31T23:59:59.999Z");
        assert_eq!(timestamp(at(1_793_131_256, 701)), "2026-10-27T20:00:56.701Z");
    }

    /// Verifies: [DESIGN] RULES.md D40: each event is a line of JSON with
    /// its time, level, kind and fields, its strings escaped.
    #[test]
    fn events_are_lines_of_json() {
        let shown = |event: &Event| line(at(0, 0), event);
        let head = r#"{"time":"1970-01-01T00:00:00.000Z","#;
        let slow = Event::SlowStatement { user: "ANN".into(), millis: 2400, statement: Some("SELECT 'a\"b'\n\u{1}".into()) };
        assert_eq!(
            shown(&slow),
            format!(r#"{head}"level":"warning","event":"slow_statement","user":"ANN","millis":2400,"statement":"SELECT 'a\"b'\n\u0001"}}"#)
        );
        let untold = Event::SlowStatement { user: "ANN".into(), millis: 2400, statement: None };
        assert_eq!(
            shown(&untold),
            format!(r#"{head}"level":"warning","event":"slow_statement","user":"ANN","millis":2400,"statement":null}}"#)
        );
        let reopened = Event::FilesReopened { reopened: 3000, limit: 1024 };
        assert_eq!(shown(&reopened), format!(r#"{head}"level":"warning","event":"files_reopened","reopened":3000,"limit":1024}}"#));
        let report = RecoveryReport { records: 1, definitions: 2, committed: 3, not_committed: 4, redone: 5, undone: 6 };
        let cases = [
            (Event::Opened(Opened::Created), r#""level":"info","event":"opened","how":"created"}"#.to_owned()),
            (Event::Opened(Opened::Reopened), r#""level":"info","event":"opened","how":"reopened"}"#.to_owned()),
            (
                Event::Opened(Opened::Recovered(report)),
                r#""level":"warning","event":"opened","how":"recovered","records":1,"definitions":2,"committed":3,"not_committed":4,"redone":5,"undone":6}"#.to_owned(),
            ),
            (Event::Closed, r#""level":"info","event":"closed"}"#.to_owned()),
            (Event::CheckpointTaken(7), r#""level":"info","event":"checkpoint_taken","taken":7}"#.to_owned()),
            (
                Event::SettingChanged { name: "WORK_MEMORY".into(), value: Some(Value::Char("x".into())) },
                r#""level":"info","event":"setting_changed","name":"WORK_MEMORY","value":"'x'"}"#.to_owned(),
            ),
            (
                Event::SettingChanged { name: "WORK_MEMORY".into(), value: None },
                r#""level":"info","event":"setting_changed","name":"WORK_MEMORY","value":null}"#.to_owned(),
            ),
            (Event::UserChanged { name: "ANN".into(), dropped: true }, r#""level":"info","event":"user_changed","name":"ANN","dropped":true}"#.to_owned()),
            (Event::UserChanged { name: "ANN".into(), dropped: false }, r#""level":"info","event":"user_changed","name":"ANN","dropped":false}"#.to_owned()),
            (Event::OwnerChanged { name: "ANN".into() }, r#""level":"info","event":"owner_changed","name":"ANN"}"#.to_owned()),
            (
                Event::Registered { kind: Extension::Encoding, name: "LZ4".into() },
                r#""level":"info","event":"registered","kind":"Encoding","name":"LZ4"}"#.to_owned(),
            ),
        ];
        for (event, rest) in cases {
            assert_eq!(shown(&event), format!("{head}{rest}"));
        }
        let failed = shown(&Event::Failed(SqlError::new(Code::Io, "[DESIGN] the disk is full\t")));
        assert!(
            failed.starts_with(&format!(r#"{head}"level":"error","event":"failed","sqlcode":-915,"sqlstate":"TB915","message":""#)),
            "{failed}"
        );
        assert!(failed.ends_with(r#"the disk is full\t"}"#), "{failed}");
    }

    /// Verifies: [DESIGN] RULES.md D40, D56: a sink writes each event as a
    /// line, and one appending to a file adds to what it holds, each line
    /// counted a write of the file it notes.
    #[test]
    fn a_sink_appends_its_lines() {
        let scratch = turborel_test_support::Scratch::new("server-log");
        let path = scratch.path("log");
        std::fs::write(&path, "kept\n").unwrap();
        let registry = std::sync::Arc::new(turborel::files::Registry::default());
        let sink = Lines::appending(&path).unwrap().noting(registry.note(&path, true));
        sink.event(&Event::Closed);
        sink.event(&Event::CheckpointTaken(1));
        let shown = registry.infos();
        assert_eq!(shown.iter().map(|f| (f.noted, f.open, f.writes)).collect::<Vec<_>>(), [(true, true, 2)]);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert_eq!(lines[0], "kept");
        assert!(lines[1].ends_with(r#""event":"closed"}"#) && lines[2].contains(r#""taken":1"#), "{text}");
        assert!(Lines::appending(&scratch.path("no/such/dir/log")).is_err());
    }

    /// Verifies: [DESIGN] RULES.md D56: on Unix, a diagnostics log the sink
    /// makes is its user's alone (0600).
    #[cfg(unix)]
    #[test]
    fn a_log_made_is_its_users_alone() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = turborel_test_support::Scratch::new("server-log-mode");
        let path = scratch.path("log");
        drop(Lines::appending(&path).unwrap());
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
