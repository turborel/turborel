//! \[EXTENSION\] The object of each kind a starter project serves
//! (RULES.md D263): its word on the command line, its manifest's
//! declaration, the items its source uses, its source on the plug-in SDK
//! (`turborel_plugin`), and how it is added to the `Plugin`. The simple
//! kinds do something small and whole; a kind whose object holds state
//! the database relies on (a target, a storage engine, an index method,
//! credentials, a partitioning strategy, a quantizer) refuses each call
//! until its author writes it, so a starter never pretends to keep data.

/// The object a starter serves of one kind.
#[derive(Debug, Clone, Copy)]
pub(in crate::extension) struct Starter {
    /// The kind's word, as `--kind` takes it.
    pub(in crate::extension) word: &'static str,
    /// Its code in protocol 1, as WELCOME names it.
    pub(in crate::extension) code: u8,
    /// Its declaration in the manifest, without the semicolon.
    pub(in crate::extension) declaration: &'static str,
    /// The paths its source uses, each a `use` of its own.
    pub(in crate::extension) uses: &'static [&'static str],
    /// Its source.
    pub(in crate::extension) source: &'static str,
    /// The call of the `Plugin` that adds it.
    pub(in crate::extension) added: &'static str,
}

/// The kind a starter serves when none is asked for.
pub(in crate::extension) const DEFAULT_KIND: &str = "scalar";

