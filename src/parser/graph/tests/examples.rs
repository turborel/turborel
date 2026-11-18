//! Unit tests of the patterns of P1's examples and of the graph
//! standard's examples and conformance tests T1 to T12 and T20 to T22:
//! each reads, and each the Syntax Rules accept is accepted, each they
//! refuse refused with its GRF condition. P1's "5M" is written 5000000,
//! its "−" as "-", and `COUNT(e.*)` as COUNT(e).

use super::*;

/// Verifies: [EXTENSION] [P1 §4.1 to §4.3] the node, edge, path and
/// graph patterns of P1's examples are accepted.
#[test]
fn p1_element_and_path_examples_4() {
    for text in [
        "(x:Account WHERE x.isBlocked='no')",
        "-[e:Transfer WHERE e.amount>5000000]->",
        "(x)",
        "(x:Account)",
        "(x:Account|IP)",
        "(:!%)",
        "(x:Account) WHERE x.isBlocked='no'",
        "()",
        "(x)-[:Transfer]->()-[:isLocatedIn]->(y)",
        "-[e]->",
        "~[e]~",
        "(x)-[e]->(y)",
        "(y WHERE y.owner='Aretha')<-[e:Transfer]-(x)",
        "(s)-[e]->(m)-[f]->(t)",
        "(p:Phone WHERE p.isBlocked='yes') ~[e:hasPhone]~(a1:Account) -[t:Transfer WHERE t.amount>1000000]->(a2)",
        "(s)-[:Transfer]->(s1)-[:Transfer]->(s2)-[:Transfer]->(s)",
        "p = (s)-[:Transfer]->(s1)-[:Transfer]->(s2)-[:Transfer]->(s)",
        "(p:Phone)~[:hasPhone]~(s:Account)-[t:Transfer]-> (d:Account)~[:hasPhone]~(p)",
        "(p:Phone WHERE p.isBlocked='yes')~[:hasPhone]~(s:Account), (s)-[t:Transfer WHERE t.amount>1000000]->()",
        "(s:Account)-[:SignInWithIP]-(), (s)-[t:Transfer WHERE t.amount>1000000]->(), (s)~[:hasPhone]~(p:Phone WHERE p.isBlocked='yes')",
        "(x:Account)-[:isLocatedIn]->(g:City)<-[:isLocatedIn]-(y:Account), ANY (x)-[e:Transfer]->+(y) \
         WHERE x.isBlocked='no' AND y.isBlocked='yes' AND g.name='Ankh-Morpork'",
    ] {
        accepted(text);
    }
}

/// Verifies: [EXTENSION] [P1 §4.4 to §4.6] P1's quantified, union and
/// conditional examples: those it calls legal are accepted, its illegal
/// join refused with GRF-0021.
#[test]
fn p1_quantifier_and_union_examples_4_4() {
    for text in [
        "(a:Account)-[:Transfer]->{2,5}(b:Account)",
        "[(a:Account)-[:Transfer]->(b:Account) WHERE a.owner=b.owner]{2,5}",
        "(a:Account) [()-[t:Transfer]->() WHERE t.amount>1000000]{2,5} (b:Account)",
        "(a:Account) [()-[t:Transfer]->() WHERE t.amount>1000000]{2,5} (b:Account) WHERE SUM(t.amount)>10000000",
        "(c:City) | (c:Country)",
        "(c:City) |+| (c:Country)",
        "->{1,5} | ->{3,7}",
        "->{1,7}",
        "[(x)->(y)] | [(x)->(z)]",
        "(x) [->(y)]?",
        "[(x:Account)-[:Transfer]->(y:Account WHERE y.isBlocked='yes')] | \
         [(x:Account)-[:Transfer]->()-[:hasPhone]-(p WHERE p.isBlocked='yes')]",
        "(x:Account)-[:Transfer]->(y:Account) [-(:hasPhone)-(p)]? WHERE y.isBlocked='yes' OR p.isBlocked='yes'",
    ] {
        accepted(text);
    }
    assert_eq!(grf("[(x)->(y)] | [(x)->(z)], (y)->(w)"), "GRF-0021");
}

