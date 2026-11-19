//! An external plug-in of turborel, whole, as `doc/client/04-plugins.md`
//! shows it: a program that speaks plug-in protocol 1 on its standard
//! input and output, and serves an object of every kind a manifest
//! declares. It links nothing of turborel, as a plug-in in another
//! language would not. Its manifest, `turborel-extension.sql`:
//!
//! ```sql
//! NAME example; VERSION '1.0.0'; PROTOCOL 1; EXECUTABLE 'plugin_example';
//! FUNCTION TWICE(INTEGER) RETURNS INTEGER DETERMINISTIC BATCH 100;
//! FUNCTION SHOUT(VARCHAR(200)) RETURNS VARCHAR(200) DETERMINISTIC;
//! AGGREGATE PRODUCT(DOUBLE PRECISION) RETURNS DOUBLE PRECISION;
//! TABLE FUNCTION SERIES(INTEGER) RETURNS TABLE (N INTEGER);
//! TABLE FUNCTION ROW_COUNT(VARCHAR(128)) RETURNS TABLE (TABLE_NAME VARCHAR(128), N BIGINT) READS SQL DATA;
//! PROCEDURE NOTE(VARCHAR(200)) MODIFIES SQL DATA;
//! METRIC TAXICAB FORMATS (FLOAT32, FLOAT64) PROPERTIES (SYMMETRIC, NON_NEGATIVE, TRIANGLE);
//! EMBEDDING MODEL VOWELS DIMENSIONS 5 DETERMINISTIC;
//! AUTHENTICATOR 'EXAMPLE'; TARGET 'example'; PARTITION STRATEGY MODULUS;
//! ```
//!
//! A function named as another with digits after, as SHOUT2, is that
//! function, so other manifests may declare its functions again. The
//! functions PID, FAIL, CRASH, HANG, GARBAGE and HUGE show what the
//! database does of a program that fails: its process id, an ERROR of
//! SQLSTATE 22012, an exit, a call that never answers, a frame of no
//! message, and an answer longer than the database takes.

use std::collections::HashMap;
use std::io::{Read, Write};

/// The message types of protocol 1.
const HELLO: u8 = 1;
const WELCOME: u8 = 2;
const CALL: u8 = 3;
const RESULT: u8 = 4;
const ERROR: u8 = 5;
const SHUTDOWN: u8 = 7;
const SQL: u8 = 8;
const SQL_DONE: u8 = 9;

/// A failure: a SQLSTATE and its message.
type Failure = (String, String);
/// What a call answers.
type Answer = Result<Vec<u8>, Failure>;

/// A failure of `state`.
fn fail<T>(state: &str, message: &str) -> Result<T, Failure> {
    Err((state.to_owned(), message.to_owned()))
}

/// A malformed payload's failure.
fn malformed<T>() -> Result<T, Failure> {
    fail("38000", "the payload is malformed")
}

