use super::*;

fn typed(editor: &mut Editor<'_>, text: &str) {
    for c in text.chars() {
        editor.key(Key::Char(c));
    }
}

/// Verifies: [DESIGN] RULES.md D211: keys edit the line; Enter gives it;
/// Ctrl-D ends input only on an empty line, else deletes; Ctrl-C throws
/// the line away; Tab asks for completion.
#[test]
fn keys_edit_and_end_the_line() {
    let history = History::default();
    let mut editor = Editor::new(&history);
    assert_eq!(editor.key(Key::EndOfInput), Action::End);
    typed(&mut editor, "selet");
    editor.key(Key::Left);
    editor.key(Key::Char('c'));
    assert_eq!(editor.buffer().text(), "select");
    editor.key(Key::Home);
    editor.key(Key::EndOfInput);
    assert_eq!(editor.buffer().text(), "elect");
    assert_eq!(editor.key(Key::Tab), Action::Complete);
    assert_eq!(editor.key(Key::Enter), Action::Submit("elect".into()));
    assert_eq!(editor.key(Key::Interrupt), Action::Submit(String::new()));
    assert!(editor.buffer().is_empty());
}

/// Verifies: [DESIGN] RULES.md D211: Up and Down walk the history, the
/// line typed kept at its end; an entry of several lines is one.
#[test]
fn up_and_down_walk_the_history() {
    let mut history = History::default();
    history.add("SELECT 1\nFROM T", 10);
    history.add("VALUES 2", 10);
    let mut editor = Editor::new(&history);
    typed(&mut editor, "dr");
    editor.key(Key::Up);
    assert_eq!(editor.buffer().text(), "VALUES 2");
    editor.key(Key::Up);
    editor.key(Key::Up);
    assert_eq!(editor.buffer().text(), "SELECT 1\nFROM T");
    editor.key(Key::Down);
    editor.key(Key::Down);
    assert_eq!(editor.buffer().text(), "dr");
}

/// Verifies: [DESIGN] RULES.md D211: Ctrl-R searches backwards as the
/// query is typed, again for an older entry; a key that is not of the
/// search takes the entry found; Escape leaves the line as it was.
#[test]
fn ctrl_r_searches_backwards() {
    let mut history = History::default();
    for entry in ["select 1", "insert", "select 2"] {
        history.add(entry, 10);
    }
    let mut editor = Editor::new(&history);
    editor.key(Key::Search);
    typed(&mut editor, "sel");
    assert!(editor.render("> ", 80).contains("(search) 'sel': select 2"));
    editor.key(Key::Search);
    assert_eq!(editor.key(Key::Enter), Action::Submit("select 1".into()));
    let mut other = Editor::new(&history);
    typed(&mut other, "x");
    other.key(Key::Search);
    typed(&mut other, "ins");
    other.key(Key::Backspace);
    other.key(Key::Cancel);
    assert_eq!(other.buffer().text(), "x");
    other.key(Key::Search);
    typed(&mut other, "ins");
    other.key(Key::End);
    assert_eq!(other.buffer().text(), "insert");
}

/// Verifies: [DESIGN] RULES.md D211: one candidate takes the word's place;
/// several add their common start, or are listed if it adds nothing.
#[test]
fn completions_are_applied() {
    let history = History::default();
    let mut editor = Editor::new(&history);
    typed(&mut editor, "SELECT * FROM st");
    let one = Completion { start: 14, candidates: vec!["STAFF".into()] };
    assert_eq!(editor.complete(&one), None);
    assert_eq!(editor.buffer().text(), "SELECT * FROM STAFF");
    let several = Completion { start: 14, candidates: vec!["STAFF".into(), "STAFFING".into()] };
    assert_eq!(editor.complete(&several), Some(vec!["STAFF".to_owned(), "STAFFING".to_owned()]));
    let mut fresh = Editor::new(&history);
    typed(&mut fresh, "a");
    let grows = Completion { start: 0, candidates: vec!["ABC".into(), "ABD".into()] };
    assert_eq!(fresh.complete(&grows), None);
    assert_eq!(fresh.buffer().text(), "AB");
    assert_eq!(fresh.complete(&Completion::default()), None);
}

/// Verifies: [DESIGN] RULES.md D211: the line is drawn from its first row,
/// line breaks as spaces, the cursor put back where it is, on a narrow
/// screen too.
#[test]
fn lines_are_drawn_with_the_cursor_in_place() {
    let history = History::default();
    let mut editor = Editor::new(&history);
    typed(&mut editor, "ab\ncd");
    editor.key(Key::Left);
    assert_eq!(editor.render("> ", 80), "\r\x1b[J> ab cd\r\x1b[6C");
    assert_eq!(editor.render("> ", 4), "\r\x1b[J> ab cd\r\x1b[2C");
    assert_eq!(editor.render("> ", 4), "\x1b[1A\r\x1b[J> ab cd\r\x1b[2C");
    assert_eq!(editor.leave(4), "\r\n");
    let mut exact = Editor::new(&history);
    typed(&mut exact, "ab");
    assert_eq!(exact.render("> ", 4), "\r\x1b[J> ab\r\n\r");
}
