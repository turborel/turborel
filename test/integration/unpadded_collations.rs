//! A collation of NO PAD that orders strings as their octets do [ISO/IEC
//! 9075-2:2023 §4.3.2, §8.2 GR3b; RULES.md D1183]: `'a'`, `'a '` and `'a'`
//! with a tab after it, which compare otherwise padded, through each
//! operator that reads the strings in place and each that still compares
//! sort keys, of either store, by batches and a row at a time.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{both_ways, exec, rows};

/// The rows written to each table.
const ROWS: usize = 6000;

/// The strings of column S, a row's by its number. Unpadded they are in
/// this order; padded the first two are equal, the third and fifth are,
/// and the fourth is below the third.
const STRINGS: [&str; 7] = ["", "  ", "a", "a\t", "a ", "ab", "b"];

/// S of row `k`: null every 11th.
fn s_of(k: usize) -> Option<&'static str> {
    (!k.is_multiple_of(11)).then(|| STRINGS[k % 7])
}

/// U of row `k`: long and nearly unique, as a table of symbols codes it,
/// a space after every fifth.
fn u_of(k: usize) -> String {
    format!("http://example.test/path/{}/item{}", k >> 1_u32, if k.is_multiple_of(5) { " " } else { "" })
}

/// Tables R, of the row store, and C, of the column store, alike, S and U
/// under BYTES, of NO PAD from UCS_BASIC, and P padded; D of two rows,
/// `'a'` and `'a '`, in S under BYTES and in X padded. Written, then frozen.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    let columns = "(K INTEGER PRIMARY KEY, S VARCHAR(20) COLLATE A.BYTES, U VARCHAR(80) COLLATE A.BYTES, P VARCHAR(20))";
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE COLLATION BYTES FOR SQL_TEXT FROM UCS_BASIC NO PAD
           CREATE TABLE R {columns}
           CREATE TABLE C {columns} USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE D (S VARCHAR(20) COLLATE A.BYTES, X VARCHAR(20), V INTEGER)"
    ))
    .unwrap();
    let a = db.session("A");
    let text = |s: Option<&str>| s.map_or("NULL".to_owned(), |s| format!("'{s}'"));
    for table in ["R", "C"] {
        for first in (0..ROWS).step_by(500) {
            let values: Vec<String> =
                (first..first + 500).map(|k| format!("({k}, {}, '{}', {})", text(s_of(k)), u_of(k), text(s_of(k)))).collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        a.commit().unwrap();
    }
    a.run("INSERT INTO D VALUES ('a', 'a', 1), ('a ', 'a ', 2)").unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// The rows of `sql` of each table, by batches, asserted the same a row at
/// a time and of both stores.
fn of_both(a: &Session, sql: &str) -> Vec<String> {
    let of = |table: &str| {
        let (batched, by_rows) = both_ways(a, &sql.replace("{t}", table));
        assert_eq!(batched, by_rows, "{table}: by batch and row by row: {sql}");
        batched
    };
    let (row_store, column_store) = (of("R"), of("C"));
    assert_eq!(row_store, column_store, "of both stores: {sql}");
    column_store
}

/// How many rows have an S that `keeps` keeps.
fn count_of(keeps: impl Fn(&[u8]) -> bool) -> Vec<String> {
    vec![(0..ROWS).filter_map(s_of).filter(|s| keeps(s.as_bytes())).count().to_string()]
}

/// The plan `s` makes of `sql`, as optimized.
fn physical(s: &Session, sql: &str) -> String {
    let plan = s.explain(sql, &turborel::params::Params::new()).unwrap();
    plan.split("-- physical:").nth(1).unwrap_or_default().to_owned()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.2, §8.2 GR3b] a filter
/// under a collation of NO PAD by octets: `=` and `<>` tell `'a'` from
/// `'a '`, and the empty string from spaces; `<`, `<=`, `>`, `>=` and
/// BETWEEN order a prefix first, a tab after `'a'` above it; IN and NOT IN
/// are of equal octets; either way round; and the padded column beside it
/// still compares padded.
#[test]
fn a_filter_compares_octets_unpadded_8_2() {
    let (_db, a) = tables();
    let count = |cond: &str| of_both(&a, &format!("SELECT COUNT(*) FROM {{t}} WHERE {cond}"));
    assert_eq!(count("S = 'a'"), count_of(|s| s == b"a"));
    assert_eq!(count("'a ' = S"), count_of(|s| s == b"a "));
    assert_eq!(count("S <> 'a'"), count_of(|s| s != b"a"));
    assert_eq!(count("S <> ''"), count_of(|s| !s.is_empty()));
    assert_eq!(count("S = ''"), count_of(<[u8]>::is_empty));
    assert_eq!(count("S < 'a '"), count_of(|s| s < b"a ".as_slice()));
    assert_eq!(count("S <= 'a'"), count_of(|s| s <= b"a".as_slice()));
    assert_eq!(count("S > 'a'"), count_of(|s| s > b"a".as_slice()));
    assert_eq!(count("'a\t' <= S"), count_of(|s| s >= b"a\t".as_slice()));
    assert_eq!(count("S BETWEEN 'a' AND 'a '"), count_of(|s| s >= b"a".as_slice() && s <= b"a ".as_slice()));
    assert_eq!(count("S IN ('a', 'b', '  ')"), count_of(|s| [b"a".as_slice(), b"b", b"  "].contains(&s)));
    assert_eq!(count("S NOT IN ('a', 'b')"), count_of(|s| s != b"a" && s != b"b"));
    assert_eq!(count("S = 'a' OR S = 'ab' OR K = 1"), {
        let kept = (0..ROWS).filter(|&k| k == 1 || s_of(k).is_some_and(|s| s == "a" || s == "ab")).count();
        vec![kept.to_string()]
    });
    // Padded: 'a' and 'a ' are one, the tab below them.
    assert_eq!(count("P = 'a'"), count_of(|s| s == b"a" || s == b"a "));
    assert_eq!(count("P < 'a'"), count_of(|s| s == b"" || s == b"  " || s == b"a\t"));
    // A constant of the symbols' column, its space told apart.
    let with_space = "http://example.test/path/5/item ";
    assert_eq!(count(&format!("U = '{with_space}'")), ["1"]);
    assert_eq!(count(&format!("U = '{}'", with_space.trim_end())), ["1"]);
    assert_eq!(count(&format!("U <> '{}'", with_space.trim_end())), [(ROWS - 1).to_string()]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.2, §7.13, §10.9] GROUP
/// BY, DISTINCT, COUNT(DISTINCT), MAX and MIN under a collation of NO PAD
/// by octets: `'a'`, `'a '` and a tab after `'a'` are three groups, each
/// of its own rows; MIN and MAX are by octet order, a prefix the less; run
/// alone and in parallel.
#[test]
fn grouping_and_extremes_are_unpadded_8_2() {
    let (_db, a) = tables();
    let groups = |strings: &[&str]| -> Vec<String> {
        let mut out: Vec<String> =
            strings.iter().map(|g| format!("[{g}],{}", (0..ROWS).filter(|&k| s_of(k) == Some(*g)).count())).collect();
        out.sort();
        out
    };
    for degree in [1, 4] {
        exec(&a, &format!("SET EXECUTOR.DEGREE = {degree}")).unwrap();
        exec(&a, "SET EXECUTOR.MORSEL_ROWS = 256").unwrap();
        exec(&a, "SET EXECUTOR.PARALLEL_MIN_ROWS = 0").unwrap();
        let grouped = of_both(&a, "SELECT '[' || S || ']', COUNT(*) FROM {t} WHERE S IS NOT NULL GROUP BY S");
        assert_eq!(grouped, groups(&STRINGS), "degree {degree}");
        assert_eq!(of_both(&a, "SELECT COUNT(*) FROM (SELECT DISTINCT S FROM {t}) AS Q"), ["8"]);
        assert_eq!(of_both(&a, "SELECT COUNT(DISTINCT S), COUNT(DISTINCT P) FROM {t}"), ["7,5"]);
        let extremes = "SELECT '[' || MIN(S) || ']', '[' || MAX(S) || ']' FROM {t} WHERE S LIKE 'a%' AND S <> 'ab'";
        assert_eq!(of_both(&a, extremes), ["[a],[a ]"]);
        let halves = "SELECT G, '[' || MIN(S) || ']', '[' || MAX(S) || ']' FROM (SELECT CASE WHEN K < 3000 THEN 0 ELSE 1 END AS G, S FROM {t}) AS Q GROUP BY G";
        assert_eq!(of_both(&a, halves), ["0,[],[b]", "1,[],[b]"]);
        let padded = of_both(&a, "SELECT COUNT(*) FROM (SELECT P FROM {t} WHERE P IS NOT NULL GROUP BY P) AS Q");
        assert_eq!(padded, ["5"], "padded: '' and spaces one group, 'a' and 'a ' one");
        // Of nearly unique strings, a group each but the pairs alike.
        let unique = (0..ROWS).map(u_of).collect::<std::collections::HashSet<_>>().len();
        assert_eq!(of_both(&a, "SELECT COUNT(*) FROM (SELECT U FROM {t} GROUP BY U) AS Q"), [unique.to_string()]);
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.2, §10.10] ORDER BY under
/// a collation of NO PAD by octets: a prefix first, so the empty string,
/// spaces, `'a'`, the tab after it, the space after it, `'ab'`, `'b'`;
/// descending the reverse; the first few of a limit the same.
#[test]
fn order_by_is_by_octets_10_10() {
    let (_db, a) = tables();
    let wanted: Vec<String> = STRINGS.iter().map(|s| format!("[{s}]")).collect();
    for table in ["R", "C"] {
        for batches in ["ON", "OFF"] {
            exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
            let sql = format!("SELECT '[' || S || ']' FROM {table} WHERE K BETWEEN 1 AND 7 ORDER BY S");
            assert_eq!(rows(&a, &sql).unwrap(), wanted, "{table} {batches}");
            let mut reversed = wanted.clone();
            reversed.reverse();
            assert_eq!(rows(&a, &format!("{sql} DESC")).unwrap(), reversed, "{table} {batches}");
            let first = format!("SELECT '[' || S || ']' FROM {table} WHERE S LIKE 'a%' ORDER BY S, K FETCH FIRST 1 ROW ONLY");
            assert_eq!(rows(&a, &first).unwrap(), ["[a]"], "{table} {batches}");
            let last = format!("SELECT '[' || S || ']' FROM {table} WHERE S LIKE 'a_' ORDER BY S, K FETCH FIRST 1 ROW ONLY");
            assert_eq!(rows(&a, &last).unwrap(), ["[a\t]"], "{table} {batches}");
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.2, §9.15 SR4] a join on
/// keys under a collation of NO PAD by octets matches equal octets alone;
/// an explicit COLLATE on one side wins over the other's column, padded or
/// unpadded as it says; and two columns of different collations are
/// refused, SQLCODE -103, a row at a time and by batches alike.
#[test]
fn a_join_key_is_unpadded_or_as_collate_says_9_15() {
    let (_db, a) = tables();
    let n = |s: &str| (0..ROWS).filter(|&k| s_of(k) == Some(s)).count();
    let joined = |on: &str| of_both(&a, &format!("SELECT D.V, COUNT(*) FROM {{t}} AS T JOIN D ON {on} GROUP BY D.V"));
    assert_eq!(joined("T.S = D.S"), [format!("1,{}", n("a")), format!("2,{}", n("a "))]);
    // The padded column made unpadded by COLLATE, on either side.
    assert_eq!(joined("T.S = D.X COLLATE A.BYTES"), [format!("1,{}", n("a")), format!("2,{}", n("a "))]);
    assert_eq!(joined("T.P COLLATE A.BYTES = D.S"), [format!("1,{}", n("a")), format!("2,{}", n("a "))]);
    // The unpadded column made padded: 'a' and 'a ' match each of D's rows.
    let both = n("a") + n("a ");
    assert_eq!(joined("T.S COLLATE UCS_BASIC = D.X"), [format!("1,{both}"), format!("2,{both}")]);
    assert_eq!(joined("T.S = D.X COLLATE UCS_BASIC"), [format!("1,{both}"), format!("2,{both}")]);
    for batches in ["ON", "OFF"] {
        exec(&a, &format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        let refused = a.open_cursor("SELECT COUNT(*) FROM C AS T JOIN D ON T.S = D.X").err().map(|e| e.code);
        assert_eq!(refused, Some(Code::SyntaxRule), "SR4e, batches {batches}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.5 SR3d] LIKE never pads,
/// so under a collation that compares each character as itself it is LIKE
/// of the strings as they are: the plan shows the column and the pattern
/// bare, and the rows are those whose octets match.
#[test]
fn like_is_of_the_strings_as_they_are_8_5() {
    let (_db, a) = tables();
    let count = |cond: &str| of_both(&a, &format!("SELECT COUNT(*) FROM {{t}} WHERE {cond}"));
    assert_eq!(count("S LIKE 'a'"), count_of(|s| s == b"a"));
    assert_eq!(count("S LIKE 'a_'"), count_of(|s| s.len() == 2 && s.starts_with(b"a")));
    assert_eq!(count("S LIKE 'a%'"), count_of(|s| s.starts_with(b"a")));
    assert_eq!(count("U LIKE '%/item '"), [(0..ROWS).filter(|k| k.is_multiple_of(5)).count().to_string()]);
    assert_eq!(count("S LIKE '%' || ' '"), count_of(|s| s.ends_with(b" ")));
    let plan = physical(&a, "SELECT COUNT(*) FROM C WHERE U LIKE '%item%'");
    assert!(plan.contains("C.U LIKE '%item%'"), "{plan}");
}

/// Verifies: [DESIGN] RULES.md D1183, D15 (regression: a zone's least and
/// greatest are in the padded order, where `'a'` is above `'a'` and a tab
/// and equal to `'a '`): of groups whose every S is `'a'`, a range or an
/// inequality that holds of every row unpadded keeps every row, no group
/// or part passed over by its zone; an equality outside the zone keeps
/// none.
#[test]
fn a_range_passes_over_no_zone_of_the_padded_order() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE COLLATION BYTES FOR SQL_TEXT FROM UCS_BASIC NO PAD
           CREATE TABLE R (K INTEGER PRIMARY KEY, S VARCHAR(20) COLLATE A.BYTES)
           CREATE TABLE C (K INTEGER PRIMARY KEY, S VARCHAR(20) COLLATE A.BYTES) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["R", "C"] {
        for first in (0..ROWS).step_by(500) {
            let values: Vec<String> = (first..first + 500).map(|k| format!("({k}, 'a')")).collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        a.commit().unwrap();
    }
    db.maintain().unwrap();
    let count = |cond: &str| of_both(&a, &format!("SELECT COUNT(*) FROM {{t}} WHERE {cond}"));
    let all = [ROWS.to_string()];
    assert_eq!(count("S < 'a\t'"), all);
    assert_eq!(count("S <= 'a\t'"), all);
    assert_eq!(count("S < 'a '"), all);
    assert_eq!(count("S <> 'a '"), all);
    assert_eq!(count("S > ''"), all);
    assert_eq!(count("S >= 'a'"), all);
    assert_eq!(count("S NOT IN ('a ', 'a\t')"), all);
    assert_eq!(count("S = 'a '"), ["0"]);
    assert_eq!(count("S > 'a'"), ["0"]);
    assert_eq!(count("S = 'b'"), ["0"]);
    assert_eq!(count("S = 'a'"), all);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §8.2 GR3d] EXPLAIN shows a
/// grouping column with the collation its groups are told apart by where
/// that is not the default: a column of a collation of NO PAD with
/// COLLATE and the name of the collation that orders it, in the plan as
/// bound and as run, and of the first grouping of a DISTINCT set
/// function's pair; a column grouped under the default with none.
#[test]
fn explain_shows_the_collation_a_grouping_groups_under_8_2() {
    let (_db, a) = tables();
    let whole = |sql: &str| a.explain(sql, &turborel::params::Params::new()).unwrap();
    let unpadded = whole("SELECT S, COUNT(*) FROM C GROUP BY S");
    let (bound, run) = unpadded.split_once("-- physical:").unwrap();
    assert!(bound.contains("Aggregate group by [C.S COLLATE UCS_BASIC]"), "as bound: {bound}");
    assert!(run.contains("Aggregate group by [C.S COLLATE UCS_BASIC]"), "as run: {run}");
    let pair = physical(&a, "SELECT COUNT(DISTINCT S) FROM C");
    assert!(pair.contains("Aggregate group by [C.S COLLATE UCS_BASIC] computing []"), "the first of the pair: {pair}");
    let padded = physical(&a, "SELECT P, COUNT(*) FROM C GROUP BY P");
    assert!(padded.contains("group by [C.P] computing"), "the default collation is not named: {padded}");
}

/// Verifies: [DESIGN] RULES.md D1183: the operators that read no string in
/// place still compare sort keys, and so unpadded: a window's partition and
/// order, UNION, INTERSECT and EXCEPT, IN and a quantified comparison of a
/// subquery, a comparison of rows, NULLIF and a simple CASE, GREATEST and
/// LEAST, a grouping streamed in the order of an index, and a unique
/// constraint and its index, which hold `'a'` and `'a '` as two keys.
#[test]
fn the_rest_compare_sort_keys_unpadded() {
    let (db, a) = tables();
    let n = |s: &str| (0..ROWS).filter(|&k| s_of(k) == Some(s)).count();
    let one = |sql: &str| of_both(&a, sql);
    let window =
        "SELECT COUNT(*) FROM (SELECT ROW_NUMBER() OVER (PARTITION BY S ORDER BY K) AS N FROM {t} WHERE S IS NOT NULL) AS Q WHERE N = 1";
    assert_eq!(one(window), ["7"]);
    let ranked = "SELECT MAX(N) FROM (SELECT RANK() OVER (ORDER BY S) AS N FROM {t} WHERE S IN ('a', 'a ')) AS Q";
    assert_eq!(one(ranked), [(n("a") + 1).to_string()]);
    assert_eq!(one("SELECT COUNT(*) FROM (SELECT S FROM {t} UNION SELECT S FROM D) AS Q"), ["8"]);
    assert_eq!(one("SELECT COUNT(*) FROM (SELECT S FROM {t} INTERSECT SELECT S FROM D) AS Q"), ["2"]);
    assert_eq!(one("SELECT COUNT(*) FROM (SELECT S FROM {t} WHERE S IS NOT NULL EXCEPT SELECT S FROM D) AS Q"), ["5"]);
    assert_eq!(one("SELECT COUNT(*) FROM {t} WHERE S IN (SELECT S FROM D WHERE V = 1)"), [n("a").to_string()]);
    assert_eq!(one("SELECT COUNT(*) FROM {t} WHERE S = ANY (SELECT S FROM D WHERE V = 2)"), [n("a ").to_string()]);
    assert_eq!(one("SELECT COUNT(*) FROM {t} WHERE (S, 1) = ('a ', 1)"), [n("a ").to_string()]);
    assert_eq!(one("SELECT COUNT(*) FROM {t} WHERE NULLIF(S, 'a') IS NULL AND S IS NOT NULL"), [n("a").to_string()]);
    assert_eq!(one("SELECT COUNT(*) FROM {t} WHERE CASE S WHEN 'a ' THEN 1 ELSE 0 END = 1"), [n("a ").to_string()]);
    assert_eq!(one("SELECT COUNT(*) FROM {t} WHERE '[' || GREATEST(S, 'a') || ']' = '[a]'"), [(n("") + n("  ") + n("a")).to_string()]);
    assert_eq!(one("SELECT COUNT(*) FROM {t} WHERE '[' || LEAST(S, 'a ') || ']' = '[a ]'"), [(n("a ") + n("ab") + n("b")).to_string()]);
    // An index on S holds the strings' octets: two keys, and a streamed
    // grouping in its order two groups.
    let b = db.session("A");
    b.run("CREATE TABLE UQ (S VARCHAR(20) COLLATE A.BYTES UNIQUE, V INTEGER)").unwrap();
    b.run("INSERT INTO UQ VALUES ('a', 1), ('a ', 2), ('a\t', 3)").unwrap();
    assert_eq!(b.run("INSERT INTO UQ VALUES ('a ', 4)").err().map(|e| e.code), Some(Code::Constraint));
    assert_eq!(rows(&b, "SELECT V FROM UQ WHERE S = 'a '").unwrap(), ["2"]);
    assert_eq!(rows(&b, "SELECT V FROM UQ ORDER BY S").unwrap(), ["1", "3", "2"]);
    assert_eq!(rows(&b, "SELECT COUNT(*) FROM (SELECT S FROM UQ GROUP BY S) AS Q").unwrap(), ["3"]);
}

/// Whether `shown`, a plan, has a part run in parallel whose top is two
/// groupings, one over the other.
fn pair_in_a_part(shown: &str) -> bool {
    let lines: Vec<&str> = shown.lines().map(str::trim_start).collect();
    lines.windows(3).any(|w| w[0].starts_with("Parallel") && w[1].starts_with("Aggregate") && w[2].starts_with("Aggregate"))
}

/// The rows whose S is `g`, by their numbers.
fn of(g: &str) -> impl Iterator<Item = usize> + '_ {
    (0..ROWS).filter(move |&k| s_of(k) == Some(g))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.2, §10.9 GR7b i]
/// (RULES.md D1184) a DISTINCT set function beside a grouping column under
/// a collation of NO PAD by octets: its two groupings run as one part in
/// parallel, and `'a'`, `'a '` and a tab after `'a'` are three groups, each
/// counting its own rows' values, as the plan run alone gives them; by
/// batches and a row at a time, of both stores; with a set function no
/// batch reads, whose second grouping is made of rows; of nearly unique
/// strings a space apart; over a projection of the first's groups; and a
/// count of the groups' counts, which crosses the partitions. A padded
/// column beside it is still one group of the two.
#[test]
fn a_distinct_set_function_s_pair_groups_unpadded_10_9() {
    let (_db, a) = tables();
    // K less its remainder by 21 is one value of each 21 rows, as `k / 21`
    // is here: a quotient of exact numbers keeps a scale, so is not it.
    let distinct = |g: &str, by: usize| of(g).map(|k| k.div_euclid(by)).collect::<std::collections::HashSet<_>>().len();
    let mut counted: Vec<String> = STRINGS.iter().map(|g| format!("[{g}],{}", distinct(g, 21))).collect();
    counted.sort();
    let mut both: Vec<String> =
        STRINGS.iter().map(|g| format!("[{g}],{},{}", distinct(g, 21), of(g).filter(|k| k % 2 == 0).count())).collect();
    both.sort();
    // Each U, how many remainders by 3 its rows' K have, and its rows.
    let mut of_u: std::collections::BTreeMap<String, (std::collections::BTreeSet<usize>, usize)> = std::collections::BTreeMap::new();
    for k in 0..ROWS {
        let held = of_u.entry(u_of(k)).or_default();
        held.0.insert(k % 3);
        held.1 += 1;
    }
    let mut by_string: Vec<(String, usize, usize)> = of_u.into_iter().map(|(u, (set, rows))| (u, set.len(), rows)).collect();
    by_string.sort_by_key(|(u, n, _)| format!("{u},{n}"));
    for degree in [1, 4] {
        exec(&a, &format!("SET EXECUTOR.DEGREE = {degree}")).unwrap();
        exec(&a, "SET EXECUTOR.MORSEL_ROWS = 256").unwrap();
        exec(&a, "SET EXECUTOR.PARALLEL_MIN_ROWS = 0").unwrap();
        let by_s = "SELECT '[' || S || ']', COUNT(DISTINCT K - MOD(K, 21)) FROM {t} WHERE S IS NOT NULL GROUP BY S";
        assert_eq!(of_both(&a, by_s), counted, "degree {degree}");
        assert_eq!(pair_in_a_part(&physical(&a, &by_s.replace("{t}", "C"))), degree > 1, "degree {degree}");
        let filtered = "SELECT '[' || S || ']', COUNT(DISTINCT K - MOD(K, 21)), COUNT(*) FILTER (WHERE MOD(K, 2) = 0) FROM {t} WHERE S IS NOT NULL GROUP BY S";
        assert_eq!(of_both(&a, filtered), both, "degree {degree}");
        // Many pairs of strings a space apart, so some share a partition.
        // The argument is no key of the table, so the pair is made.
        let by_u = "SELECT U, COUNT(DISTINCT MOD(K, 3)) FROM {t} GROUP BY U";
        assert_eq!(of_both(&a, by_u), by_string.iter().map(|(u, n, _)| format!("{u},{n}")).collect::<Vec<_>>(), "degree {degree}");
        assert_eq!(pair_in_a_part(&physical(&a, &by_u.replace("{t}", "C"))), degree > 1, "degree {degree}");
        let by_rows = "SELECT U, COUNT(DISTINCT MOD(K, 3)), COUNT(*) FILTER (WHERE K >= 0) FROM {t} GROUP BY U";
        let with_rows: Vec<String> = by_string.iter().map(|(u, n, rows)| format!("{u},{n},{rows}")).collect();
        assert_eq!(of_both(&a, by_rows), with_rows, "degree {degree}");
        // A projection between the two: the second groups by its own key.
        let over = "SELECT '[' || T || ']', COUNT(*) FROM (SELECT SUBSTRING(S FROM 1 FOR 2) AS T, N FROM (SELECT S, N FROM (SELECT S, K - MOD(K, 21) AS N FROM {t} WHERE S IS NOT NULL) AS P GROUP BY S, N) AS Q) AS W GROUP BY T";
        assert_eq!(of_both(&a, over), counted, "degree {degree}");
        let crossing = "SELECT COUNT(*) FROM (SELECT N, COUNT(*) AS M FROM (SELECT S, COUNT(*) AS N FROM {t} WHERE S IS NOT NULL GROUP BY S) AS Q GROUP BY N) AS W";
        let sizes = STRINGS.iter().map(|g| of(g).count()).collect::<std::collections::HashSet<_>>().len();
        assert_eq!(of_both(&a, crossing), [sizes.to_string()], "degree {degree}");
        let padded = of_both(
            &a,
            "SELECT COUNT(*) FROM (SELECT P, COUNT(DISTINCT K - MOD(K, 21)) AS N FROM {t} WHERE P IS NOT NULL GROUP BY P) AS Q",
        );
        assert_eq!(padded, ["5"], "degree {degree}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.2, §10.9 GR7b i]
/// (RULES.md D1219) COUNT(DISTINCT) of no GROUP BY: its two groupings run
/// as one part in parallel, the second's one group counted of every
/// partition, of strings under a collation of NO PAD by octets, grouped
/// as they are, `'a'` and `'a '` apart; of nearly unique strings; of a
/// padded column, whose keys are grouped by; of an integer; of no rows,
/// one row of 0; under HAVING. Beside SUM(DISTINCT), whose values the
/// second grouping takes in their order, the second stays above the part.
/// Of three groupings one over another, a count of no grouping at the top
/// or a grouped one, the upper two are no part and the query gives its
/// rows. Each gives what the plan run alone gives, of both stores.
#[test]
fn a_count_of_distinct_values_of_no_grouping_is_one_part_10_9() {
    let (_db, a) = tables();
    let strings = (0..ROWS).map(u_of).collect::<std::collections::HashSet<_>>().len();
    let numbers = (0..ROWS).map(|k| k - k % 21).collect::<std::collections::HashSet<_>>();
    let summed: usize = numbers.iter().sum();
    // How many sizes the groups of S have, and how many sizes share a count.
    let size_of = |g: &&str| of(g).count();
    let sizes = STRINGS.iter().map(size_of).collect::<std::collections::HashSet<_>>();
    let of_size = |n: &usize| STRINGS.iter().filter(|g| size_of(g) == *n).count();
    let (sizes, tallies) = (sizes.len(), sizes.iter().map(of_size).collect::<std::collections::HashSet<_>>().len());
    for degree in [1, 4] {
        for set in [
            format!("SET EXECUTOR.DEGREE = {degree}"),
            "SET EXECUTOR.MORSEL_ROWS = 256".into(),
            "SET EXECUTOR.PARALLEL_MIN_ROWS = 0".into(),
        ] {
            exec(&a, &set).unwrap();
        }
        for (sql, expected) in [
            ("SELECT COUNT(DISTINCT S) FROM {t}", "7".to_owned()),
            ("SELECT COUNT(DISTINCT U) FROM {t}", strings.to_string()),
            ("SELECT COUNT(DISTINCT P) FROM {t}", "5".to_owned()),
            ("SELECT COUNT(DISTINCT K - MOD(K, 21)) FROM {t}", numbers.len().to_string()),
            ("SELECT COUNT(DISTINCT S) FROM {t} WHERE U = 'none'", "0".to_owned()),
        ] {
            assert_eq!(of_both(&a, sql), [expected], "{sql}, degree {degree}");
            let shown = physical(&a, &sql.replace("{t}", "C"));
            assert_eq!(pair_in_a_part(&shown), degree > 1, "{sql}, degree {degree}\n{shown}");
        }
        assert_eq!(of_both(&a, "SELECT COUNT(DISTINCT S) FROM {t} HAVING COUNT(DISTINCT S) > 7"), Vec::<String>::new());
        assert_eq!(of_both(&a, "SELECT COUNT(DISTINCT S) FROM {t} HAVING COUNT(DISTINCT S) > 6"), ["7"]);
        // Three groupings, one over another: the upper two are no part,
        // as the middle one reads no pipeline, and the lower two are.
        let three = "SELECT COUNT(*) FROM (SELECT N, COUNT(*) AS M FROM (SELECT S, COUNT(*) AS N FROM {t} WHERE S IS NOT NULL GROUP BY S) AS Q GROUP BY N) AS W";
        assert_eq!(of_both(&a, three), [sizes.to_string()], "degree {degree}");
        let grouped = "SELECT COUNT(*) FROM (SELECT M, COUNT(*) AS L FROM (SELECT N, COUNT(*) AS M FROM (SELECT S, COUNT(*) AS N FROM {t} WHERE S IS NOT NULL GROUP BY S) AS Q GROUP BY N) AS W GROUP BY M) AS V";
        assert_eq!(of_both(&a, grouped), [tallies.to_string()], "degree {degree}");
        let beside = "SELECT COUNT(DISTINCT S), SUM(DISTINCT K - MOD(K, 21)) FROM {t}";
        assert_eq!(of_both(&a, beside), [format!("7,{summed}")], "degree {degree}");
        assert!(!pair_in_a_part(&physical(&a, &beside.replace("{t}", "C"))), "SUM in the second keeps it above the part");
    }
}
