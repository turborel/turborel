//! A grouping over a LEFT outer join grouped first below it (RULES.md
//! D1112, `optimizer/pregrouped.rs`).

use turborel::optimizer::GROUP_BEFORE_OUTER_JOIN;

use turborel::db::{Database, Session};

use super::{COLUMNS, alike, left, plan, tables};

/// Schema Q's tables, and of each store L of 80 rows, ids repeated, null
/// and of no order, O of 2,000 orders of 50 of those ids, its columns but
/// N declared to hold no null, and P of an order an id; their statistics
/// gathered.
fn with_orders() -> (Database, Session) {
    let (db, s) = tables();
    for (prefix, store) in [("", ""), (COLUMNS, " USING columnstore")] {
        s.run(&format!("CREATE TABLE {prefix}L (ID INTEGER, G INTEGER NOT NULL){store}")).unwrap();
        let not_null = "K INTEGER NOT NULL, C INTEGER NOT NULL, V INTEGER NOT NULL, N INTEGER, S VARCHAR(10) NOT NULL";
        s.run(&format!("CREATE TABLE {prefix}O ({not_null}){store}")).unwrap();
        s.run(&format!("CREATE TABLE {prefix}P (K INTEGER NOT NULL, C INTEGER NOT NULL){store}")).unwrap();
        let ids: Vec<String> =
            (0..80).map(|i| if i % 13 == 5 { format!("(NULL, {})", i % 4) } else { format!("({}, {})", i % 70, i % 4) }).collect();
        s.run(&format!("INSERT INTO {prefix}L VALUES {}", ids.join(", "))).unwrap();
        let orders: Vec<String> = (0..2000)
            .map(|k| {
                let n = if k % 9 == 0 { "NULL".to_owned() } else { (k % 5).to_string() };
                format!("({k}, {}, {}, {n}, 's{}')", (k * 7) % 50, k % 11, k % 3)
            })
            .collect();
        s.run(&format!("INSERT INTO {prefix}O VALUES {}", orders.join(", "))).unwrap();
        s.run(&format!("INSERT INTO {prefix}P SELECT K, K FROM {prefix}O")).unwrap();
        s.commit().unwrap();
        for table in ["L", "O", "P"] {
            db.analyze(&format!("Q.{prefix}{table}")).unwrap();
        }
    }
    db.maintain().unwrap();
    (db, s)
}

/// Verifies: [ISO/IEC 9075:1992 §7.5 GR5, §6.5 GR1]; [DESIGN] RULES.md
/// D1112: COUNT, MAX and MIN of columns that hold no null of the right
/// operand of a LEFT join on equalities, grouped by the left's columns,
/// give the same rows and the same warning of a left row nothing matched
/// with the right operand grouped first by its keys, of either store: by
/// one column and by another the join does not read, with a filter of the
/// right operand's own, under a HAVING condition, and under a second
/// grouping of the counts.
#[test]
fn a_grouping_over_a_left_join_is_grouped_first_below_it() {
    let (_db, s) = with_orders();
    for sql in [
        "SELECT L.ID, COUNT(O.K) FROM {L} L LEFT JOIN {O} O ON L.ID = O.C GROUP BY L.ID",
        "SELECT L.G, COUNT(O.K), MIN(O.V), MAX(O.K) FROM {L} L LEFT JOIN {O} O ON L.ID = O.C GROUP BY L.G",
        "SELECT L.ID, L.G, COUNT(O.S) FROM {L} L LEFT JOIN {O} O ON O.C = L.ID AND O.V > 6 GROUP BY L.ID, L.G",
        "SELECT L.ID, MAX(O.V) FROM {L} L LEFT JOIN {O} O ON L.ID = O.C AND L.G = O.V GROUP BY L.ID",
        "SELECT L.ID, COUNT(O.K) FROM {L} L LEFT JOIN {O} O ON L.ID = O.C GROUP BY L.ID HAVING COUNT(O.K) = 0",
        "SELECT N, COUNT(*) FROM (SELECT L.ID, COUNT(O.K) FROM {L} L LEFT JOIN {O} O ON L.ID = O.C AND O.S <> 's1' GROUP BY L.ID) \
         AS X (ID, N) GROUP BY N",
    ] {
        alike(&s, GROUP_BEFORE_OUTER_JOIN, sql);
    }
    let shown = plan(&s, "SELECT L.ID, COUNT(O.K) FROM L LEFT JOIN O ON L.ID = O.C GROUP BY L.ID");
    let physical = shown.split("-- physical:").nth(1).unwrap_or_default();
    let (above, below) = physical.split_once("LeftOuterJoin").unwrap_or_default();
    assert!(above.contains("SUM(COUNT(O.K))") && below.contains("Aggregate group by [O.C] computing [COUNT(O.K)]"), "{shown}");
}

/// Verifies: [DESIGN] RULES.md D1112, D15: the rule leaves a grouping
/// whose rewrite could change a row, a warning or an error: COUNT(*),
/// which counts a left row nothing matched; a column that may hold a
/// null; SUM; a DISTINCT or filtered function; MIN of character strings;
/// an expression; a grouping by the right operand's column; a condition
/// that is no equality of columns; an inner join; and a right operand of
/// a row a key, which grouping first would not shrink.
#[test]
fn a_grouping_the_rewrite_could_change_is_left() {
    let (_db, s) = with_orders();
    for sql in [
        "SELECT L.ID, COUNT(*) FROM {L} L LEFT JOIN {O} O ON L.ID = O.C GROUP BY L.ID",
        "SELECT L.ID, COUNT(O.N) FROM {L} L LEFT JOIN {O} O ON L.ID = O.C GROUP BY L.ID",
        "SELECT L.ID, SUM(O.V) FROM {L} L LEFT JOIN {O} O ON L.ID = O.C GROUP BY L.ID",
        "SELECT L.ID, COUNT(DISTINCT O.V) FROM {L} L LEFT JOIN {O} O ON L.ID = O.C GROUP BY L.ID",
        "SELECT L.ID, COUNT(O.K) FILTER (WHERE O.V > 2) FROM {L} L LEFT JOIN {O} O ON L.ID = O.C GROUP BY L.ID",
        "SELECT L.ID, MIN(O.S) FROM {L} L LEFT JOIN {O} O ON L.ID = O.C GROUP BY L.ID",
        "SELECT L.ID, COUNT(O.K + 1) FROM {L} L LEFT JOIN {O} O ON L.ID = O.C GROUP BY L.ID",
        "SELECT O.C, COUNT(O.K) FROM {L} L LEFT JOIN {O} O ON L.ID = O.C GROUP BY O.C",
        "SELECT L.ID, COUNT(O.K) FROM {L} L LEFT JOIN {O} O ON L.ID = O.C AND O.V > L.G GROUP BY L.ID",
        "SELECT L.ID, COUNT(O.K) FROM {L} L JOIN {O} O ON L.ID = O.C GROUP BY L.ID",
        "SELECT L.ID, COUNT(P.K) FROM {L} L LEFT JOIN {P} P ON L.ID = P.C GROUP BY L.ID",
    ] {
        left(&s, GROUP_BEFORE_OUTER_JOIN, sql);
    }
}
