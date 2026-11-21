//! A TPC-H database scaled down, its tables as the benchmark's DDL makes
//! them, keys and the indexes of their foreign keys, and its rows drawn
//! from a fixed seed with the benchmark's shares: suppliers spread over 25
//! nations, the parts' names of five of 92 colours, half the orders
//! finished, and most lines received after their commit date.

use turborel::db::Database;
use turborel::threads::JobPolicy;

use crate::common::Rng;

/// Schema TPCH: the benchmark's eight tables and its indexes.
const TABLES: &str = "CREATE SCHEMA AUTHORIZATION TPCH
    CREATE TABLE REGION (R_REGIONKEY INTEGER NOT NULL, R_NAME CHAR(25), R_COMMENT VARCHAR(152), CONSTRAINT REGION_PK PRIMARY KEY (R_REGIONKEY))
    CREATE TABLE NATION (N_NATIONKEY INTEGER NOT NULL, N_NAME CHAR(25), N_REGIONKEY INTEGER, N_COMMENT VARCHAR(152),
        CONSTRAINT NATION_PK PRIMARY KEY (N_NATIONKEY))
    CREATE TABLE SUPPLIER (S_SUPPKEY INTEGER NOT NULL, S_NAME CHAR(25), S_ADDRESS VARCHAR(40), S_NATIONKEY INTEGER, S_PHONE CHAR(15),
        S_ACCTBAL DECIMAL(15,2), S_COMMENT VARCHAR(102), CONSTRAINT SUPPLIER_PK PRIMARY KEY (S_SUPPKEY))
    CREATE TABLE PART (P_PARTKEY INTEGER NOT NULL, P_NAME VARCHAR(55), P_MFGR CHAR(25), P_BRAND CHAR(10), P_TYPE VARCHAR(25),
        P_SIZE INTEGER, P_CONTAINER CHAR(10), P_RETAILPRICE DECIMAL(15,2), P_COMMENT VARCHAR(23), CONSTRAINT PART_PK PRIMARY KEY (P_PARTKEY))
    CREATE TABLE PARTSUPP (PS_PARTKEY INTEGER NOT NULL, PS_SUPPKEY INTEGER NOT NULL, PS_AVAILQTY INTEGER, PS_SUPPLYCOST DECIMAL(15,2) NOT NULL,
        PS_COMMENT VARCHAR(199), CONSTRAINT PARTSUPP_PK PRIMARY KEY (PS_PARTKEY, PS_SUPPKEY))
    CREATE TABLE ORDERS (O_ORDERKEY BIGINT NOT NULL, O_CUSTKEY INTEGER NOT NULL, O_ORDERSTATUS CHAR(1), O_TOTALPRICE DECIMAL(15,2),
        O_ORDERDATE DATE, O_ORDERPRIORITY CHAR(15), O_CLERK CHAR(15), O_SHIPPRIORITY INTEGER, O_COMMENT VARCHAR(79),
        CONSTRAINT ORDERS_PK PRIMARY KEY (O_ORDERKEY))
    CREATE TABLE LINEITEM (L_ORDERKEY BIGINT NOT NULL, L_PARTKEY INTEGER NOT NULL, L_SUPPKEY INTEGER NOT NULL, L_LINENUMBER INTEGER NOT NULL,
        L_QUANTITY DECIMAL(15,2) NOT NULL, L_EXTENDEDPRICE DECIMAL(15,2) NOT NULL, L_DISCOUNT DECIMAL(15,2) NOT NULL,
        L_TAX DECIMAL(15,2) NOT NULL, L_RETURNFLAG CHAR(1), L_LINESTATUS CHAR(1), L_SHIPDATE DATE, L_COMMITDATE DATE,
        L_RECEIPTDATE DATE, L_SHIPINSTRUCT CHAR(25), L_SHIPMODE CHAR(10), L_COMMENT VARCHAR(44),
        CONSTRAINT LINEITEM_PK PRIMARY KEY (L_ORDERKEY, L_LINENUMBER))";

