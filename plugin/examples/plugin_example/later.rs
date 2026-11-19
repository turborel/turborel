//! The example's ranking, quantizer, collation, system table, XQuery
//! function, type, credentials and sink of events.

use std::io::Write;

use turborel::fulltext::rank::{Corpus, InDocument, TermStats};
use turborel::value::{Decimal, Value};
use turborel_plugin::{
    Answer, Collation, Credentials, Diagnostics, Made, Plugin, PluginError, Quantizer, Ranking, SystemTable, Told, Trained, Training, Type,
    XQueryFunction,
};

/// The plug-in with them.
pub fn with(plugin: Plugin) -> Plugin {
    plugin
        .ranking("COUNTED", Counted)
        .quantizer("FIRSTS", Firsts)
        .collation("FOLDED", Folded)
        .system_table("EXAMPLE_FACTS", Facts)
        .xquery_function("{http://example.com/fn}twice", Twice)
        .data_type("PAIR", Pair)
        .credentials("REVERSED", Reversed)
        .diagnostics("EVENTS", Events)
}

/// COUNTED: a term's frequency, by its rarity.
struct Counted;

/// How rare a term is in `corpus`.
fn rarity(corpus: &Corpus, term: &TermStats) -> f64 {
    let documents = f64::from(u32::try_from(corpus.documents).unwrap_or(u32::MAX));
    let with = f64::from(u32::try_from(term.documents.max(1)).unwrap_or(u32::MAX));
    (1.0_f64 + documents / with).ln()
}

impl Ranking for Counted {
    fn weight(&mut self, corpus: &Corpus, term: &TermStats, in_doc: &InDocument) -> f64 {
        f64::from(in_doc.frequency) * rarity(corpus, term)
    }
    fn bound(&mut self, corpus: &Corpus, term: &TermStats) -> f64 {
        f64::from(term.most) * rarity(corpus, term)
    }
}

/// FIRSTS: the first vectors of the sample, each subspace's centroids.
struct Firsts;

impl Quantizer for Firsts {
    fn train(&mut self, t: &Training) -> Answer<Trained> {
        let dims = usize::try_from(t.dims).unwrap_or(0);
        let parts = usize::try_from(t.code_bytes).unwrap_or(0).max(1).min(dims.max(1));
        let k = t.sample.len().clamp(1, 256);
        let base = dims.checked_div(parts).unwrap_or(0);
        let wider = dims.checked_rem(parts).unwrap_or(0);
        let widths: Vec<usize> = (0..parts).map(|s| base.saturating_add(usize::from(s < wider))).collect();
        let mut start = 0_usize;
        let mut centroids = Vec::with_capacity(parts);
        for w in &widths {
            let end = start.saturating_add(*w);
            let of = |v: &Vec<f32>| v.get(start..end).map_or_else(|| vec![0.0_f32; *w], <[f32]>::to_vec);
            centroids.push(t.sample.iter().cycle().take(k).flat_map(of).collect());
            start = end;
        }
        let widths = widths.into_iter().map(|w| u32::try_from(w).unwrap_or(u32::MAX)).collect();
        Ok(Trained { k: u32::try_from(k).unwrap_or(1), widths, centroids })
    }
}

/// FOLDED: strings compared as their lower case, in code point order.
struct Folded;

impl Collation for Folded {
    fn sort_key(&mut self, text: &str, pad: bool) -> Answer<Vec<u8>> {
        let text = if pad { text.trim_end_matches(' ') } else { text };
        Ok(text.to_lowercase().into_bytes())
    }
}

/// EXAMPLE_FACTS: whether its reader owns the database, then 2500 rows.
struct Facts;

/// An integer value.
fn exact(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

impl SystemTable for Facts {
    fn rows(&mut self, _user: &str, owner: bool) -> Answer<Vec<Vec<Value>>> {
        let mut rows = vec![vec![Value::Char("OWNER".to_owned()), exact(i128::from(owner))]];
        rows.extend((1..=2500).map(|i| vec![Value::Char("ROW".to_owned()), exact(i)]));
        Ok(rows)
    }
}

/// `{http://example.com/fn}twice`: twice an integer.
struct Twice;

impl XQueryFunction for Twice {
    fn call(&mut self, args: &[Value]) -> Answer<Value> {
        let Some(Value::Exact(d)) = args.first() else { return Ok(Value::Null) };
        Ok(Value::Exact(d.checked_mul(Decimal::new(2, 0))?))
    }
}

/// PAIR: two integers, written `(a,b)`, ordered by the first then the
/// second; its binary form sixteen bytes, each little-endian.
struct Pair;

/// The value of `(a, b)`.
fn pair(a: i64, b: i64) -> Made {
    let ordered = |x: i64| (x.cast_unsigned() ^ (1_u64 << 63)).to_be_bytes();
    let key = ordered(a).into_iter().chain(ordered(b)).collect();
    let binary = a.to_le_bytes().into_iter().chain(b.to_le_bytes()).collect();
    Made { key, binary, text: format!("({a},{b})") }
}

/// A text no pair's.
fn not_pair(text: &str) -> PluginError {
    PluginError::new("22P02", &format!("{text} is not a pair (a,b)"))
}

impl Type for Pair {
    fn input(&mut self, text: &str) -> Answer<Made> {
        let inner = text.trim().strip_prefix('(').and_then(|t| t.strip_suffix(')')).ok_or_else(|| not_pair(text))?;
        let (a, b) = inner.split_once(',').ok_or_else(|| not_pair(text))?;
        let number = |s: &str| s.trim().parse::<i64>().map_err(|_| not_pair(text));
        Ok(pair(number(a)?, number(b)?))
    }
    fn receive(&mut self, bytes: &[u8]) -> Answer<Made> {
        let half = |at: usize| bytes.get(at..at.saturating_add(8)).and_then(|b| b.try_into().ok()).map(i64::from_le_bytes);
        match (bytes.len(), half(0), half(8)) {
            (16, Some(a), Some(b)) => Ok(pair(a, b)),
            _ => Err(PluginError::new("22P03", "a pair's binary form is sixteen bytes")),
        }
    }
}

/// REVERSED: a verifier of the user and the password backwards.
struct Reversed;

impl Credentials for Reversed {
    fn verifier(&mut self, user: &str, password: &str) -> Answer<Vec<u8>> {
        Ok(format!("{user}:{}", password.chars().rev().collect::<String>()).into_bytes())
    }
}

/// EVENTS: each event's level and kind a line of `events.log`.
struct Events;

impl Diagnostics for Events {
    fn events(&mut self, events: &[Told], dropped: u64) -> Answer<()> {
        let io = |e: std::io::Error| PluginError::new("58030", &e.to_string());
        let mut log = std::fs::OpenOptions::new().create(true).append(true).open("events.log").map_err(io)?;
        for e in events {
            writeln!(log, "{} {}", e.level, e.kind).map_err(io)?;
        }
        if dropped > 0 {
            writeln!(log, "DROPPED {dropped}").map_err(io)?;
        }
        Ok(())
    }
}
