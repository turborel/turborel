//! The example's metric, model, authenticator, backup target and
//! partitioning strategy.

use std::path::PathBuf;

use turborel::catalog::PartitionBound;
use turborel::value::{Decimal, Value};
use turborel_plugin::{
    Answer, Authenticator, Embedder, Exchange, Metric, PartitionStrategy, Plugin, PluginError, Step, Target, TargetStore, Upload, Who,
};

/// The plug-in with them.
pub fn with(plugin: Plugin) -> Plugin {
    plugin
        .metric("TAXICAB", Taxicab)
        .embedder("VOWELS", Vowels)
        .authenticator("EXAMPLE", Sesame)
        .target("example", Files)
        .archive("VAULT", Files)
        .partition_strategy("MODULUS", Modulus)
}

/// TAXICAB: the sum of the coordinates' differences.
struct Taxicab;

impl Metric for Taxicab {
    fn distance(&mut self, query: &[f64], candidate: &[f64]) -> Answer<Option<f64>> {
        Ok(Some(query.iter().zip(candidate).map(|(a, b)| (a - b).abs()).sum()))
    }
}

/// VOWELS: how many of each vowel a text has.
struct Vowels;

impl Embedder for Vowels {
    fn embed(&mut self, text: &str) -> Answer<Vec<f64>> {
        let lower = text.to_lowercase();
        Ok("aeiou".chars().map(|v| lower.chars().filter(|c| *c == v).fold(0.0_f64, |n, _| n + 1.0_f64)).collect())
    }
}

/// EXAMPLE: a challenge, then the user accepted if the answer is "open
/// sesame".
struct Sesame;

/// An exchange of EXAMPLE: its steps so far.
struct Asked(u32);

impl Authenticator for Sesame {
    fn begin(&mut self, _who: &Who) -> Answer<Box<dyn Exchange>> {
        Ok(Box::new(Asked(0)))
    }
}

impl Exchange for Asked {
    fn step(&mut self, bytes: &[u8]) -> Answer<Step> {
        let first = self.0 == 0;
        self.0 = self.0.saturating_add(1);
        Ok(match (first, bytes) {
            (true, _) => Step::Challenge(b"say the word".to_vec()),
            (false, b"open sesame") => Step::Accept,
            (false, _) => Step::Refuse("that is not the word".to_owned()),
        })
    }
}

/// `example`, and the host archive `vault`: each object a file of the
/// directory the location names, after its scheme.
struct Files;

/// A target of `example`: its directory.
struct Dir(PathBuf);

/// An object being written: its path and bytes.
struct Writing(PathBuf, Vec<u8>);

/// An I/O failure, as SQLSTATE 58030.
fn io(e: &std::io::Error) -> PluginError {
    PluginError::new("58030", &e.to_string())
}

impl Target for Files {
    fn open(&mut self, location: &str) -> Answer<Box<dyn TargetStore>> {
        let dir = PathBuf::from(location.split_once(':').map_or(location, |(_, path)| path));
        std::fs::create_dir_all(&dir).map_err(|e| io(&e))?;
        Ok(Box::new(Dir(dir)))
    }
}

/// The names of the files under `dir`, each after `within`.
fn walk(dir: &std::path::Path, within: &str, names: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = format!("{within}{}", entry.file_name().to_string_lossy());
        if entry.file_type()?.is_dir() {
            walk(&entry.path(), &format!("{name}/"), names)?;
        } else {
            names.push(name);
        }
    }
    Ok(())
}

impl TargetStore for Dir {
    fn begin(&mut self, object: &str) -> Answer<Box<dyn Upload>> {
        Ok(Box::new(Writing(self.0.join(object), Vec::new())))
    }
    fn read(&mut self, object: &str, offset: u64, max: u32) -> Answer<Vec<u8>> {
        let all = std::fs::read(self.0.join(object)).map_err(|e| io(&e))?;
        let from = usize::try_from(offset).unwrap_or(usize::MAX).min(all.len());
        let to = from.saturating_add(usize::try_from(max).unwrap_or(0)).min(all.len());
        Ok(all.get(from..to).unwrap_or_default().to_vec())
    }
    fn list(&mut self, prefix: &str) -> Answer<Vec<String>> {
        let mut names = Vec::new();
        walk(&self.0, "", &mut names).map_err(|e| io(&e))?;
        names.retain(|n| n.starts_with(prefix));
        names.sort();
        Ok(names)
    }
    fn remove(&mut self, object: &str) -> Answer<()> {
        std::fs::remove_file(self.0.join(object)).map_err(|e| io(&e))
    }
}

impl Upload for Writing {
    fn write(&mut self, bytes: &[u8]) -> Answer<()> {
        self.1.extend_from_slice(bytes);
        Ok(())
    }
    fn finish(self: Box<Self>) -> Answer<()> {
        if let Some(parent) = self.0.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(&e))?;
        }
        std::fs::write(&self.0, &self.1).map_err(|e| io(&e))
    }
}

/// MODULUS: rows by their one integer key's remainder.
struct Modulus;

/// A bound of MODULUS, WITH (MODULUS m, REMAINDER r), as its pair.
fn modulus(b: &PartitionBound) -> Answer<(i128, i128)> {
    let PartitionBound::With(named) = b else {
        return Err(PluginError::new("42P16", "MODULUS takes a bound WITH (MODULUS m, REMAINDER r)"));
    };
    let mut pair = (0, 0);
    let refused = || PluginError::new("42P16", "MODULUS and REMAINDER are integers");
    for (name, v) in named {
        let Value::Exact(d) = v else { return Err(refused()) };
        if d.scale != 0 {
            return Err(refused());
        }
        match name.to_uppercase().as_str() {
            "MODULUS" => pair.0 = d.units,
            "REMAINDER" => pair.1 = d.units,
            _ => return Err(refused()),
        }
    }
    if pair.0 <= 0 || pair.1 < 0 || pair.1 >= pair.0 {
        return Err(PluginError::new("42P16", "MODULUS is positive, and REMAINDER at least 0 and less than it"));
    }
    Ok(pair)
}

impl PartitionStrategy for Modulus {
    fn check_key(&mut self, types: &[String]) -> Answer<()> {
        if types.len() == 1 { Ok(()) } else { Err(PluginError::new("42P16", "MODULUS partitions by one integer column")) }
    }
    fn bound(&mut self, _types: &[String], bound: PartitionBound) -> Answer<PartitionBound> {
        let (m, n) = modulus(&bound)?;
        let exact = |x: i128| Value::Exact(Decimal::new(x, 0));
        Ok(PartitionBound::With(vec![("MODULUS".to_owned(), exact(m)), ("REMAINDER".to_owned(), exact(n))]))
    }
    fn overlaps(&mut self, a: &PartitionBound, b: &PartitionBound) -> Answer<bool> {
        let (a, b) = (modulus(a)?, modulus(b)?);
        Ok((0..a.0.saturating_mul(b.0).min(1_000_000)).any(|k| k.rem_euclid(a.0) == a.1 && k.rem_euclid(b.0) == b.1))
    }
    fn contains(&mut self, bound: &PartitionBound, key: &[Value]) -> Answer<bool> {
        let (m, n) = modulus(bound)?;
        let Some(Value::Exact(k)) = key.first() else { return Ok(false) };
        Ok(k.scale == 0 && k.units.rem_euclid(m) == n)
    }
}