/// Verifies: [EXTENSION] [P1 §5, §6, §7] P1's restrictor, selector,
/// prefilter and KEEP examples: its unterminating one is GRF-0031, its
/// prefilter aggregate GRF-0032, and the rest are accepted.
#[test]
fn p1_termination_examples_5() {
    assert_eq!(grf("p = (a WHERE a.owner='Dave')-[t:Transfer]->* (b WHERE b.owner='Aretha')"), "GRF-0031");
    assert_eq!(grf("ALL SHORTEST [ (x)-[e]->*(y) WHERE COUNT(e)/(COUNT(e)+1)>1 ]"), "GRF-0032");
    for text in [
        "TRAIL p = (a WHERE a.owner='Dave')-[t:Transfer]->* (b WHERE b.owner='Aretha')",
        "ANY SHORTEST p = (a WHERE a.owner='Dave')-[t:Transfer]->*(b WHERE b.owner='Aretha')",
        "ALL SHORTEST TRAIL p = (a WHERE a.owner='Dave')-[t:Transfer]->* \
         (b WHERE b.owner='Aretha')-[r:Transfer]->*(c WHERE c.owner='Mike')",
        "(p:Account WHERE p.owner='Natalia')->{1,10} (q:Account WHERE q.owner='Mike')->{1,10} (r:Account WHERE r.owner='Scott')",
        "ALL SHORTEST (p:Account WHERE p.owner='Scott')->+ (q:Account WHERE q.isBlocked='yes')->+ (r:Account WHERE r.owner='Charles')",
        "ALL SHORTEST (p:Account WHERE p.owner='Scott')->+ (q:Account)->+ (r:Account WHERE r.owner='Charles') WHERE q.isblocked='yes'",
        "ALL SHORTEST (x)-[e]->*(y) WHERE COUNT(e)/(COUNT(e)+1) > 1",
        "ALL SHORTEST [ TRAIL (x)-[e]->*(y) WHERE COUNT(e)/(COUNT(e)+1) > 1 ]",
        "TRAIL (a WHERE a.owner='Jay') [-[b:Transfer WHERE b.amount>5000000]->]+ \
         (a) [-[:isLocatedIn]->(c:City) | -[:isLocatedIn]->(c:Country)]",
        "TRAIL (a WHERE a.owner='Jay') [-[b:Transfer WHERE b.amount>5000000]->]+ (a)-[:isLocatedIn]->(c:City|Country)",
        "ALL SHORTEST (a WHERE a.owner='Jay') [-[b:Transfer WHERE b.amount>5000000]->]+ \
         (a) [-[:isLocatedIn]->(c:City) | -[:isLocatedIn]->(c:Country)]",
        "(x)-[e]->*(y) WHERE AVG(e.a)<1 KEEP ANY SHORTEST",
    ] {
        accepted(text);
    }
}

/// Verifies: [EXTENSION] [Graph standard §23.2 T1 to T12] the conformance
/// tests' patterns: T4 is GRF-0031 and T12 GRF-0032 in brackets, the
/// rest accepted.
#[test]
fn conformance_tests_t1_to_t12_23_2() {
    let ends = "(a WHERE a.owner='Dave')-[t:Transfer]->* (b WHERE b.owner='Aretha')";
    for prefix in ["TRAIL p =", "ACYCLIC p =", "ANY SHORTEST p ="] {
        accepted(&format!("{prefix} {ends}"));
    }
    assert_eq!(grf(&format!("WALK p = {ends}")), "GRF-0031", "T4");
    accepted("(c:City) | (c:Country)");
    accepted("(c:City) |+| (c:Country)");
    accepted("ALL SHORTEST (p WHERE owner='Scott')->+ (q WHERE is_blocked='yes')->+(r WHERE owner='Charles')");
    accepted("ALL SHORTEST (p WHERE owner='Scott')->+ (q)->+(r WHERE owner='Charles') WHERE q.is_blocked='yes'");
    for union in ["|", "|+|"] {
        accepted(&format!(
            "TRAIL (a WHERE a.owner='Jay') [-[b:Transfer WHERE b.amount>5000000]->]+ \
             (a) [-[:isLocatedIn]->(c:City) {union} -[:isLocatedIn]->(c:Country)]"
        ));
    }
    accepted("(p:Phone)~[:hasPhone]~(s:Account)-[t:Transfer]-> (d:Account)~[:hasPhone]~(p)");
    accepted("(x)-[e]-(y)");
    assert_eq!(grf("ALL SHORTEST [(x)-[e]->*(y) WHERE COUNT(e)/(COUNT(e)+1) > 1]"), "GRF-0032", "T12");
    accepted("ALL SHORTEST (x)-[e]->*(y) WHERE COUNT(e)/(COUNT(e)+1) > 1");
}

