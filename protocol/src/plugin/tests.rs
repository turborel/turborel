//! Unit tests of the plug-in SDK, each call made through `Harness` as the
//! database makes it (RULES.md R-TEST-1): batches answered object by
//! object, errors kept by their SQLSTATE, cancellation, the answer's
//! limit, SQL run through the link, the paging the SDK does, and the
//! frames `serve_on` reads and writes.

use std::collections::BTreeMap;

use turborel::bytes::Put;
use turborel::plugin::payload::{self as p, engine, process, system};
use turborel::plugin::wire::{Call, Hello, Message, read_frame, rows, write_message};
use turborel::storage::{Row, RowId};
use turborel::value::{Decimal, Value};

use super::*;

/// A scalar of the tests: twice an integer, or a long while until
/// cancelled, then a string repeated.
struct Twice;

impl Scalar for Twice {
    fn call(&mut self, cx: &mut Cx<'_>, args: &[Value]) -> Answer<Value> {
        match args.first() {
            Some(Value::Exact(d)) => Ok(Value::Exact(d.checked_mul(Decimal::new(2, 0))?)),
            Some(Value::Char(s)) => {
                if s == "forever" {
                    for _ in 0..1_000_000_u32 {
                        cx.check()?;
                    }
                }
                Ok(Value::Char(s.repeat(1000)))
            }
            _ => Err(PluginError::data("not an integer")),
        }
    }
}

/// A collation of the tests: lower case.
struct Lower;

impl Collation for Lower {
    fn sort_key(&mut self, text: &str, pad: bool) -> Answer<Vec<u8>> {
        Ok(if pad { text.trim_end() } else { text }.to_lowercase().into_bytes())
    }
}

/// A system table of the tests: `n` rows, the first its reader's name.
struct Rows(i128);

impl SystemTable for Rows {
    fn rows(&mut self, user: &str, _owner: bool) -> Answer<Vec<Vec<Value>>> {
        let mut all = vec![vec![Value::Char(user.to_owned())]];
        all.extend((1..self.0).map(|i| vec![Value::Exact(Decimal::new(i, 0))]));
        Ok(all)
    }
}

/// A procedure of the tests: its argument inserted by SQL.
struct Note;

impl Procedure for Note {
    fn call(&mut self, cx: &mut Cx<'_>, args: &[Value]) -> Answer<()> {
        cx.run("INSERT INTO NOTES VALUES (?)", args)
    }
}

/// An engine of the tests, its one table in memory.
#[derive(Default)]
struct Memory(BTreeMap<RowId, Row>);

impl Engine for Memory {
    fn create(&mut self, table: &str, _columns: &[String]) -> Answer<Vec<u8>> {
        Ok(table.as_bytes().to_vec())
    }
    fn open(&mut self, _locator: &[u8]) -> Answer<Opened> {
        Ok(Opened { found: true, next: 1, rows: 0, durable: 0 })
    }
    fn scan(&mut self, _locator: &[u8], after: RowId, max: u32) -> Answer<(Vec<(RowId, Row)>, bool)> {
        let mut all = self.0.range(after.saturating_add(1)..).map(|(id, r)| (*id, r.clone()));
        let page = all.by_ref().take(usize::try_from(max).unwrap_or(usize::MAX)).collect();
        Ok((page, all.next().is_some()))
    }
    fn get(&mut self, _locator: &[u8], ids: &[RowId]) -> Answer<Vec<Option<Row>>> {
        Ok(ids.iter().map(|id| self.0.get(id).cloned()).collect())
    }
    fn apply(&mut self, _locator: &[u8], changes: &[(RowId, Option<Row>)]) -> Answer<Vec<Option<Row>>> {
        Ok(changes
            .iter()
            .map(|(id, row)| match row {
                Some(row) => self.0.insert(*id, row.clone()),
                None => self.0.remove(id),
            })
            .collect())
    }
    fn checkpoint(&mut self, _locator: &[u8], _through: u64) -> Answer<()> {
        Ok(())
    }
    fn drop_table(&mut self, _locator: &[u8]) -> Answer<()> {
        Ok(())
    }
}

/// Credentials of the tests: the user and the password.
struct Plain;

impl Credentials for Plain {
    fn verifier(&mut self, user: &str, password: &str) -> Answer<Vec<u8>> {
        Ok(format!("{user}:{password}").into_bytes())
    }
}