/// The benchmark's indexes of foreign keys: name, table and columns.
const INDEXES: &[(&str, &str, &[&str])] = &[
    ("LINEITEM_PART_SUPP_FKIDX", "TPCH.LINEITEM", &["L_PARTKEY", "L_SUPPKEY"]),
    ("PARTSUPP_SUPPLIER_FKIDX", "TPCH.PARTSUPP", &["PS_SUPPKEY"]),
    ("SUPPLIER_NATION_FKIDX", "TPCH.SUPPLIER", &["S_NATIONKEY"]),
];

/// The tables, in the order they are filled.
pub const ALL: &[&str] = &["REGION", "NATION", "SUPPLIER", "PART", "PARTSUPP", "ORDERS", "LINEITEM"];

/// The benchmark's nations and their regions.
const NATIONS: [(&str, usize); 25] = [
    ("ALGERIA", 0),
    ("ARGENTINA", 1),
    ("BRAZIL", 1),
    ("CANADA", 1),
    ("EGYPT", 4),
    ("ETHIOPIA", 0),
    ("FRANCE", 3),
    ("GERMANY", 3),
    ("INDIA", 2),
    ("INDONESIA", 2),
    ("IRAN", 4),
    ("IRAQ", 4),
    ("JAPAN", 2),
    ("JORDAN", 4),
    ("KENYA", 0),
    ("MOROCCO", 0),
    ("MOZAMBIQUE", 0),
    ("PERU", 1),
    ("CHINA", 2),
    ("ROMANIA", 3),
    ("SAUDI ARABIA", 4),
    ("VIETNAM", 2),
    ("RUSSIA", 3),
    ("UNITED KINGDOM", 3),
    ("UNITED STATES", 1),
];

/// The benchmark's 92 colours, five of which name a part.
const COLOURS: &[&str] = &[
    "almond",
    "antique",
    "aquamarine",
    "azure",
    "beige",
    "bisque",
    "black",
    "blanched",
    "blue",
    "blush",
    "brown",
    "burlywood",
    "burnished",
    "chartreuse",
    "chiffon",
    "chocolate",
    "coral",
    "cornflower",
    "cornsilk",
    "cream",
    "cyan",
    "dark",
    "deep",
    "dim",
    "dodger",
    "drab",
    "firebrick",
    "floral",
    "forest",
    "frosted",
    "gainsboro",
    "ghost",
    "goldenrod",
    "green",
    "grey",
    "honeydew",
    "hot",
    "indian",
    "ivory",
    "khaki",
    "lace",
    "lavender",
    "lawn",
    "lemon",
    "light",
    "lime",
    "linen",
    "magenta",
    "maroon",
    "medium",
    "metallic",
    "midnight",
    "mint",
    "misty",
    "moccasin",
    "navajo",
    "navy",
    "olive",
    "orange",
    "orchid",
    "pale",
    "papaya",
    "peach",
    "peru",
    "pink",
    "plum",
    "powder",
    "puff",
    "purple",
    "red",
    "rose",
    "rosy",
    "royal",
    "saddle",
    "salmon",
    "sandy",
    "seashell",
    "sienna",
    "sky",
    "slate",
    "smoke",
    "snow",
    "spring",
    "steel",
    "tan",
    "thistle",
    "tomato",
    "turquoise",
    "violet",
    "wheat",
    "white",
    "yellow",
];

/// The sizes of the tables, a scale of the benchmark's.
pub struct Scale {
    pub suppliers: usize,
    pub parts: usize,
    pub orders: usize,
}

/// The database of `scale`, its rows committed, and no statistics gathered
/// until a test asks (JOBS.POLICY MANUAL).
pub fn load(scale: &Scale) -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(TABLES).unwrap();
    for (name, table, columns) in INDEXES {
        db.create_index(name, table, columns, "btree").unwrap();
    }
    let s = db.session("TPCH");
    let mut rng = Rng::new(9);
    for (table, rows) in ALL.iter().zip(rows(scale, &mut rng)) {
        for chunk in rows.chunks(500) {
            s.run(&format!("INSERT INTO {table} VALUES {}", chunk.join(", "))).unwrap();
        }
    }
    s.commit().unwrap();
    db
}