/// Verifies: [EXTENSION] [Graph standard §11, §23.3 T20 to T22] the
/// extension tests' forms: PREV in a quantified prefilter, FOR EACH
/// SEGMENT, LINE_GRAPH and EXISTS IN PATH; the standard's examples, which
/// write no selector, read, and are GRF-0031 until one is written.
#[test]
fn extension_tests_t20_to_t22_23_3() {
    let prev = "(a) [ -[t:Transfer]-> WHERE PREV(t) IS NULL OR t.ts > PREV(t).ts ]+ (b)";
    assert!(pattern(prev).is_ok());
    assert_eq!(grf(prev), "GRF-0031");
    accepted(&format!("ANY SHORTEST {prev}"));
    accepted("ANY p = (a)->+(b) FOR EACH SEGMENT (e1, e2) IN p WHERE e1.ts < e2.ts");
    let read = table("GRAPH_TABLE (LINE_GRAPH(bank, Transfer) MATCH ANY (x)->+(y) WHERE x.ts < y.ts COLUMNS (x.ts AS T))");
    assert!(read.is_ok(), "{read:?}");
    let in_path = "p = (x:L1)->+(y:L2) WHERE NOT EXISTS IN PATH p ( -[u]->()-[v]-> WHERE u.k >= v.k )";
    assert!(pattern(in_path).is_ok());
    assert_eq!(grf(in_path), "GRF-0031");
    accepted(&format!("ANY {in_path}"));
}

/// Verifies: [EXTENSION] [Graph standard §8, §19] the standard's
/// GRAPH_TABLE examples read and are accepted: every clause of §8 with a
/// rows clause, LISTAGG with the WITHIN GROUP SQL:2023 needs, and §19's
/// subquery in a prefilter.
#[test]
fn graph_table_examples_8() {
    let columns = "COLUMNS ( a.owner AS src, b.owner AS dst, PATH_LENGTH(p) AS hops, SUM(t.amount) AS total, \
        LISTAGG(t.txn_id, ',') WITHIN GROUP (ORDER BY t.txn_id) AS txns, ELEMENT_ID(a) AS a_id, a.* )";
    let head = "GRAPH_TABLE ( bank MATCH ANY SHORTEST p = (a IS Account WHERE a.owner = 'Dave') -[t IS Transfer]->+ \
        (b IS Account WHERE b.owner = 'Aretha') WHERE b.is_blocked = 'no'";
    for rows in ["", "ONE ROW PER MATCH", "ONE ROW PER VERTEX (v)", "ONE ROW PER STEP (v1, e, v2)"] {
        let text = format!("{head} {rows} {columns} ) AS t");
        assert!(table(&text).is_ok(), "{text}: {:?}", table(&text));
    }
    let text = format!("{head} {columns} ) AS t AS OF TIMESTAMP CURRENT_TIMESTAMP");
    assert!(table(&text).is_ok(), "{text}: {:?}", table(&text));
    let seed = "GRAPH_TABLE (kg MATCH (d:Doc WHERE d.id IN (SELECT id FROM docs ORDER BY id FETCH FIRST 10 ROWS ONLY))-[:CITES]->{1,2}(x) \
        COLUMNS (x.id))";
    assert!(table(seed).is_ok(), "{:?}", table(seed));
}