/// What a payload is read by: little-endian numbers, strings and bytes
/// after their length.
struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], Failure> {
        if n > self.0.len() {
            return malformed();
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }
    fn u8(&mut self) -> Result<u8, Failure> {
        Ok(self.take(1)?.first().copied().unwrap_or(0))
    }
    fn u32(&mut self) -> Result<u32, Failure> {
        let b: [u8; 4] = self.take(4)?.try_into().or_else(|_| malformed())?;
        Ok(u32::from_le_bytes(b))
    }
    fn u64(&mut self) -> Result<u64, Failure> {
        let b: [u8; 8] = self.take(8)?.try_into().or_else(|_| malformed())?;
        Ok(u64::from_le_bytes(b))
    }
    fn i128(&mut self) -> Result<i128, Failure> {
        let b: [u8; 16] = self.take(16)?.try_into().or_else(|_| malformed())?;
        Ok(i128::from_le_bytes(b))
    }
    fn bytes(&mut self) -> Result<Vec<u8>, Failure> {
        let n = usize::try_from(self.u32()?).or_else(|_| malformed())?;
        Ok(self.take(n)?.to_vec())
    }
    fn str(&mut self) -> Result<String, Failure> {
        String::from_utf8(self.bytes()?).or_else(|_| malformed())
    }
    fn strs(&mut self) -> Result<Vec<String>, Failure> {
        (0..self.u32()?).map(|_| self.str()).collect()
    }
    fn values(&mut self) -> Result<Vec<V>, Failure> {
        (0..self.u32()?).map(|_| self.value()).collect()
    }
    fn rows(&mut self) -> Result<Vec<Vec<V>>, Failure> {
        (0..self.u32()?).map(|_| self.values()).collect()
    }
    /// A value: its tag, then its body.
    fn value(&mut self) -> Result<V, Failure> {
        Ok(match self.u8()? {
            0 => V::Null,
            1 => V::Bool(self.u8()? != 0),
            2 => V::Exact(self.i128()?, self.u32()?),
            3 => V::Approx(f64::from_bits(self.u64()?)),
            4 | 6 => V::Char(self.str()?),
            5 => self.bytes().map(|_| V::Other)?,
            7 | 8 => self.values().map(|_| V::Other)?,
            9 => V::Dense((0..self.u32()?).map(|_| self.u64().map(f64::from_bits)).collect::<Result<Vec<f64>, Failure>>()?),
            10 => {
                let dims = self.u32()?;
                let pairs = (0..self.u32()?)
                    .map(|_| Ok((self.u32()?, f64::from_bits(self.u64()?))))
                    .collect::<Result<Vec<(u32, f64)>, Failure>>()?;
                V::Sparse(dims, pairs)
            }
            _ => return malformed(),
        })
    }
}

/// A value of protocol 1, as this program uses them.
#[derive(Clone, Debug)]
enum V {
    Null,
    Bool(bool),
    Exact(i128, u32),
    Approx(f64),
    Char(String),
    Dense(Vec<f64>),
    Sparse(u32, Vec<(u32, f64)>),
    /// A value this program reads past: binary, array or row.
    Other,
}