/// Every kind, in the order of their codes.
pub(in crate::extension) const STARTERS: [Starter; 19] = [
    Starter {
        word: "scalar",
        code: 1,
        declaration: "FUNCTION TWICE(INTEGER) RETURNS INTEGER DETERMINISTIC",
        uses: &["turborel::value::Decimal", "turborel::value::Value", "turborel_plugin::Cx", "turborel_plugin::Scalar"],
        source: "/// TWICE: its argument doubled, a null of a null.
struct Twice;

impl Scalar for Twice {
    fn call(&mut self, _cx: &mut Cx<'_>, args: &[Value]) -> Answer<Value> {
        match args.first() {
            Some(Value::Exact(d)) => Ok(Value::Exact(d.checked_mul(Decimal::new(2, 0))?)),
            Some(_) | None => Ok(Value::Null),
        }
    }
}
",
        added: ".scalar(\"TWICE\", Twice)",
    },
    Starter {
        word: "aggregate",
        code: 2,
        declaration: "AGGREGATE TOTAL(DOUBLE PRECISION) RETURNS DOUBLE PRECISION",
        uses: &["turborel::value::Value", "turborel_plugin::Aggregate"],
        source: "/// TOTAL: the sum of its values, its state eight bytes of the sum so far.
struct Total;

/// The sum a state holds, none of an empty state.
fn sum(state: &[u8]) -> Option<f64> {
    state.try_into().ok().map(|b: [u8; 8]| f64::from_le_bytes(b))
}

impl Aggregate for Total {
    fn step(&mut self, state: &mut Vec<u8>, value: &Value) -> Answer<()> {
        if let Value::Approx(x) = value {
            *state = (sum(state).unwrap_or(0.0) + x).to_le_bytes().to_vec();
        }
        Ok(())
    }
    fn merge(&mut self, state: &mut Vec<u8>, other: &[u8]) -> Answer<()> {
        if let Some(x) = sum(other) {
            *state = (sum(state).unwrap_or(0.0) + x).to_le_bytes().to_vec();
        }
        Ok(())
    }
    fn finish(&mut self, state: &[u8]) -> Answer<Value> {
        Ok(sum(state).map_or(Value::Null, Value::Approx))
    }
}
",
        added: ".aggregate(\"TOTAL\", Total)",
    },
    Starter {
        word: "table",
        code: 3,
        declaration: "TABLE FUNCTION COUNTING(INTEGER) RETURNS TABLE (N INTEGER)",
        uses: &["turborel::value::Decimal", "turborel::value::Value", "turborel_plugin::Cx", "turborel_plugin::TableFunction"],
        source: "/// COUNTING(n): the rows 1 to n.
struct Counting;

impl TableFunction for Counting {
    fn rows(&mut self, cx: &mut Cx<'_>, args: &[Value], _read: &[Vec<Vec<Value>>]) -> Answer<Vec<Vec<Value>>> {
        let Some(Value::Exact(n)) = args.first() else { return Ok(Vec::new()) };
        let mut rows = Vec::new();
        for i in 1..=n.units {
            cx.check()?;
            rows.push(vec![Value::Exact(Decimal::new(i, 0))]);
        }
        Ok(rows)
    }
}
",
        added: ".table_function(\"COUNTING\", Counting)",
    },
    Starter {
        word: "procedure",
        code: 4,
        declaration: "PROCEDURE NOTE(VARCHAR(200)) MODIFIES SQL DATA",
        uses: &["turborel::value::Value", "turborel_plugin::Cx", "turborel_plugin::Procedure"],
        source: "/// NOTE(text): inserts its text into the caller's table NOTES, in the
/// caller's transaction.
struct Note;

impl Procedure for Note {
    fn call(&mut self, cx: &mut Cx<'_>, args: &[Value]) -> Answer<()> {
        cx.run(\"INSERT INTO NOTES VALUES (?)\", args)
    }
}
",
        added: ".procedure(\"NOTE\", Note)",
    },
    Starter {
        word: "metric",
        code: 5,
        declaration: "METRIC TAXICAB PROPERTIES (SYMMETRIC, NON_NEGATIVE, TRIANGLE)",
        uses: &["turborel_plugin::Metric"],
        source: "/// TAXICAB: the sum of the coordinates' differences.
struct Taxicab;

impl Metric for Taxicab {
    fn distance(&mut self, query: &[f64], candidate: &[f64]) -> Answer<Option<f64>> {
        if query.len() != candidate.len() {
            return Ok(None);
        }
        Ok(Some(query.iter().zip(candidate).map(|(a, b)| (a - b).abs()).sum()))
    }
}
",
        added: ".metric(\"TAXICAB\", Taxicab)",
    },
    Starter {
        word: "embedding",
        code: 6,
        declaration: "EMBEDDING MODEL LETTERS DIMENSIONS 26 DETERMINISTIC",
        uses: &["turborel_plugin::Embedder"],
        source: "/// LETTERS: a text's count of each letter a to z.
struct Letters;

impl Embedder for Letters {
    fn embed(&mut self, text: &str) -> Answer<Vec<f64>> {
        let mut counts = vec![0.0; 26];
        for c in text.to_lowercase().chars().filter(char::is_ascii_lowercase) {
            let at = usize::try_from(u32::from(c) - u32::from('a')).ok();
            if let Some(slot) = at.and_then(|i| counts.get_mut(i)) {
                *slot += 1.0;
            }
        }
        Ok(counts)
    }
}
",
        added: ".embedder(\"LETTERS\", Letters)",
    },
    Starter {
        word: "authenticator",
        code: 7,
        declaration: "AUTHENTICATOR 'hello-token'",
        uses: &["turborel_plugin::Authenticator", "turborel_plugin::Exchange", "turborel_plugin::Step", "turborel_plugin::Who"],
        source: "/// The mechanism hello-token: refuses every client until its check is
/// written in `step`.
struct Token;

/// One client's exchange.
struct Checking;

impl Authenticator for Token {
    fn begin(&mut self, _who: &Who) -> Answer<Box<dyn Exchange>> {
        Ok(Box::new(Checking))
    }
}

impl Exchange for Checking {
    fn step(&mut self, _bytes: &[u8]) -> Answer<Step> {
        Ok(Step::Refuse(\"hello-token checks no token yet\".to_owned()))
    }
}
",
        added: ".authenticator(\"hello-token\", Token)",
    },
    Starter {
        word: "target",
        code: 8,
        declaration: "TARGET 'hello'",
        uses: &["turborel_plugin::PluginError", "turborel_plugin::Target", "turborel_plugin::TargetStore"],
        source: "/// The backup target of scheme hello.
struct Store;

impl Target for Store {
    fn open(&mut self, location: &str) -> Answer<Box<dyn TargetStore>> {
        Err(PluginError::external(&format!(\"the target {location} is not written yet\")))
    }
}
",
        added: ".target(\"hello\", Store)",
    },
    Starter {
        word: "partition",
        code: 9,
        declaration: "PARTITION STRATEGY BUCKETS",
        uses: &[
            "turborel::catalog::PartitionBound",
            "turborel::value::Value",
            "turborel_plugin::PartitionStrategy",
            "turborel_plugin::PluginError",
        ],
        source: "/// BUCKETS: a partitioning strategy, refusing every key until written.
struct Buckets;

/// What each call answers until the strategy is written.
fn unwritten<T>() -> Answer<T> {
    Err(PluginError::external(\"BUCKETS is not written yet\"))
}

impl PartitionStrategy for Buckets {
    fn check_key(&mut self, _types: &[String]) -> Answer<()> {
        unwritten()
    }
    fn bound(&mut self, _types: &[String], _bound: PartitionBound) -> Answer<PartitionBound> {
        unwritten()
    }
    fn overlaps(&mut self, _a: &PartitionBound, _b: &PartitionBound) -> Answer<bool> {
        unwritten()
    }
    fn contains(&mut self, _bound: &PartitionBound, _key: &[Value]) -> Answer<bool> {
        unwritten()
    }
}
",
        added: ".partition_strategy(\"BUCKETS\", Buckets)",
    },
    Starter {
        word: "ranking",
        code: 10,
        declaration: "RANKING FLAT",
        uses: &["turborel::fulltext::rank::Corpus", "turborel::fulltext::rank::InDocument", "turborel::fulltext::rank::TermStats", "turborel_plugin::Ranking"],
        source: "/// FLAT: every term found weighs 1.
struct Flat;

impl Ranking for Flat {
    fn weight(&mut self, _corpus: &Corpus, _term: &TermStats, _in_doc: &InDocument) -> f64 {
        1.0
    }
    fn bound(&mut self, _corpus: &Corpus, _term: &TermStats) -> f64 {
        1.0
    }
}
",
        added: ".ranking(\"FLAT\", Flat)",
    },
    Starter {
        word: "quantizer",
        code: 11,
        declaration: "QUANTIZER CODES",
        uses: &["turborel_plugin::PluginError", "turborel_plugin::Quantizer", "turborel_plugin::Trained", "turborel_plugin::Training"],
        source: "/// CODES: a vector quantizer, which trains no codebook until written.
struct Codes;

impl Quantizer for Codes {
    fn train(&mut self, _training: &Training) -> Answer<Trained> {
        Err(PluginError::external(\"CODES is not written yet\"))
    }
}
",
        added: ".quantizer(\"CODES\", Codes)",
    },
    Starter {
        word: "collation",
        code: 12,
        declaration: "COLLATION CASELESS VERSION '1'",
        uses: &["turborel_plugin::Collation"],
        source: "/// CASELESS: strings compared by their lower case, trailing spaces
/// passed over where padding asks.
struct Caseless;

impl Collation for Caseless {
    fn sort_key(&mut self, text: &str, pad: bool) -> Answer<Vec<u8>> {
        let text = if pad { text.trim_end_matches(' ') } else { text };
        Ok(text.to_lowercase().into_bytes())
    }
}
",
        added: ".collation(\"CASELESS\", Caseless)",
    },
    Starter {
        word: "system-table",
        code: 13,
        declaration: "SYSTEM TABLE FACTS (NAME VARCHAR(40), N INTEGER)",
        uses: &["turborel::value::Decimal", "turborel::value::Value", "turborel_plugin::SystemTable"],
        source: "/// FACTS: one row, the answer.
struct Facts;

impl SystemTable for Facts {
    fn rows(&mut self, _user: &str, _owner: bool) -> Answer<Vec<Vec<Value>>> {
        Ok(vec![vec![Value::Char(\"answer\".to_owned()), Value::Exact(Decimal::new(42, 0))]])
    }
}
",
        added: ".system_table(\"FACTS\", Facts)",
    },
    Starter {
        word: "xquery",
        code: 14,
        declaration: "XQUERY FUNCTION '{http://example.com/fn}twice' (INTEGER) RETURNS INTEGER",
        uses: &["turborel::value::Decimal", "turborel::value::Value", "turborel_plugin::XQueryFunction"],
        source: "/// fn:twice: its argument doubled.
struct XTwice;

impl XQueryFunction for XTwice {
    fn call(&mut self, args: &[Value]) -> Answer<Value> {
        match args.first() {
            Some(Value::Exact(d)) => Ok(Value::Exact(d.checked_mul(Decimal::new(2, 0))?)),
            Some(_) | None => Ok(Value::Null),
        }
    }
}
",
        added: ".xquery_function(\"{http://example.com/fn}twice\", XTwice)",
    },
    Starter {
        word: "type",
        code: 15,
        declaration: "TYPE CODE ORDERED",
        uses: &["turborel_plugin::Made", "turborel_plugin::PluginError", "turborel_plugin::Type"],
        source: "/// CODE: a text of letters and digits, ordered and compared in upper case.
struct Code;

/// The value of the text `text`.
fn code(text: &str) -> Answer<Made> {
    if !text.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(PluginError::data(&format!(\"{text} is not a CODE\")));
    }
    let upper = text.to_ascii_uppercase();
    Ok(Made { key: upper.clone().into_bytes(), binary: upper.clone().into_bytes(), text: upper })
}

impl Type for Code {
    fn input(&mut self, text: &str) -> Answer<Made> {
        code(text)
    }
    fn receive(&mut self, bytes: &[u8]) -> Answer<Made> {
        code(&String::from_utf8_lossy(bytes))
    }
}
",
        added: ".data_type(\"CODE\", Code)",
    },
    Starter {
        word: "engine",
        code: 16,
        declaration: "STORAGE ENGINE STORE",
        uses: &[
            "turborel::storage::Row",
            "turborel::storage::RowId",
            "turborel_plugin::Engine",
            "turborel_plugin::Opened",
            "turborel_plugin::PluginError",
        ],
        source: "/// STORE: a storage engine, which keeps no table until written.
struct Rows;

/// What each call of STORE answers until it is written.
fn no_store<T>() -> Answer<T> {
    Err(PluginError::external(\"STORE is not written yet\"))
}

impl Engine for Rows {
    fn create(&mut self, _table: &str, _columns: &[String]) -> Answer<Vec<u8>> {
        no_store()
    }
    fn open(&mut self, _locator: &[u8]) -> Answer<Opened> {
        no_store()
    }
    fn scan(&mut self, _locator: &[u8], _after: RowId, _max: u32) -> Answer<(Vec<(RowId, Row)>, bool)> {
        no_store()
    }
    fn get(&mut self, _locator: &[u8], _ids: &[RowId]) -> Answer<Vec<Option<Row>>> {
        no_store()
    }
    fn apply(&mut self, _locator: &[u8], _changes: &[(RowId, Option<Row>)]) -> Answer<Vec<Option<Row>>> {
        no_store()
    }
    fn checkpoint(&mut self, _locator: &[u8], _through: u64) -> Answer<()> {
        no_store()
    }
    fn drop_table(&mut self, _locator: &[u8]) -> Answer<()> {
        no_store()
    }
}
",
        added: ".engine(\"STORE\", Rows)",
    },
    Starter {
        word: "index-method",
        code: 17,
        declaration: "INDEX METHOD SORTED SUPPORTS (EQUALITY, RANGE)",
        uses: &[
            "turborel::storage::RowId",
            "turborel::value::Value",
            "turborel_plugin::Entries",
            "turborel_plugin::IndexMethod",
            "turborel_plugin::PluginError",
            "turborel_plugin::Search",
            "turborel_plugin::Write",
        ],
        source: "/// SORTED: an index method, which keeps no index until written.
struct Sorted;

/// What each call of SORTED answers until it is written.
fn no_index<T>() -> Answer<T> {
    Err(PluginError::external(\"SORTED is not written yet\"))
}

impl IndexMethod for Sorted {
    fn create(&mut self, _types: &[String]) -> Answer<Vec<u8>> {
        no_index()
    }
    fn open(&mut self, _locator: &[u8]) -> Answer<(bool, bool)> {
        no_index()
    }
    fn write(&mut self, _locator: &[u8], _writes: &[Write]) -> Answer<()> {
        no_index()
    }
    fn search(&mut self, _locator: &[u8], _search: &Search) -> Answer<(Entries, Vec<u8>)> {
        no_index()
    }
    fn rows(&mut self, _locator: &[u8], _key: &[Value]) -> Answer<Vec<RowId>> {
        no_index()
    }
    fn checkpoint(&mut self, _locator: &[u8]) -> Answer<()> {
        no_index()
    }
    fn drop_index(&mut self, _locator: &[u8]) -> Answer<()> {
        no_index()
    }
}
",
        added: ".index_method(\"SORTED\", Sorted)",
    },
    Starter {
        word: "credentials",
        code: 18,
        declaration: "CREDENTIALS VERIFIERS",
        uses: &["turborel_plugin::Credentials", "turborel_plugin::PluginError"],
        source: "/// VERIFIERS: makes no verifier until written, so no password is kept
/// in a form not chosen.
struct Verifiers;

impl Credentials for Verifiers {
    fn verifier(&mut self, user: &str, _password: &str) -> Answer<Vec<u8>> {
        Err(PluginError::external(&format!(\"VERIFIERS makes no verifier of {user} yet\")))
    }
}
",
        added: ".credentials(\"VERIFIERS\", Verifiers)",
    },
    Starter {
        word: "diagnostics",
        code: 19,
        declaration: "DIAGNOSTICS EVENTS",
        uses: &["turborel_plugin::Diagnostics", "turborel_plugin::Told"],
        source: "/// EVENTS: writes each event to the standard error, which the database
/// keeps the last of.
struct Events;

impl Diagnostics for Events {
    fn events(&mut self, events: &[Told], dropped: u64) -> Answer<()> {
        for e in events {
            eprintln!(\"{} {}: {}\", e.level, e.kind, e.text);
        }
        if dropped > 0 {
            eprintln!(\"{dropped} events dropped\");
        }
        Ok(())
    }
}
",
        added: ".diagnostics(\"EVENTS\", Events)",
    },
];
