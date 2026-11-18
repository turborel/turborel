//! EMBED and CHUNK_TEXT as functions and in SQL: their forms, nulls,
//! refusals, a model's failures, and a specification read once.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::error::SqlError;
use crate::functions::ScalarFunction;
use crate::memory::MemoryBudget;
use crate::value::{Decimal, Value};
use crate::vector::Vector;

/// Asserts `a` and `b` are the same value.
#[track_caller]
fn assert_same(a: &Value, b: &Value) {
    assert_eq!(a.norm(), b.norm());
}

fn text(s: &str) -> Value {
    Value::Char(s.to_owned())
}

fn routine(models: &Arc<Embedders>, name: &str) -> Box<dyn ScalarFunction> {
    routines(models).into_iter().find(|r| r.name() == name).unwrap()
}

/// A model failing its first `fails` calls, or giving vectors of the
/// wrong shape.
struct Flaky {
    name: &'static str,
    fails: AtomicU32,
    on_error: OnError,
    wrong: bool,
    calls: Mutex<u32>,
}

impl Embedder for Flaky {
    fn name(&self) -> &str {
        self.name
    }
    fn shape(&self) -> ModelShape {
        ModelShape { dims: 2, element: Element::Float32, layout: Layout::Dense }
    }
    fn on_error(&self) -> OnError {
        self.on_error
    }
    fn prefix(&self, purpose: Purpose) -> &'static str {
        if purpose == Purpose::Query { "query: " } else { "" }
    }
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vector>> {
        let mut calls = crate::sync::lock_builtin(&self.calls);
        *calls = calls.saturating_add(1);
        if self.fails.load(Ordering::Acquire) > 0 {
            self.fails.fetch_sub(1, Ordering::AcqRel);
            return Err(SqlError::new(Code::Api, "the endpoint timed out"));
        }
        let dims = if self.wrong { 3 } else { 2 };
        texts.iter().map(|t| Vector::dense(Element::Float32, vec![f64::from(u32::try_from(t.len()).unwrap()); dims])).collect()
    }
}

fn flaky(name: &'static str, fails: u32, on_error: OnError, wrong: bool) -> Arc<Flaky> {
    Arc::new(Flaky { name, fails: AtomicU32::new(fails), on_error, wrong, calls: Mutex::new(0) })
}

/// Verifies: [EXTENSION] (§12.2, R12.1) EMBED gives the model's vector as
/// a VECTOR value, by either name, the model named in any case; a query's
/// prefix goes first; a null argument gives null; an unknown model or
/// purpose is refused with SQLCODE -301, and arguments not text with -103.
#[test]
fn embed_gives_the_models_vector() {
    let models = Arc::new(Embedders::built_in());
    let embed = routine(&models, "EMBED");
    let expected = HashedEmbedder::dense().embed(&["a text"]).unwrap().pop().unwrap().value().unwrap();
    assert_same(&embed.call(&[text("hash_dense"), text("a text")]).unwrap(), &expected);
    assert_same(&routine(&models, "VECTOR_EMBEDDING").call(&[text("HASH_DENSE"), text("a text")]).unwrap(), &expected);
    assert_same(&embed.call(&[text("HASH_DENSE"), Value::Null]).unwrap(), &Value::Null);
    assert_eq!(embed.call(&[text("NONE"), text("x")]).unwrap_err().code, Code::Data);
    assert_eq!(embed.call(&[text("HASH_DENSE"), text("x"), text("passage")]).unwrap_err().code, Code::Data);
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: crate::value::Spelled::Integer };
    assert_eq!(embed.return_type(&[DataType::varying(10), integer]).unwrap_err().code, Code::SyntaxRule);
    assert!(embed.deterministic());
    let model = flaky("PREFIXED", 0, OnError::Fail, false);
    models.register(model).unwrap();
    let query = embed.call(&[text("PREFIXED"), text("abc"), text("Query")]).unwrap();
    // "query: abc" is ten long.
    assert_same(&query, &Vector::dense(Element::Float32, vec![10.0_f64; 2]).unwrap().value().unwrap());
}

