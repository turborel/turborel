use super::*;

fn keys(bytes: &[u8]) -> Vec<Key> {
    let mut decoder = Decoder::default();
    bytes.iter().filter_map(|b| decoder.feed(*b)).collect()
}

/// Verifies: [DESIGN] RULES.md D211: bytes decode to keys: characters,
/// UTF-8 among them, control keys, and the ANSI escape sequences of the
/// arrows, Home, End, Delete and moves by word.
#[test]
fn bytes_decode_to_keys() {
    assert_eq!(keys("aé€\r".as_bytes()), [Key::Char('a'), Key::Char('é'), Key::Char('€'), Key::Enter]);
    assert_eq!(keys(&[0x01, 0x05, 0x12, 0x03, 0x04, 0x7f, b'\t', 0x15, 0x0b, 0x17, 0x0c, 0x07, 0x00]), [
        Key::Home,
        Key::End,
        Key::Search,
        Key::Interrupt,
        Key::EndOfInput,
        Key::Backspace,
        Key::Tab,
        Key::CutStart,
        Key::CutEnd,
        Key::CutWord,
        Key::Redraw,
        Key::Cancel,
        Key::Other
    ]);
    assert_eq!(keys(b"\x1b[A\x1b[B\x1b[C\x1b[D\x1bOH\x1b[F\x1b[3~"), [Key::Up, Key::Down, Key::Right, Key::Left, Key::Home, Key::End, Key::Delete]);
    assert_eq!(keys(b"\x1b[1;5C\x1b[1;3D\x1bb\x1bf\x1b[1~\x1b[4~\x1b[9z"), [Key::WordRight, Key::WordLeft, Key::WordLeft, Key::WordRight, Key::Home, Key::End, Key::Other]);
    assert_eq!(keys(&[0xff]), [Key::Other]);
    let mut decoder = Decoder::default();
    assert_eq!(decoder.feed(0x1b), None);
    assert_eq!(decoder.flush(), Some(Key::Cancel));
    assert_eq!(decoder.flush(), None);
}

/// Verifies: [DESIGN] RULES.md D211: the buffer inserts and deletes at its
/// cursor, moves by characters and words, and cuts.
#[test]
fn the_buffer_edits_at_its_cursor() {
    let mut buffer = Buffer::of("select a_b, c");
    buffer.word_left();
    assert_eq!(buffer.cursor(), 12);
    buffer.word_left();
    assert_eq!(buffer.cursor(), 7);
    buffer.word_right();
    assert_eq!(buffer.cursor(), 10);
    buffer.cut_word();
    assert_eq!((buffer.text().as_str(), buffer.cursor()), ("select , c", 7));
    buffer.home();
    buffer.delete();
    buffer.right();
    buffer.backspace();
    assert_eq!(buffer.text(), "lect , c");
    buffer.end();
    buffer.right();
    buffer.left();
    buffer.cut_end();
    assert_eq!(buffer.text(), "lect , ");
    buffer.cut_start();
    assert!(buffer.is_empty());
    buffer.backspace();
    buffer.insert('x');
    buffer.replace_before(0, "abc");
    assert_eq!((buffer.before().as_str(), buffer.cursor()), ("abc", 3));
}
