//! Throwaway (fix-memory-key, never committed): TPC-H's schema with its
//! keys, a small scale loaded, the 22 plans printed without statistics and
//! again with every table counted. `FMK_TPCH` names a directory holding
//! ddl.sql, indexes.sql, view.sql, q/turborel/qNN.sql and sf/*.tbl.

use std::fmt::Write as _;

use turborel::db::Database;
use turborel::params::Params;

const TABLES: [&str; 8] = ["REGION", "NATION", "SUPPLIER", "CUSTOMER", "PART", "PARTSUPP", "ORDERS", "LINEITEM"];

/// Each column's type word of `create`, a CREATE TABLE statement.
fn types(create: &str) -> Vec<String> {
    let body = create.split_once('(').map_or("", |(_, b)| b);
    body.split(", ")
        .take_while(|c| !c.starts_with("CONSTRAINT"))
        .filter_map(|c| c.split_whitespace().nth(1))
        .map(|t| t.to_owned())
        .collect()
}

/// A field of a .tbl line as a literal of type `ty`.
fn literal(field: &str, ty: &str) -> String {
    if ty.starts_with("INTEGER") || ty.starts_with("BIGINT") || ty.starts_with("DECIMAL") {
        field.to_owned()
    } else if ty.starts_with("DATE") {
        format!("DATE '{field}'")
    } else {
        format!("'{}'", field.replace('\'', "''"))
    }
}

#[test]
#[ignore = "throwaway, run by hand with FMK_TPCH"]
fn fmk_tpch_plans() {
    let dir = std::env::var("FMK_TPCH").unwrap();
    let read = |p: &str| std::fs::read_to_string(format!("{dir}/{p}")).unwrap();
    let db = Database::new();
    db.set_job_policy(turborel::threads::JobPolicy::Manual).unwrap();
    let ddl: Vec<String> = read("ddl.sql")
        .lines()
        .filter(|l| l.starts_with("CREATE"))
        .map(|l| l.replace("@USING@", &std::env::var("FMK_USING").unwrap_or_default()).trim_end_matches(';').to_owned())
        .collect();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION TPCHC {}", ddl.join(" "))).unwrap();
    let s = db.session("TPCHC");
    for line in read("indexes.sql").lines().filter(|l| l.starts_with("CREATE")) {
        s.run(line.trim_end_matches(';')).unwrap();
    }
    s.run(read("view.sql").trim().trim_end_matches(';')).unwrap();
    s.commit().unwrap();
    for (table, create) in TABLES.iter().zip(&ddl) {
        let tys = types(create);
        let data = read(&format!("sf/{}.tbl", table.to_ascii_lowercase()));
        let rows: Vec<String> = data
            .lines()
            .map(|l| {
                let fields: Vec<String> = l.trim_end_matches('|').split('|').zip(&tys).map(|(f, t)| literal(f, t)).collect();
                format!("({})", fields.join(", "))
            })
            .collect();
        for chunk in rows.chunks(2000) {
            s.run(&format!("INSERT INTO {table} VALUES {}", chunk.join(", "))).unwrap();
        }
        s.commit().unwrap();
        eprintln!("FMK loaded {table} {}", rows.len());
    }
    let plans = |tag: &str| {
        let mut out = String::new();
        for n in 1..=22 {
            let sql = read(&format!("q/turborel/q{n:02}.sql"));
            let plan = s.physical_plan(&sql, &Params::new()).unwrap_or_else(|e| format!("ERROR {e}"));
            let _ = writeln!(out, "=== {tag} q{n:02}\n{plan}");
        }
        out
    };
    print!("{}", plans("none"));
    for table in TABLES {
        db.analyze(&format!("TPCHC.{table}")).unwrap();
    }
    print!("{}", plans("counted"));
}