/// Verifies: [EXTENSION] (R12.4) A model that fails fails the call by
/// default, gives null under ON_ERROR NULL, and is asked again under
/// RETRY; a vector not of the model's shape is refused.
#[test]
fn embed_follows_on_error() {
    let models = Arc::new(Embedders::new());
    let (failing, null, retry, wrong) = (
        flaky("FAIL", 1, OnError::Fail, false),
        flaky("NUL", 1, OnError::Null, false),
        flaky("RETRY", 2, OnError::Retry(2), false),
        flaky("WRONG", 0, OnError::Fail, true),
    );
    for model in [&failing, &null, &retry, &wrong] {
        let model: Arc<dyn Embedder> = Arc::<Flaky>::clone(model);
        models.register(model).unwrap();
    }
    let embed = routine(&models, "EMBED");
    assert_eq!(embed.call(&[text("FAIL"), text("x")]).unwrap_err().code, Code::Api);
    assert_same(&embed.call(&[text("NUL"), text("x")]).unwrap(), &Value::Null);
    assert!(embed.call(&[text("RETRY"), text("x")]).is_ok());
    assert_eq!(*crate::sync::lock_builtin(&retry.calls), 3);
    assert_eq!(embed.call(&[text("WRONG"), text("x")]).unwrap_err().code, Code::Data);
}

/// Verifies: [EXTENSION] (§12.3) CHUNK_TEXT gives an ARRAY of ROW(CHUNK_ID,
/// CHUNK_OFFSET, CHUNK_LENGTH, CHUNK_TEXT); the specification as one
/// string and as parts in place give the same chunks; a constant
/// specification prepared gives them too; TOKENS counts the model's
/// tokens; a null text or part gives null.
#[test]
fn chunk_text_forms_agree() {
    let models = Arc::new(Embedders::built_in());
    let chunk = routine(&models, "CHUNK_TEXT");
    let body = text("one two  three, four five six");
    let whole = |n: i128| Value::Exact(Decimal::new(n, 0));
    let spec = chunk.call(&[body.clone(), text("BY WORDS MAX 2 OVERLAP 1 NORMALIZE WHITESPACE")]).unwrap();
    let parts = chunk.call(&[body.clone(), text("WORDS"), whole(2), whole(1), Value::Null, Value::Bool(true)]).unwrap();
    assert_same(&spec, &parts);
    let Value::Array(rows) = &spec else { panic!("an array") };
    assert_eq!(rows.len(), 5);
    let second: Arc<[Value]> = Arc::from([whole(2), whole(5), whole(11), text("two three,")]);
    assert_same(&rows[1], &Value::Row(second));
    let budget = Arc::new(MemoryBudget::new(usize::MAX));
    let spec_text = text("BY TOKENS HASH_DENSE MAX 2");
    let prepared = chunk.specialize(&[None, Some(&spec_text)], &budget).unwrap();
    let tokens = prepared.0.call(std::slice::from_ref(&body)).unwrap();
    assert_same(&tokens, &chunk.call(&[body.clone(), spec_text]).unwrap());
    let Value::Array(token_rows) = &tokens else { panic!("an array") };
    assert_eq!(token_rows.len(), 3, "the comma is no token of HASH_DENSE");
    assert_same(&chunk.call(&[Value::Null, text("BY WORDS")]).unwrap(), &Value::Null);
    assert_same(&chunk.call(&[body, text("WORDS"), Value::Null]).unwrap(), &Value::Null);
    assert_eq!(chunk.call(&[text("x"), text("BY TOKENS NONE")]).unwrap_err().code, Code::Data);
    assert!(chunk.return_type(&[DataType::varying(10), DataType::varying(10)]).is_ok());
    assert_eq!(chunk.return_type(&[DataType::Boolean]).unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] (§12.3, §12.2) In SQL, UNNEST of CHUNK_TEXT is
/// the table of chunks, and EMBED of each gives VECTOR values a VECTOR
/// column stores.
#[test]
fn chunks_and_embeddings_in_sql() {
    let db = database();
    let s = db.session("U");
    let chunks = rows(&s, "SELECT C.CHUNK_ID, C.CHUNK_OFFSET, C.CHUNK_TEXT FROM UNNEST(CHUNK_TEXT('a b c d e', 'BY WORDS MAX 2')) AS C (CHUNK_ID, CHUNK_OFFSET, CHUNK_LENGTH, CHUNK_TEXT)").unwrap();
    assert_eq!(chunks, ["1,1,a b", "2,5,c d", "3,9,e"]);
    s.run("CREATE TABLE V (K INTEGER, E VECTOR)").unwrap();
    s.run("INSERT INTO V VALUES (1, EMBED('HASH_DENSE', 'some words'))").unwrap();
    assert_eq!(rows(&s, "SELECT K FROM V WHERE E IS NOT NULL").unwrap(), ["1"]);
}