/// What a payload is written by.
#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn u8(mut self, v: u8) -> Out {
        self.0.push(v);
        self
    }
    fn u32(mut self, v: u32) -> Out {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(mut self, v: u64) -> Out {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn bytes(self, v: &[u8]) -> Out {
        let mut o = self.u32(u32::try_from(v.len()).unwrap_or(u32::MAX));
        o.0.extend_from_slice(v);
        o
    }
    fn str(self, v: &str) -> Out {
        self.bytes(v.as_bytes())
    }
    fn strs(self, all: &[String]) -> Out {
        all.iter().fold(self.u32(u32::try_from(all.len()).unwrap_or(u32::MAX)), |o, s| o.str(s))
    }
    fn values(self, all: &[V]) -> Out {
        all.iter().fold(self.u32(u32::try_from(all.len()).unwrap_or(u32::MAX)), Out::value)
    }
    fn rows(self, all: &[Vec<V>]) -> Out {
        all.iter().fold(self.u32(u32::try_from(all.len()).unwrap_or(u32::MAX)), |o, r| o.values(r))
    }
    fn value(self, v: &V) -> Out {
        match v {
            V::Null | V::Other => self.u8(0),
            V::Bool(b) => self.u8(1).u8(u8::from(*b)),
            V::Exact(units, scale) => {
                let mut o = self.u8(2);
                o.0.extend_from_slice(&units.to_le_bytes());
                o.u32(*scale)
            }
            V::Approx(x) => self.u8(3).u64(x.to_bits()),
            V::Char(s) => self.u8(4).str(s),
            V::Dense(xs) => xs.iter().fold(self.u8(9).u32(u32::try_from(xs.len()).unwrap_or(u32::MAX)), |o, x| o.u64(x.to_bits())),
            V::Sparse(dims, pairs) => {
                let o = self.u8(10).u32(*dims).u32(u32::try_from(pairs.len()).unwrap_or(u32::MAX));
                pairs.iter().fold(o, |o, (i, x)| o.u32(*i).u64(x.to_bits()))
            }
        }
    }
}

/// The program's pipes and what it keeps between calls: its exchanges,
/// targets and uploads, each by the number it gave.
struct Plugin {
    input: std::io::Stdin,
    output: std::io::Stdout,
    next: u64,
    exchanges: HashMap<u64, u32>,
    targets: HashMap<u64, std::path::PathBuf>,
    uploads: HashMap<u64, (String, Vec<u8>)>,
}

impl Plugin {
    /// The next message's type and body, or none at the end of input.
    fn read(&mut self) -> Option<(u8, Vec<u8>)> {
        let mut length = [0u8; 4];
        self.input.read_exact(&mut length).ok()?;
        let mut body = vec![0u8; usize::try_from(u32::from_le_bytes(length)).ok()?];
        self.input.read_exact(&mut body).ok()?;
        let (&kind, rest) = body.split_first()?;
        Some((kind, rest.to_vec()))
    }

    /// Writes a message of type `kind` and body `body`.
    fn write(&mut self, kind: u8, body: &[u8]) -> std::io::Result<()> {
        let length = u32::try_from(body.len().saturating_add(1)).unwrap_or(u32::MAX);
        self.output.write_all(&length.to_le_bytes())?;
        self.output.write_all(&[kind])?;
        self.output.write_all(body)?;
        self.output.flush()
    }

    /// A number for a new exchange, target or upload.
    fn number(&mut self) -> u64 {
        self.next = self.next.saturating_add(1);
        self.next
    }

    /// Runs `text` with `args` in the call's transaction, a query if
    /// `rows`: its rows, or the database's failure.
    fn sql(&mut self, call: u64, (rows, text): (bool, &str), args: &[V]) -> Result<Vec<Vec<V>>, Failure> {
        let asked = Out::default().u64(call).u8(u8::from(rows)).str(text).values(args);
        self.write(SQL, &asked.0).or_else(|_| fail("38000", "the database is gone"))?;
        match self.read() {
            Some((SQL_DONE, reply)) => {
                let mut r = Reader(&reply);
                r.u64()?;
                r.rows()
            }
            Some((ERROR, reply)) => {
                let mut r = Reader(&reply);
                r.u64()?;
                let state = String::from_utf8_lossy(r.take(5)?).into_owned();
                Err((state, r.str()?))
            }
            Some(_) | None => fail("38000", "the database answered no SQL_DONE"),
        }
    }
}

fn main() {
    let mut p = Plugin {
        input: std::io::stdin(),
        output: std::io::stdout(),
        next: 0,
        exchanges: HashMap::new(),
        targets: HashMap::new(),
        uploads: HashMap::new(),
    };
    while let Some((kind, body)) = p.read() {
        let written = match kind {
            HELLO => welcome(&mut p, &body),
            CALL => called(&mut p, &body),
            SHUTDOWN => return,
            // A CANCEL of a call answered already, or anything else.
            _ => Ok(()),
        };
        if written.is_err() {
            return;
        }
    }
}

/// WELCOME, of the name HELLO gives: protocol 1, every kind.
fn welcome(p: &mut Plugin, body: &[u8]) -> std::io::Result<()> {
    let mut r = Reader(body);
    let name = r.u32().and_then(|_| r.str()).unwrap_or_default();
    let caps = ["batch".to_owned(), "sql".to_owned()];
    let out = Out::default().u32(1).str(&name).str("1.0.0").strs(&caps).bytes(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
    p.write(WELCOME, &out.0)
}

/// A CALL, answered by RESULT or ERROR.
fn called(p: &mut Plugin, body: &[u8]) -> std::io::Result<()> {
    let mut r = Reader(body);
    let head = (|| Ok::<_, Failure>((r.u64()?, r.u8()?, r.u8()?, r.str()?)))();
    let Ok((call, kind, op, object)) = head else { return Ok(()) };
    let payload = r.0.to_vec();
    match answer(p, call, (kind, op, object.as_str()), &payload) {
        Ok(result) => p.write(RESULT, &Out::default().u64(call).0.into_iter().chain(result).collect::<Vec<u8>>()),
        Err((state, message)) => {
            let mut out = Out::default().u64(call);
            out.0.extend(state.bytes().chain(std::iter::repeat(b'0')).take(5));
            p.write(ERROR, &out.str(&message).0)
        }
    }
}

/// The answer of operation `op` of the object `object` of kind `kind`.
fn answer(p: &mut Plugin, call: u64, (kind, op, object): (u8, u8, &str), payload: &[u8]) -> Answer {
    let mut r = Reader(payload);
    match kind {
        1 => scalar(object, r.rows()?),
        2 => aggregate(op, &mut r),
        3 => table(op, object, &mut r),
        4 => procedure(p, call, &r.values()?),
        5 => metric(&mut r),
        6 => Ok(embed(&r.strs()?)),
        7 => authenticate(p, op, &mut r),
        8 => target(p, op, &mut r),
        9 => partition(op, &mut r),
        _ => fail("38000", "no such kind"),
    }
}

/// A scalar function of each row of `rows`.
fn scalar(object: &str, rows: Vec<Vec<V>>) -> Answer {
    let mut out = Vec::with_capacity(rows.len());
    // A function named as another with digits after, as SHOUT2, is it.
    let object = object.trim_end_matches(|c: char| c.is_ascii_digit());
    let takes = !matches!(object, "PID" | "CRASH" | "HANG" | "GARBAGE");
    for row in rows {
        let first = row.into_iter().next().unwrap_or(V::Null);
        if takes && matches!(first, V::Null) {
            out.push(V::Null);
            continue;
        }
        out.push(match (object, first) {
            ("TWICE", V::Exact(units, scale)) => {
                V::Exact(units.checked_mul(2).ok_or(("22003".to_owned(), "out of range".to_owned()))?, scale)
            }
            ("SHOUT", V::Char(s)) => V::Char(s.to_uppercase()),
            ("PID", _) => V::Exact(i128::from(std::process::id()), 0),
            ("FAIL", V::Char(s)) => return fail("22012", &s),
            ("CRASH", _) => std::process::abort(),
            ("HANG", _) => loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            },
            ("GARBAGE", _) => {
                let _ = std::io::stdout().write_all(&[1, 0, 0, 0, 99]).and_then(|()| std::io::stdout().flush());
                V::Null
            }
            ("HUGE", V::Exact(n, 0)) => V::Char("x".repeat(usize::try_from(n).unwrap_or(0))),
            _ => return fail("38000", "no such function, or an argument of a type it takes not"),
        });
    }
    Ok(Out::default().values(&out).0)
}

/// PRODUCT: its state is the product so far, eight bytes, or none.
fn aggregate(op: u8, r: &mut Reader<'_>) -> Answer {
    let product = |state: &[u8]| state.try_into().ok().map(|b: [u8; 8]| f64::from_bits(u64::from_le_bytes(b)));
    let state = product(&r.bytes()?);
    let state = match op {
        1 => r.values()?.iter().fold(state, |s, v| match v {
            V::Approx(x) => Some(s.unwrap_or(1.0_f64) * x),
            V::Exact(units, 0) => Some(s.unwrap_or(1.0_f64) * format!("{units}").parse::<f64>().unwrap_or(0.0_f64)),
            V::Exact(..) | V::Null | V::Bool(_) | V::Char(_) | V::Dense(_) | V::Sparse(..) | V::Other => s,
        }),
        2 => match (state, product(&r.bytes()?)) {
            (Some(a), Some(b)) => Some(a * b),
            (a, b) => a.or(b),
        },
        _ => return Ok(Out::default().value(&state.map_or(V::Null, V::Approx)).0),
    };
    Ok(Out::default().bytes(&state.map(|x| x.to_bits().to_le_bytes().to_vec()).unwrap_or_default()).0)
}

/// SERIES and ROW_COUNT: the queries ROW_COUNT reads, and the rows.
fn table(op: u8, object: &str, r: &mut Reader<'_>) -> Answer {
    let args = r.values()?;
    let first = args.into_iter().next().unwrap_or(V::Null);
    match (op, object, first) {
        (2, "ROW_COUNT", V::Char(t)) => Ok(Out::default().strs(&[format!("SELECT COUNT(*) FROM {t}")]).0),
        (2, _, _) => Ok(Out::default().strs(&[]).0),
        (_, "SERIES", V::Exact(n, 0)) => {
            let rows: Vec<Vec<V>> = (1..=n.min(1_000_000)).map(|i| vec![V::Exact(i, 0)]).collect();
            Ok(Out::default().rows(&rows).0)
        }
        (_, "ROW_COUNT", V::Char(t)) => {
            let reads: Vec<Vec<Vec<V>>> = (0..r.u32()?).map(|_| r.rows()).collect::<Result<_, _>>()?;
            let count = reads.into_iter().flatten().flatten().next().unwrap_or(V::Null);
            Ok(Out::default().rows(&[vec![V::Char(t), count]]).0)
        }
        _ => Ok(Out::default().rows(&[]).0),
    }
}

/// NOTE: inserts its text into the table NOTES, by SQL in the call's
/// transaction.
fn procedure(p: &mut Plugin, call: u64, args: &[V]) -> Answer {
    p.sql(call, (false, "INSERT INTO NOTES VALUES (?)"), args)?;
    Ok(Vec::new())
}

/// The coordinates of `v`, dense.
fn dense(v: &V) -> Vec<f64> {
    match v {
        V::Dense(xs) => xs.clone(),
        V::Sparse(dims, pairs) => {
            let mut xs = vec![0.0_f64; usize::try_from(*dims).unwrap_or(0)];
            for (i, x) in pairs {
                if let Some(slot) = usize::try_from(*i).ok().and_then(|i| xs.get_mut(i)) {
                    *slot = *x;
                }
            }
            xs
        }
        V::Null | V::Bool(_) | V::Exact(..) | V::Approx(_) | V::Char(_) | V::Other => Vec::new(),
    }
}

/// TAXICAB: the distance from the query to each candidate.
fn metric(r: &mut Reader<'_>) -> Answer {
    let query = dense(&r.value()?);
    let candidates = (0..r.u32()?).map(|_| r.value()).collect::<Result<Vec<_>, _>>()?;
    let distances: Vec<V> = candidates.iter().map(|c| V::Approx(query.iter().zip(dense(c)).map(|(a, b)| (a - b).abs()).sum())).collect();
    Ok(Out::default().values(&distances).0)
}

/// VOWELS: how many of each vowel a text has.
fn embed(texts: &[String]) -> Vec<u8> {
    let vectors: Vec<V> = texts
        .iter()
        .map(|t| {
            V::Dense("aeiou".chars().map(|v| t.to_lowercase().chars().filter(|c| *c == v).fold(0.0_f64, |n, _| n + 1.0_f64)).collect())
        })
        .collect();
    Out::default().values(&vectors).0
}

/// EXAMPLE: a challenge, then the user accepted if the answer is "open
/// sesame", refused if it is not.
fn authenticate(p: &mut Plugin, op: u8, r: &mut Reader<'_>) -> Answer {
    if op == 1 {
        let n = p.number();
        p.exchanges.insert(n, 0);
        return Ok(Out::default().u64(n).0);
    }
    let n = r.u64()?;
    let said = r.bytes()?;
    let steps = p.exchanges.get(&n).copied().ok_or(("38000".to_owned(), "no such exchange".to_owned()))?;
    p.exchanges.insert(n, steps.saturating_add(1));
    Ok(match (steps, said.as_slice()) {
        (0, _) => Out::default().u8(0).bytes(b"say the word"),
        (_, b"open sesame") => Out::default().u8(1),
        _ => Out::default().u8(2).str("that is not the word"),
    }
    .0)
}

/// `example`: each object a file of the directory the location names.
fn target(p: &mut Plugin, op: u8, r: &mut Reader<'_>) -> Answer {
    let io = |e: std::io::Error| ("58030".to_owned(), e.to_string());
    if op == 1 {
        let dir = std::path::PathBuf::from(r.str()?);
        std::fs::create_dir_all(&dir).map_err(io)?;
        let n = p.number();
        p.targets.insert(n, dir);
        return Ok(Out::default().u64(n).0);
    }
    let dir = p.targets.get(&r.u64()?).cloned().ok_or(("38000".to_owned(), "no such target".to_owned()))?;
    match op {
        2 => {
            let n = p.number();
            p.uploads.insert(n, (r.str()?, Vec::new()));
            Ok(Out::default().u64(n).0)
        }
        3 => p.uploads.remove(&r.u64()?).map(|_| Vec::new()).ok_or(("38000".to_owned(), "no such upload".to_owned())),
        4 => {
            let upload = r.u64()?;
            let bytes = r.bytes()?;
            p.uploads.get_mut(&upload).map(|(_, all)| all.extend(bytes)).ok_or(("38000".to_owned(), "no such upload".to_owned()))?;
            Ok(Vec::new())
        }
        5 => {
            let (object, bytes) = p.uploads.remove(&r.u64()?).ok_or(("38000".to_owned(), "no such upload".to_owned()))?;
            std::fs::write(dir.join(object), bytes).map_err(io)?;
            Ok(Vec::new())
        }
        _ => stored(op, &dir, r),
    }
}

/// `example`'s reads, lists and removals.
fn stored(op: u8, dir: &std::path::Path, r: &mut Reader<'_>) -> Answer {
    let io = |e: std::io::Error| ("58030".to_owned(), e.to_string());
    let name = r.str()?;
    match op {
        6 => {
            let all = std::fs::read(dir.join(&name)).map_err(io)?;
            let from = usize::try_from(r.u64()?).unwrap_or(usize::MAX).min(all.len());
            let max = usize::try_from(r.u32()?).unwrap_or(0);
            Ok(Out::default().bytes(all.get(from..from.saturating_add(max).min(all.len())).unwrap_or_default()).0)
        }
        7 => {
            let mut names: Vec<String> =
                std::fs::read_dir(dir).map_err(io)?.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect();
            names.retain(|n| n.starts_with(&name));
            names.sort();
            Ok(Out::default().strs(&names).0)
        }
        8 => std::fs::remove_file(dir.join(name)).map(|()| Vec::new()).map_err(io),
        _ => fail("38000", "no such operation"),
    }
}

/// A bound of MODULUS: WITH (MODULUS m, REMAINDER r), as its pair.
fn modulus(r: &mut Reader<'_>) -> Result<(i128, i128), Failure> {
    if r.u8()? != 3 {
        return fail("42P16", "MODULUS takes a bound WITH (MODULUS m, REMAINDER r)");
    }
    let mut pair = (0, 0);
    for _ in 0..r.u32()? {
        match (r.str()?.to_uppercase().as_str(), r.value()?) {
            ("MODULUS", V::Exact(m, 0)) => pair.0 = m,
            ("REMAINDER", V::Exact(n, 0)) => pair.1 = n,
            _ => return fail("42P16", "MODULUS and REMAINDER are integers"),
        }
    }
    if pair.0 <= 0 || pair.1 < 0 || pair.1 >= pair.0 {
        return fail("42P16", "MODULUS is positive, and REMAINDER at least 0 and less than it");
    }
    Ok(pair)
}

/// Whether the key `key` falls in the bound (m, n).
fn falls(key: &[V], (m, n): (i128, i128)) -> bool {
    let Some(V::Exact(k, 0)) = key.first() else { return false };
    k.rem_euclid(m) == n
}

/// MODULUS: rows by their one integer key's remainder.
fn partition(op: u8, r: &mut Reader<'_>) -> Answer {
    let truth = |b: bool| Ok(Out::default().u8(u8::from(b)).0);
    match op {
        1 => match r.strs()?.as_slice() {
            [_] => Ok(Vec::new()),
            _ => fail("42P16", "MODULUS partitions by one integer column"),
        },
        2 => {
            r.strs()?;
            let (m, n) = modulus(r)?;
            let with = [("MODULUS", m), ("REMAINDER", n)];
            Ok(with.iter().fold(Out::default().u8(3).u32(2), |o, (k, v)| o.str(k).value(&V::Exact(*v, 0))).0)
        }
        3 => {
            let (a, b) = (modulus(r)?, modulus(r)?);
            truth((0..a.0.saturating_mul(b.0).min(1_000_000)).any(|k| k.rem_euclid(a.0) == a.1 && k.rem_euclid(b.0) == b.1))
        }
        4 => {
            let bound = modulus(r)?;
            truth(falls(&r.values()?, bound))
        }
        5 => {
            let bounds = (0..r.u32()?).map(|_| modulus(r)).collect::<Result<Vec<_>, _>>()?;
            let key = r.values()?;
            match bounds.iter().position(|b| falls(&key, *b)).and_then(|i| u32::try_from(i).ok()) {
                Some(i) => Ok(Out::default().u8(1).u32(i).0),
                None => Ok(Out::default().u8(0).0),
            }
        }
        _ => truth(true),
    }
}
