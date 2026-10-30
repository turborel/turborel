//! \[DESIGN\] An event as a line of JSON (RULES.md D40, D42): its time, in
//! UTC to the millisecond (RFC 3339), its level and its kind, then its
//! fields; and the lines the sink writes of itself, of lines it dropped
//! and of a log it could not open again. Split from log.rs (RULES.md
//! R-SIZE-2).

use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use turborel::db::Opened;
use turborel::diagnostics::{Event, Level, Overdue};
use turborel::error::SqlError;
use turborel::threads::ThreadChange;

/// `event`, taken at `at`, as a line of JSON.
pub fn line(at: SystemTime, event: &Event) -> String {
    let mut json = Json::begun(at, event.level());
    fields(&mut json, event);
    json.done()
}

/// \[DESIGN\] The sink's own line, at `at`: `lines` lines dropped, its
/// queue full, since it last said so.
pub(super) fn dropped(at: SystemTime, lines: u64) -> String {
    let mut json = Json::begun(at, Level::Warning);
    json.text("event", "lines_dropped");
    json.number("lines", lines);
    json.done()
}

/// \[DESIGN\] The sink's own line, at `at`: the log at `path` could not be
/// opened again, for `why`, so lines go on to the file it had.
pub(super) fn not_reopened(at: SystemTime, path: &str, why: &str) -> String {
    let mut json = Json::begun(at, Level::Error);
    json.text("event", "log_not_reopened");
    json.text("path", path);
    json.text("message", why);
    json.done()
}

fn level(level: Level) -> &'static str {
    match level {
        Level::Error => "error",
        Level::Warning => "warning",
        Level::Info => "info",
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
        Event::OwnerChanged { name } => json.texts(&[("event", "owner_changed"), ("name", name)]),
        Event::Registered { kind, name } => json.texts(&[("event", "registered"), ("kind", &format!("{kind:?}")), ("name", name)]),
        Event::Failed(e) => failed(json, e),
        Event::Thread { label, change } => thread(json, label, change),
        Event::SlowStatement { user, millis, statement } => slow(json, user, *millis, statement.as_deref()),
        Event::FilesReopened { reopened, limit } => counts(json, "files_reopened", &[("reopened", *reopened), ("limit", *limit)]),
        Event::CheckpointOverdue { changes, cause } => overdue(json, *changes, cause),
        Event::Listening { addresses } => listening(json, addresses),
        Event::Stopping { connections } => counts(json, "stopping", &[("connections", *connections)]),
        Event::StopHurried { sessions } => counts(json, "stop_hurried", &[("sessions", *sessions)]),
        Event::AcceptFailed { failures, message } => {
            counts(json, "accept_failed", &[("failures", *failures)]);
            json.text("message", message);
        }
        Event::TlsFailed { peer, message } => json.texts(&[("event", "tls_failed"), ("peer", peer), ("message", message)]),
        Event::LoginFailed { user, peer, mechanism, message } => {
            json.texts(&[("event", "login_failed"), ("user", user), ("peer", peer), ("mechanism", mechanism), ("message", message)]);
        }
        Event::ConnectionsRefused { refused, limit } => counts(json, "connections_refused", &[("refused", *refused), ("limit", *limit)]),
        // An event of a later database than this sink knows: kept whole.
        other => json.texts(&[("event", "other"), ("debug", &format!("{other:?}"))]),
    }
}

/// An event of kind `event` whose fields are the counts `named`.
fn counts(json: &mut Json, event: &str, named: &[(&str, u64)]) {
    json.text("event", event);
    for (name, n) in named {
        json.number(name, *n);
    }
}

/// A failure's fields: its SQLCODE, SQLSTATE and message.
fn failed(json: &mut Json, e: &SqlError) {
    json.text("event", "failed");
    json.raw("sqlcode", &e.sqlcode().to_string());
    json.text("sqlstate", e.sqlstate().as_str());
    json.text("message", &e.to_string());
}