/// An integer.
fn exact(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The plug-in of the tests.
fn plugin() -> Plugin {
    Plugin::new("1.0.0")
        .scalar("TWICE", Twice)
        .collation("LOWER", Lower)
        .system_table("ROWS", Rows(2500))
        .procedure("NOTE", Note)
        .engine("MEMORY", Memory::default())
        .credentials("PLAIN", Plain)
}

/// Verifies: [EXTENSION] RULES.md D167, D169: WELCOME gives the kinds the
/// plug-in serves; a scalar's batch is answered row by row; a collation's
/// keys are asked together; an object no kind has, or an error, is an
/// ERROR of its SQLSTATE.
#[test]
fn calls_are_answered_by_their_objects() {
    let mut h = Harness::new(plugin());
    assert_eq!(h.kinds("T"), [1, 4, 12, 13, 16, 18]);
    let rows_in = [vec![exact(1)], vec![Value::Null], vec![exact(21)]];
    assert!(h.scalar("TWICE", &rows_in).is_err(), "a null is the function's to refuse");
    let twice: Vec<String> = h.scalar("TWICE", &[vec![exact(1)], vec![exact(21)]]).unwrap().iter().map(Value::to_string).collect();
    assert_eq!(twice, ["2", "42"]);
    assert_eq!(h.scalar("TWICE", &[vec![Value::Null]]).unwrap_err().state, "22000");
    assert_eq!(h.scalar("NOWHERE", &[vec![exact(1)]]).unwrap_err().state, "38000");
    assert_eq!(h.sort_keys("LOWER", &["B ", "a"], true).unwrap(), [b"b".to_vec(), b"a".to_vec()]);
    let mut asked = Vec::new();
    process::put_verifier(&mut asked, "ANN", "pw");
    let mut verifier = h.call(18, p::CREDENTIALS_VERIFIER, "PLAIN", asked).unwrap();
    assert_eq!(verifier.split_off(4), b"ANN:pw");
}

/// Verifies: [EXTENSION] RULES.md D170: a call that checks its `Cx` is
/// stopped by a CANCEL, SQLSTATE 57014; an answer past the limits HELLO
/// gave is an ERROR of 38T04, not sent.
#[test]
fn cancels_and_limits_are_honoured() {
    let mut h = Harness::new(plugin());
    h.cancel(true);
    assert_eq!(h.scalar("TWICE", &[vec![Value::Char("forever".into())]]).unwrap_err().state, "57014");
    h.cancel(false);
    let limits = Limits { max_message_bytes: 500, max_call_bytes: 0, memory_bytes: 0 };
    let mut small = Harness::new(plugin()).with_limits(limits);
    assert_eq!(small.scalar("TWICE", &[vec![Value::Char("x".into())]]).unwrap_err().state, "38T04");
}

/// Verifies: [EXTENSION] RULES.md D169: a procedure's SQL is sent on the
/// link, in its call's transaction, its values bound.
#[test]
fn sql_goes_through_the_link() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let kept = std::rc::Rc::clone(&seen);
    let mut h = Harness::new(plugin()).with_sql(move |text, args, rows| {
        kept.borrow_mut().push((text.to_owned(), args.len(), rows));
        Ok(Vec::new())
    });
    let mut payload = Vec::new();
    turborel::plugin::wire::put_rows(&mut payload, &[vec![Value::Char("hi".into())]]).unwrap();
    h.call(4, p::PROCEDURE_CALL, "NOTE", payload).unwrap();
    assert_eq!(*seen.borrow(), [("INSERT INTO NOTES VALUES (?)".to_owned(), 1, false)]);
}

/// Verifies: [EXTENSION] RULES.md D222: a system table's rows are asked
/// of it once and given a page at a time, the last saying no more follow.
#[test]
fn system_tables_are_paged() {
    let mut h = Harness::new(plugin());
    let mut given = 0_u64;
    let mut pages = 0_u32;
    loop {
        let mut asked = Vec::new();
        system::put_page(&mut asked, &system::Page { user: "ANN".into(), owner: false, from: given, max: 1024 });
        let got = h.call(13, p::SYSTEM_ROWS, "ROWS", asked).unwrap();
        let mut r = p::reader(&got);
        let page = rows(&mut r).unwrap();
        if given == 0 {
            assert_eq!(page[0][0].to_string(), "ANN");
        }
        given += u64::try_from(page.len()).unwrap();
        pages += 1_u32;
        if r.u8().unwrap() == 0 {
            break;
        }
    }
    assert_eq!((given, pages), (2500_u64, 3_u32));
}

