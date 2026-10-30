//! Unit tests of the sink: its lines written by a thread of its own,
//! never waited for, and its file opened again (RULES.md R-TEST-1).

use std::io::Write;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use turborel::diagnostics::{Diagnostics, Event};

use super::{Lines, Target};

/// A stream whose writes are kept, each held until the test lets it go
/// once `held` is set: the first write tells `entered`, then waits.
#[derive(Clone)]
struct Gate {
    kept: Arc<Mutex<Vec<u8>>>,
    entered: Sender<()>,
    open: Arc<Mutex<Option<Receiver<()>>>>,
}

impl Write for Gate {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let poisoned = |_| std::io::Error::other("poisoned");
        let gate = self.open.lock().map_err(poisoned)?.take();
        if let Some(open) = gate {
            self.entered.send(()).map_err(std::io::Error::other)?;
            open.recv().map_err(std::io::Error::other)?;
        }
        self.kept.lock().map_err(|_| std::io::Error::other("poisoned"))?.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Whether `done` comes true within ten seconds, the sink's thread
/// writing meanwhile.
fn eventually(done: impl Fn() -> bool) -> bool {
    (0..10_000_u32).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(1));
        done()
    })
}

/// Verifies: [DESIGN] RULES.md D40, D56: a sink writes each event as a
/// line, and one appending to a file adds to what it holds, each line
/// counted a write of the file it notes; every line queued written by the
/// time it is dropped.
#[test]
fn a_sink_appends_its_lines() {
    let scratch = turborel_test_support::Scratch::new("server-log");
    let path = scratch.path("log");
    std::fs::write(&path, "kept\n").unwrap();
    let registry = Arc::new(turborel::files::Registry::default());
    let sink = Lines::appending(&path).unwrap().noting(registry.note(&path, true));
    sink.event(&Event::Closed);
    sink.event(&Event::CheckpointTaken(1));
    // Written by the sink's thread, in its time.
    let counted = || registry.infos().iter().map(|f| (f.noted, f.open, f.writes)).collect::<Vec<_>>();
    assert!(eventually(|| counted() == [(true, true, 2)]), "{:?}", counted());
    drop(sink);
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert_eq!(lines[0], "kept");
    assert!(lines[1].ends_with(r#""event":"closed"}"#) && lines[2].contains(r#""taken":1"#), "{text}");
    assert!(Lines::appending(&scratch.path("no/such/dir/log")).is_err());
}

/// Verifies: [DESIGN] RULES.md D40: a sink returns soon, its stream
/// blocked or not: while the writer waits on its stream, lines queue up to
/// the bound, and those past it are dropped, not waited for; counted, and
/// told once, a line of their own, after the next line written.
#[test]
fn a_blocked_stream_holds_up_no_event() {
    let (entered, waiting) = channel();
    let (release, open) = channel();
    let gate = Gate { kept: Arc::default(), entered, open: Arc::new(Mutex::new(Some(open))) };
    let kept = Arc::clone(&gate.kept);
    let sink = Lines::started(Target::Stream(Box::new(gate)), 4).unwrap();
    sink.event(&Event::CheckpointTaken(0));
    waiting.recv().unwrap();
    // The writer holds the first; four wait; three are dropped.
    for n in 1..=7_u64 {
        sink.event(&Event::CheckpointTaken(n));
    }
    release.send(()).unwrap();
    drop(sink);
    let text = String::from_utf8(kept.lock().unwrap().clone()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 6, "{text}");
    assert!(lines[0].ends_with(r#""taken":0}"#), "{text}");
    assert!(lines[1].ends_with(r#""level":"warning","event":"lines_dropped","lines":3}"#), "{text}");
    for (n, line) in (1_u64..).zip(lines.iter().skip(2)) {
        assert!(line.ends_with(&format!(r#""taken":{n}}}"#)), "{text}");
    }
}

/// Verifies: [DESIGN] RULES.md D40: a sink told to open its file again,
/// as a rotation does once it has moved the file away, writes the lines
/// told before to the file moved, and those after to a new one at its
/// path; one that cannot be opened again, its directory gone, keeps the
/// file it has and says so in it. A sink to a stream has nothing to open
/// again.
#[test]
fn a_log_rotated_is_opened_again() {
    let scratch = turborel_test_support::Scratch::new("server-log-reopen");
    std::fs::create_dir_all(scratch.path("d")).unwrap();
    let path = scratch.path("d/log");
    let sink = Lines::appending(&path).unwrap();
    let reopener = sink.reopener();
    sink.event(&Event::CheckpointTaken(1));
    std::fs::rename(&path, scratch.path("d/log.1")).unwrap();
    reopener.reopen();
    sink.event(&Event::CheckpointTaken(2));
    // Written by the sink's thread, in its time, before the directory goes.
    assert!(eventually(|| std::fs::read_to_string(&path).is_ok_and(|text| text.contains(r#""taken":2"#))));
    std::fs::rename(scratch.path("d"), scratch.path("e")).unwrap();
    reopener.reopen();
    sink.event(&Event::CheckpointTaken(3));
    drop(sink);
    let rotated = std::fs::read_to_string(scratch.path("e/log.1")).unwrap();
    assert!(rotated.contains(r#""taken":1"#) && !rotated.contains(r#""taken":2"#), "{rotated}");
    let moved = std::fs::read_to_string(scratch.path("e/log")).unwrap();
    let lines: Vec<&str> = moved.lines().collect();
    assert_eq!(lines.len(), 3, "{moved}");
    assert!(lines[0].contains(r#""taken":2"#), "{moved}");
    assert!(lines[1].contains(r#""level":"error","event":"log_not_reopened","path":""#), "{moved}");
    assert!(lines[2].contains(r#""taken":3"#), "{moved}");
    let stream = Lines::new(Box::new(std::io::sink())).unwrap();
    stream.reopener().reopen();
    stream.event(&Event::Closed);
}

/// Verifies: [DESIGN] RULES.md D40: a reopening asked while the queue is
/// full is not lost: it is done before the next line written.
#[test]
fn a_reopening_asked_with_the_queue_full_is_done() {
    let (entered, waiting) = channel();
    let (release, open) = channel();
    let gate = Gate { kept: Arc::default(), entered, open: Arc::new(Mutex::new(Some(open))) };
    let sink = Lines::started(Target::Stream(Box::new(gate)), 1).unwrap();
    sink.event(&Event::Closed);
    waiting.recv().unwrap();
    sink.event(&Event::Closed);
    sink.reopener().reopen();
    assert!(sink.shared.reopen.load(std::sync::atomic::Ordering::Acquire), "flagged, the queue full");
    let shared = Arc::clone(&sink.shared);
    release.send(()).unwrap();
    drop(sink);
    assert!(!shared.reopen.load(std::sync::atomic::Ordering::Acquire), "done");
}

/// Verifies: [DESIGN] RULES.md D56: on Unix, a diagnostics log the sink
/// makes is its user's alone (0600), and so is one it opens again.
#[cfg(unix)]
#[test]
fn a_log_made_is_its_users_alone() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = turborel_test_support::Scratch::new("server-log-mode");
    let path = scratch.path("log");
    let sink = Lines::appending(&path).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    std::fs::remove_file(&path).unwrap();
    sink.reopener().reopen();
    sink.event(&Event::Closed);
    drop(sink);
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
}