/// Where a server listens: its addresses, as an array.
fn listening(json: &mut Json, addresses: &[String]) {
    json.text("event", "listening");
    let mut array = String::from("[");
    for (n, address) in addresses.iter().enumerate() {
        if n > 0 {
            array.push(',');
        }
        quoted(&mut array, address);
    }
    array.push(']');
    json.raw("addresses", &array);
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

/// The fields of checkpoints overdue, the log keeping `changes` row
/// changes, and why.
fn overdue(json: &mut Json, changes: u64, cause: &Overdue) {
    json.text("event", "checkpoint_overdue");
    json.number("changes", changes);
    match cause {
        Overdue::Refused(e) => {
            json.text("cause", "refused");
            json.raw("sqlcode", &e.sqlcode().to_string());
            json.text("message", &e.to_string());
        }
        Overdue::Transaction(age) => {
            json.text("cause", "transaction");
            json.number("seconds", age.as_secs());
        }
        Overdue::NotTaken(policy) => {
            json.text("cause", "not_taken");
            json.text("policy", policy.word());
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
            json.number("cut", report.cut);
        }
    }
}

/// A JSON object, written a field at a time.
#[derive(Default)]
struct Json(String);

impl Json {
    /// An object of a line taken at `at`, of `of`, its time and level
    /// written.
    fn begun(at: SystemTime, of: Level) -> Json {
        let mut json = Json::default();
        json.text("time", &timestamp(at));
        json.text("level", level(of));
        json
    }

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

    /// Text fields, in order.
    fn texts(&mut self, named: &[(&str, &str)]) {
        for (name, value) in named {
            self.text(name, value);
        }
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
        let report = RecoveryReport { records: 1, definitions: 2, committed: 3, not_committed: 4, redone: 5, undone: 6, cut: 7 };
        let cases = [
            (Event::Opened(Opened::Created), r#""level":"info","event":"opened","how":"created"}"#.to_owned()),
            (Event::Opened(Opened::Reopened), r#""level":"info","event":"opened","how":"reopened"}"#.to_owned()),
            (
                Event::Opened(Opened::Recovered(report)),
                r#""level":"warning","event":"opened","how":"recovered","records":1,"definitions":2,"committed":3,"not_committed":4,"redone":5,"undone":6,"cut":7}"#.to_owned(),
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

    /// Verifies: [DESIGN] RULES.md D25, D40: checkpoints overdue are a
    /// warning, with the row changes the log keeps and the cause: refused
    /// with an error, an open transaction and its age in seconds, or none
    /// taken under the job policy named.
    #[test]
    fn checkpoints_overdue_are_lines_of_json() {
        use turborel::diagnostics::Overdue;
        let shown = |cause: Overdue| line(at(0, 0), &Event::CheckpointOverdue { changes: 400, cause });
        let head = r#"{"time":"1970-01-01T00:00:00.000Z","level":"warning","event":"checkpoint_overdue","changes":400,"#;
        let damaged = SqlError::new(Code::Damaged, "T is damaged");
        let refused = shown(Overdue::Refused(damaged));
        assert!(refused.starts_with(&format!(r#"{head}"cause":"refused","sqlcode":-914,"message":""#)), "{refused}");
        assert_eq!(shown(Overdue::Transaction(Duration::from_millis(90_500))), format!(r#"{head}"cause":"transaction","seconds":90}}"#));
        let policy = turborel::threads::JobPolicy::Paused;
        assert_eq!(shown(Overdue::NotTaken(policy)), format!(r#"{head}"cause":"not_taken","policy":"PAUSED"}}"#));
    }

    /// Verifies: [DESIGN] RULES.md D40, D42, D43: a server's own events are
    /// lines of JSON with their fields: where it listens, as an array; its
    /// stopping, and stopping hurried; accepts that failed, TLS and logins
    /// that failed, with no password; connections refused; and the sink's
    /// own lines, of lines dropped and of a log not opened again.
    #[test]
    fn a_servers_events_are_lines_of_json() {
        let shown = |event: &Event| line(at(0, 0), event);
        let head = r#"{"time":"1970-01-01T00:00:00.000Z","#;
        let cases = [
            (
                Event::Listening { addresses: vec!["127.0.0.1:7422".into(), "/tmp/\"s".into()] },
                r#""level":"info","event":"listening","addresses":["127.0.0.1:7422","/tmp/\"s"]}"#,
            ),
            (Event::Listening { addresses: vec![] }, r#""level":"info","event":"listening","addresses":[]}"#),
            (Event::Stopping { connections: 3 }, r#""level":"info","event":"stopping","connections":3}"#),
            (Event::StopHurried { sessions: 2 }, r#""level":"warning","event":"stop_hurried","sessions":2}"#),
            (
                Event::AcceptFailed { failures: 4, message: "too many open files".into() },
                r#""level":"error","event":"accept_failed","failures":4,"message":"too many open files"}"#,
            ),
            (
                Event::TlsFailed { peer: "10.0.0.1:5000".into(), message: "bad record".into() },
                r#""level":"warning","event":"tls_failed","peer":"10.0.0.1:5000","message":"bad record"}"#,
            ),
            (
                Event::LoginFailed { user: "ANN".into(), peer: "local".into(), mechanism: "password".into(), message: "refused".into() },
                r#""level":"warning","event":"login_failed","user":"ANN","peer":"local","mechanism":"password","message":"refused"}"#,
            ),
            (
                Event::ConnectionsRefused { refused: 9, limit: 256 },
                r#""level":"warning","event":"connections_refused","refused":9,"limit":256}"#,
            ),
        ];
        for (event, rest) in cases {
            assert_eq!(shown(&event), format!("{head}{rest}"));
        }
        assert_eq!(dropped(at(0, 0), 5), format!(r#"{head}"level":"warning","event":"lines_dropped","lines":5}}"#));
        assert_eq!(
            not_reopened(at(0, 0), "/var/log/t", "denied"),
            format!(r#"{head}"level":"error","event":"log_not_reopened","path":"/var/log/t","message":"denied"}}"#)
        );
    }
}