/// Verifies: [EXTENSION] RULES.md D224: an engine's APPLY answers the rows
/// it replaced, GET and SCAN read back what it holds, a page at a time.
#[test]
fn an_engines_operations() {
    let mut h = Harness::new(plugin());
    let mut created = Vec::new();
    created.put_str("T");
    p::put_strings(&mut created, &["INTEGER".to_owned()]);
    let locator = h.call(16, p::ENGINE_CREATE, "MEMORY", created).unwrap();
    let locator = p::reader(&locator).bytes().unwrap().to_vec();
    let changes: Vec<(RowId, Option<Row>)> = (1..=5).map(|i| (i, Some(vec![exact(i128::from(i))]))).collect();
    let mut applied = Vec::new();
    engine::put_changes(&mut applied, &locator, &changes).unwrap();
    let before = h.call(16, p::ENGINE_APPLY, "MEMORY", applied).unwrap();
    assert!(engine::maybes(&mut p::reader(&before)).unwrap().iter().all(Option::is_none));
    let mut asked = Vec::new();
    engine::put_ids(&mut asked, &locator, &[2, 9]);
    let got = engine::maybes(&mut p::reader(&h.call(16, p::ENGINE_GET, "MEMORY", asked).unwrap())).unwrap();
    assert_eq!((got[0].is_some(), got[1].is_none()), (true, true));
    let mut scan = Vec::new();
    engine::put_scan(&mut scan, &locator, 3, 1);
    let page = h.call(16, p::ENGINE_SCAN, "MEMORY", scan).unwrap();
    let mut r = p::reader(&page);
    let ids: Vec<RowId> = engine::identified(&mut r).unwrap().into_iter().map(|(id, _)| id).collect();
    assert_eq!((ids, r.u8().unwrap()), (vec![4], 1));
}

/// The messages `served` holds, frame by frame.
fn messages_of(served: &[u8]) -> Vec<Message> {
    let mut from = served;
    let mut all = Vec::new();
    while let Some(body) = read_frame(&mut from, 1 << 20).unwrap() {
        all.push(Message::decode(&body).unwrap());
    }
    all
}

/// Verifies: [EXTENSION] RULES.md D167, D170: `serve_on` answers HELLO with
/// WELCOME, of protocol 1, the capabilities it takes and its kinds; each
/// CALL with its RESULT, or with an ERROR of 38T04 past the limit HELLO
/// gave; and ends at SHUTDOWN.
#[test]
fn frames_are_served() {
    let hello = Hello {
        protocol: 1,
        extension: "T".into(),
        capabilities: vec!["batch".into(), "cancel".into(), "other".into()],
        max_message_bytes: 200,
        max_call_bytes: 0,
        memory_bytes: 0,
    };
    let mut input = Vec::new();
    let call = |n: u64, text: &str| {
        let mut payload = Vec::new();
        turborel::plugin::wire::put_rows(&mut payload, &[vec![Value::Char(text.into())]]).unwrap();
        Message::Call(Call { call: n, kind: 1, op: p::SCALAR_CALL, object: "TWICE".into(), payload })
    };
    for m in [Message::Hello(hello), call(1, ""), call(2, "x"), Message::Shutdown, call(3, "")] {
        write_message(&mut input, &m).unwrap();
    }
    let mut output = Vec::new();
    serve_on(plugin(), std::io::Cursor::new(input), &mut output).unwrap();
    let all = messages_of(&output);
    assert_eq!(all.len(), 3, "a RESULT, an ERROR, nothing after SHUTDOWN: {all:?}");
    let Message::Welcome(w) = &all[0] else { panic!("{all:?}") };
    assert_eq!((w.protocol, w.capabilities.clone(), w.kinds.clone()), (1, vec!["batch".to_owned(), "cancel".to_owned()], vec![1, 4, 12, 13, 16, 18]));
    assert!(matches!(&all[1], Message::Result { call: 1, .. }), "{all:?}");
    assert!(matches!(&all[2], Message::Error { call: 2, state, .. } if state == "38T04"), "{all:?}");
}