/// Each table's rows, in the order of `ALL`.
fn rows(scale: &Scale, rng: &mut Rng) -> Vec<Vec<String>> {
    let regions = ["AFRICA", "AMERICA", "ASIA", "EUROPE", "MIDDLE EAST"];
    let region = regions.iter().enumerate().map(|(k, n)| format!("({k}, '{n}', 'r')")).collect();
    let nation = NATIONS.iter().enumerate().map(|(k, (n, r))| format!("({k}, '{n}', {r}, 'n')")).collect();
    let supplier = (1..=scale.suppliers).map(|k| format!("({k}, 'Supplier#{k}', 'a', {}, 'p', 100.00, 'c')", rng.below(25))).collect();
    let part = (1..=scale.parts)
        .map(|k| {
            let name: Vec<&str> = (0..5_usize).map(|_| rng.pick(COLOURS)).collect();
            format!("({k}, '{}', 'M', 'B', 'T', {}, 'C', 900.00, 'c')", name.join(" "), 1 + rng.below(50))
        })
        .collect();
    let partsupp = (1..=scale.parts)
        .flat_map(|p| (0..4_usize).map(move |i| (p, supplier_of(p, i, scale.suppliers))))
        .map(|(p, s)| format!("({p}, {s}, 10, {}.00, 'c')", 1 + p % 100))
        .collect();
    let (orders, lines) = orders(scale, rng);
    vec![region, nation, supplier, part, partsupp, orders, lines]
}

/// The supplier `i` of four of part `p`, as the benchmark spreads them.
fn supplier_of(p: usize, i: usize, suppliers: usize) -> usize {
    (p + i * (suppliers.div_euclid(4) + (p - 1).div_euclid(suppliers))) % suppliers + 1
}

/// The orders and their lines: one to seven each, of a part and one of
/// its four suppliers.
fn orders(scale: &Scale, rng: &mut Rng) -> (Vec<String>, Vec<String>) {
    let (mut orders, mut lines) = (Vec::new(), Vec::new());
    for o in 1..=scale.orders {
        let ordered = rng.below(2_400);
        let status = ["F", "F", "O", "O", "P"][rng.below(5)];
        let status = if status == "P" && rng.chance(80) { ["F", "O"][rng.below(2)] } else { status };
        orders.push(format!("({o}, 1, '{status}', 1000.00, {}, '1-URGENT', 'Clerk', 0, 'c')", date(ordered)));
        for l in 1..=1 + rng.below(7) {
            let part = 1 + rng.below(scale.parts);
            let supplier = supplier_of(part, rng.below(4), scale.suppliers);
            let shipped = ordered + 1 + rng.below(121);
            let (committed, received) = (ordered + 30 + rng.below(61), shipped + 1 + rng.below(30));
            lines.push(format!(
                "({o}, {part}, {supplier}, {l}, {}.00, 1000.00, 0.05, 0.02, 'N', 'O', {}, {}, {}, 'NONE', 'MAIL', 'c')",
                1 + rng.below(50),
                date(shipped),
                date(committed),
                date(received)
            ));
        }
    }
    (orders, lines)
}

/// The date `days` after 1 January 1992, as a literal.
fn date(days: usize) -> String {
    let lengths = |y: usize| if y.is_multiple_of(4) { 366 } else { 365 };
    let (mut year, mut left) = (1992, days);
    while left >= lengths(year) {
        left -= lengths(year);
        year += 1;
    }
    let months = [31, if year.is_multiple_of(4) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 0;
    while left >= months[month] {
        left -= months[month];
        month += 1;
    }
    format!("DATE '{year}-{:02}-{:02}'", month + 1, left + 1)
}
