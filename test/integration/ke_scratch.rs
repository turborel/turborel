//! Scratch, never committed: kept_entries' race, many rounds a process.

use std::thread;

use turborel::db::Database;

use crate::common::rows;

const ROWS: usize = 200;

fn setup() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER NOT NULL, X INTEGER, Y INTEGER)").unwrap();
    db.create_index("T_X", "APP.T", &["X"], "btree").unwrap();
    let s = db.session("APP");
    let values: Vec<String> = (0..ROWS).map(|i| format!("({i}, {i}, 0)")).collect();
    s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
    db
}

fn round(writers: usize, delay_us: u64) -> Result<(), String> {
    let db = setup();
    let threads: Vec<_> = (0..writers)
        .map(|t| {
            let db = db.clone();
            thread::spawn(move || {
                let s = db.session("APP");
                let mut errors: Vec<String> = Vec::new();
                for n in 0..150 {
                    let sql = format!("UPDATE T SET Y = Y + 1 WHERE ID = {}", (n * 7 + t) % ROWS);
                    loop {
                        match s.run(&sql).and_then(|_| s.commit()) {
                            Ok(_) => break,
                            Err(e) => {
                                if !format!("{e:?}").contains("Conflict") {
                                    errors.push(format!("{e:?}"));
                                }
                                if let Err(r) = s.rollback() {
                                    errors.push(format!("ROLLBACK {r:?}"));
                                }
                            }
                        }
                    }
                }
                errors
            })
        })
        .collect();
    thread::sleep(std::time::Duration::from_micros(delay_us));
    db.create_index("T_X2", "APP.T", &["X"], "btree").unwrap();
    let mut errors = Vec::new();
    for t in threads {
        errors.extend(t.join().unwrap());
    }
    db.maintain().unwrap();
    let s = db.session("APP");
    let updated = s.run("UPDATE T SET X = X + 1000").and_then(|_| s.commit());
    if !errors.is_empty() || updated.is_err() {
        return Err(format!("writers {:?} update {updated:?}", errors.iter().take(5).collect::<Vec<_>>()));
    }
    db.maintain().unwrap();
    let sum = rows(&s, "SELECT SUM(Y) FROM T");
    if sum != Ok(vec![(writers * 150).to_string()]) {
        return Err(format!("sum {sum:?}"));
    }
    Ok(())
}

#[test]
fn ke_rounds() {
    let rounds: usize = std::env::var("KE_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(50);
    let mut failed = 0;
    for i in 0..rounds {
        let writers = 3 + i % 4;
        let delay = [0, 200, 1000, 3000][i % 4];
        if let Err(e) = round(writers, delay) {
            failed += 1;
            eprintln!("KEROUND {i} writers={writers} delay={delay}: {e}");
        }
    }
    eprintln!("KEROUNDS {rounds} failed {failed}");
    assert_eq!(failed, 0);
}
