//! The contract of `PartitionStrategy` (RULES.md D154), checked of each
//! built-in strategy over keys and bounds of each, and of a strategy an
//! embedding program registers, used through SQL.

use turborel::catalog::{PartitionBound, RangeDatum};
use turborel::error::{Code, Result};
use turborel::partition::{HashStrategy, ListStrategy, PartitionStrategy, RangeStrategy, Restriction};
use turborel::value::{DataType, Spelled, Value};

/// INTEGER.
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

use crate::common::{exec, sorted};

/// An integer.
fn int(k: i64) -> Value {
    Value::Exact(turborel::value::Decimal::new(i128::from(k), 0))
}

/// Siblings of each built-in strategy, as its `bound` gives them, none
/// overlapping another.
fn siblings() -> Vec<(Box<dyn PartitionStrategy>, Vec<PartitionBound>)> {
    let types = [INTEGER];
    let range =
        |from: i64, to: i64| PartitionBound::Range { from: vec![RangeDatum::Value(int(from))], to: vec![RangeDatum::Value(int(to))] };
    let with = |m: i64, r: i64| PartitionBound::With(vec![("MODULUS".to_owned(), int(m)), ("REMAINDER".to_owned(), int(r))]);
    let made =
        |s: &dyn PartitionStrategy, specs: Vec<PartitionBound>| specs.into_iter().map(|b| s.bound(&types, b).unwrap()).collect::<Vec<_>>();
    let mut out: Vec<(Box<dyn PartitionStrategy>, Vec<PartitionBound>)> = Vec::new();
    let r = made(&RangeStrategy, vec![range(-50, 0), range(0, 10), range(10, 60)]);
    out.push((Box::new(RangeStrategy), r));
    let l = made(
        &ListStrategy,
        vec![PartitionBound::List(vec![vec![int(1)], vec![int(3)]]), PartitionBound::List(vec![vec![int(2)], vec![Value::Null]])],
    );
    out.push((Box::new(ListStrategy), l));
    let h = made(&HashStrategy, vec![with(2, 0), with(4, 1), with(4, 3)]);
    out.push((Box::new(HashStrategy), h));
    out
}

/// Verifies: [EXTENSION] RULES.md D154: of each built-in strategy, a
/// DEFAULT is no bound it takes; siblings that do not overlap never both
/// contain a key, and route gives the one that does; admits is true of
/// every bound containing a key that meets the restrictions; each method
/// gives one answer each time.
#[test]
fn the_built_in_strategies_keep_the_contract() -> Result<()> {
    for (s, bounds) in siblings() {
        assert!(s.bound(&[INTEGER], PartitionBound::Default).is_err(), "{}", s.name());
        for (i, a) in bounds.iter().enumerate() {
            for b in bounds.iter().skip(i + 1) {
                assert!(!s.overlaps(a, b)?, "{}", s.name());
            }
        }
        let refs: Vec<&PartitionBound> = bounds.iter().collect();
        for k in -60..70 {
            let key = [int(k)];
            let holding: Vec<usize> = (0..bounds.len()).filter(|&i| s.contains(&bounds[i], &key).unwrap()).collect();
            assert!(holding.len() <= 1, "{} {k}", s.name());
            assert_eq!(s.route(&refs, &key)?, holding.first().copied(), "{} {k}", s.name());
            assert_eq!(s.route(&refs, &key)?, s.route(&refs, &key)?);
            let restrictions = [
                vec![Some(Restriction::Equal(vec![int(k)]))],
                vec![Some(Restriction::Range { low: Some((int(k), true)), high: Some((int(k + 5), false)) })],
                vec![None],
            ];
            for r in &restrictions {
                for &i in &holding {
                    assert!(s.admits(&bounds[i], r)?, "{} {k} {r:?}", s.name());
                }
            }
        }
    }
    Ok(())
}

/// A strategy of its own: a row's key, an integer, goes to the partition
/// whose bound, WITH (EVEN 1) or WITH (EVEN 0), says whether it is even.
#[derive(Debug)]
struct Parity;

impl Parity {
    fn even(bound: &PartitionBound) -> Option<bool> {
        let PartitionBound::With(options) = bound else { return None };
        options.first().map(|(_, v)| v.to_string() == "1")
    }
}

impl PartitionStrategy for Parity {
    fn name(&self) -> &'static str {
        "PARITY"
    }

    fn check_key(&self, types: &[DataType]) -> Result<()> {
        if let [DataType::Exact { scale: 0, .. }] = types {
            Ok(())
        } else {
            Err(turborel::error::SqlError::new(Code::SyntaxRule, "PARITY divides an INTEGER"))
        }
    }

    fn bound(&self, _types: &[DataType], spec: PartitionBound) -> Result<PartitionBound> {
        if Parity::even(&spec).is_some() {
            Ok(spec)
        } else {
            Err(turborel::error::SqlError::new(Code::SyntaxRule, "PARITY takes WITH (EVEN 0 or 1)"))
        }
    }

    fn overlaps(&self, a: &PartitionBound, b: &PartitionBound) -> Result<bool> {
        Ok(Parity::even(a) == Parity::even(b))
    }

    fn contains(&self, bound: &PartitionBound, key: &[Value]) -> Result<bool> {
        let even = key.first().is_some_and(|v| v.to_string().parse::<i64>().is_ok_and(|k| k % 2 == 0));
        Ok(Parity::even(bound) == Some(even))
    }

    fn admits(&self, bound: &PartitionBound, restrictions: &[Option<Restriction>]) -> Result<bool> {
        let Some(Some(Restriction::Equal(values))) = restrictions.first() else { return Ok(true) };
        Ok(values.iter().any(|v| self.contains(bound, std::slice::from_ref(v)).unwrap_or(true)))
    }
}

/// Verifies: [EXTENSION] RULES.md D154: a strategy an embedding program
/// registers is named by PARTITION BY, routes rows and prunes reads; its
/// name is taken once.
#[test]
fn a_registered_strategy_divides_rows() {
    let db = turborel::db::Database::new();
    db.register_partition_strategy(Box::new(Parity)).unwrap();
    assert!(db.register_partition_strategy(Box::new(Parity)).is_err());
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    exec(&s, "CREATE TABLE P (K INTEGER) PARTITION BY PARITY (K)").unwrap();
    exec(&s, "CREATE TABLE PE PARTITION OF P FOR VALUES WITH (EVEN 1)").unwrap();
    exec(&s, "CREATE TABLE PO PARTITION OF P FOR VALUES WITH (EVEN 0)").unwrap();
    exec(&s, "INSERT INTO P VALUES (1), (2), (3), (4)").unwrap();
    assert_eq!(sorted(&s, "SELECT K FROM PE"), ["2", "4"]);
    let shown = super::plan(&s, "SELECT K FROM P WHERE K = 3");
    assert!(shown.contains("PO") && !shown.contains("PE"), "{shown}");
}
