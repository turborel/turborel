//! [DESIGN] RULES.md D211: a line edited from the bytes raw mode gives,
//! read from memory, as the terminal would give them.

use turborel_test_support::Scratch;

use super::*;

/// A line typed as `bytes` after `turborel> `, on 40 columns, with
/// `history`, completed by `complete`: the line, and what was drawn.
fn typed(bytes: &[u8], history: &History, complete: &mut dyn FnMut(&str) -> Completion) -> (Option<String>, String) {
    let mut out = Vec::new();
    let typed = Typed { prompt: "turborel> ", width: 40, history };
    let line = edit_line(&typed, &mut &bytes[..], &mut out, complete).unwrap();
    (line, String::from_utf8(out).unwrap())
}

/// Completes nothing.
fn nothing(_: &str) -> Completion {
    Completion::default()
}

/// Verifies: [DESIGN] RULES.md D211: Enter submits the line typed, the
/// prompt drawn first; Ctrl-D on an empty line ends the input, the
/// cursor moved to a new line; the input's end before Enter ends it too;
/// Ctrl-L clears the screen and draws the line again.
#[test]
fn keys_submit_end_and_redraw() {
    let history = History::default();
    let (line, drawn) = typed(b"VALUES 1\r", &history, &mut nothing);
    assert_eq!(line.as_deref(), Some("VALUES 1"));
    assert!(drawn.contains("turborel> "), "{drawn:?}");
    let (ended, left) = typed(&[0x04], &history, &mut nothing);
    assert_eq!(ended, None);
    assert!(left.ends_with("\r\n"), "{left:?}");
    assert_eq!(typed(b"VAL", &history, &mut nothing).0, None);
    let (redrawn, cleared) = typed(b"V\x0c\r", &history, &mut nothing);
    assert_eq!(redrawn.as_deref(), Some("V"));
    assert!(cleared.contains("\x1b[H\x1b[2J"), "{cleared:?}");
}

/// Verifies: [DESIGN] RULES.md D211: Tab asks for the completions of the
/// text before the cursor: one is put in place of the word, several are
/// listed below the line, which is drawn again; Up recalls the history's
/// last entry.
#[test]
fn tab_completes_and_up_recalls() {
    let mut history = History::default();
    history.add("VALUES 7", 10);
    let mut asked = Vec::new();
    let mut one = |before: &str| {
        asked.push(before.to_owned());
        Completion { start: 0, candidates: vec!["SELECT".to_owned()] }
    };
    let (line, _) = typed(b"SEL\t\r", &history, &mut one);
    assert_eq!(asked, ["SEL"]);
    assert!(line.as_deref().is_some_and(|l| l.starts_with("SELECT")), "{line:?}");
    let mut two = |_: &str| Completion { start: 0, candidates: vec!["SELECT".to_owned(), "SET".to_owned()] };
    let (_, drawn) = typed(b"SE\t\t\r", &history, &mut two);
    assert!(drawn.contains("SELECT  SET\r\n"), "{drawn:?}");
    let (recalled, _) = typed(b"\x1b[A\r", &history, &mut nothing);
    assert_eq!(recalled.as_deref(), Some("VALUES 7"));
}

/// Input that cannot be read.
struct Broken;

impl Read for Broken {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("gone"))
    }
}

/// Verifies: [DESIGN] RULES.md D211: a terminal that cannot be read is
/// an error, not a line; an entry remembered is added to the history and
/// saved to its file, and a file that cannot be written leaves it kept
/// for the session alone.
#[test]
fn unreadable_input_fails_and_entries_are_remembered() {
    let history = History::default();
    let typed = Typed { prompt: "> ", width: 40, history: &history };
    assert!(edit_line(&typed, &mut Broken, &mut Vec::new(), &mut nothing).is_err());
    let scratch = Scratch::new("cli-line-remember");
    let file = scratch.path("history");
    let mut editor = LineEditor { history: History::default() };
    editor.remember("VALUES 1", 5, Some(&file));
    editor.remember("VALUES 2", 5, None);
    assert_eq!(History::load(&file).entries(), ["VALUES 1"]);
    editor.remember("VALUES 3", 5, Some(&scratch.path("no/such/dir/history")));
    assert_eq!(editor.history.entries(), ["VALUES 1", "VALUES 2", "VALUES 3"]);
}
