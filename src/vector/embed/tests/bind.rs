//! Models of the catalog bound to their plug-ins (`bind.rs`, `model.rs`):
//! the options the database reads, the shape the others agree with, a
//! binding kept until its model changes, a stand-in for a plug-in not
//! registered, and the texts embedded ahead.

use std::sync::Arc;

use super::super::*;
use crate::catalog::{ModelDesc, ModelOption, QualifiedName};
use crate::error::Code;
use crate::functions::ScalarFunction;
use crate::value::{DataType, Value};

fn option(name: &str, value: &str, quoted: bool) -> ModelOption {
    ModelOption { name: name.to_owned(), value: value.to_owned(), quoted }
}

/// The model U.M of `plugin`, of `options`, of the shape of HASH_DENSE.
fn desc(plugin: &str, options: Vec<ModelOption>) -> ModelDesc {
    ModelDesc {
        name: QualifiedName { auth: "U".into(), ident: "M".into() },
        plugin: plugin.into(),
        file: None,
        blob: None,
        options,
        version: None,
        dims: HASH_DENSE_DIMS,
        element: Element::Float32,
        layout: Layout::Dense,
        execute: Vec::new(),
    }
}

/// Verifies: [EXTENSION] (vector design §12.1, R12.3, R12.4, R12.5) The
/// database reads VERSION, BATCH_SIZE, ON_ERROR and MAX_RETRIES itself,
/// a model's version its VERSION else its plug-in's; it refuses a value
/// of them it does not take, and a whole number written as a string.
#[test]
fn the_database_reads_its_own_options() {
    let models = Embedders::built_in();
    let (_, (version, batch, on_error)) = models.configure(HASH_DENSE, (None, None), &[]).unwrap();
    assert_eq!((version, batch, on_error), (1, DEFAULT_BATCH, OnError::Fail));
    let given = [
        option("VERSION", "4", false),
        option("BATCH_SIZE", "3", false),
        option("ON_ERROR", "retry", true),
        option("MAX_RETRIES", "5", false),
    ];
    let (_, settings) = models.configure(HASH_DENSE, (None, None), &given).unwrap();
    assert_eq!(settings, (4, 3, OnError::Retry(5)));
    let (_, (_, _, null)) = models.configure(HASH_DENSE, (None, None), &[option("ON_ERROR", "NULL", true)]).unwrap();
    assert_eq!(null, OnError::Null);
    for refused in
        [option("VERSION", "4", true), option("BATCH_SIZE", "0", false), option("ON_ERROR", "SKIP", true), option("VERSION", "-1", false)]
    {
        assert_eq!(
            models.configure(HASH_DENSE, (None, None), std::slice::from_ref(&refused)).err().map(|e| e.code),
            Some(Code::SyntaxRule),
            "{refused:?}"
        );
    }
    assert_eq!(models.configure("NONE", (None, None), &[]).err().map(|e| e.code), Some(Code::SyntaxRule));
    assert_eq!(number_option(&option("N", "12", false)).unwrap(), 12);
}

/// Verifies: [EXTENSION] (vector design §12.1 VEC-0031) DIMENSIONS,
/// FORMAT and LAYOUT agree with the plug-in's shape, or are refused; a
/// plug-in that takes no options of its own refuses another; HASH_DENSE
/// takes DIMENSIONS and NGRAMS, and FROM FILE is given it as FILE, which
/// it refuses.
#[test]
fn shape_options_agree_with_the_plug_in_vec_0031() {
    let shape = ModelShape { dims: 8, element: Element::Float32, layout: Layout::Dense };
    let agreeing = [option("DIMENSIONS", "8", false), option("FORMAT", "float32", false), option("LAYOUT", "DENSE", false)];
    assert_eq!(configure_shape(shape, &agreeing).unwrap(), shape);
    for refused in [
        option("DIMENSIONS", "9", false),
        option("FORMAT", "INT8", false),
        option("LAYOUT", "SPARSE", false),
        option("POOLING", "mean", true),
    ] {
        assert_eq!(configure_shape(shape, std::slice::from_ref(&refused)).err().map(|e| e.code), Some(Code::SyntaxRule), "{refused:?}");
    }
    let models = Embedders::built_in();
    let (made, _) = models.configure(HASH_DENSE, (None, None), &[option("DIMENSIONS", "32", false), option("NGRAMS", "3", false)]).unwrap();
    assert_eq!(made.shape().dims, 32);
    assert_eq!(models.configure(HASH_DENSE, (Some("/m.onnx"), None), &[]).err().map(|e| e.code), Some(Code::SyntaxRule));
    assert_eq!(models.configure(HASH_DENSE, (None, None), &[option("NGRAMS", "0", false)]).err().map(|e| e.code), Some(Code::SyntaxRule));
    assert_eq!(
        models.configure(HASH_SPARSE, (None, None), &[option("FORMAT", "FLOAT64", false)]).err().map(|e| e.code),
        Some(Code::SyntaxRule)
    );
}

/// Verifies: [EXTENSION] (vector design §12.1) A model is bound once for
/// what it was made of, again once its options change; a model whose
/// plug-in gives another shape than it was made of is refused; one whose
/// plug-in is not registered is bound to a stand-in of its shape that
/// refuses to embed (VEC-0030), and bound to the plug-in once it is.
#[test]
fn a_binding_is_kept_until_its_model_changes() {
    let models = Embedders::built_in();
    let first = models.bind(&desc(HASH_DENSE, Vec::new())).unwrap();
    assert!(Arc::ptr_eq(&first, &models.bind(&desc(HASH_DENSE, Vec::new())).unwrap()));
    let changed = models.bind(&desc(HASH_DENSE, vec![option("VERSION", "2", false)])).unwrap();
    assert!(!Arc::ptr_eq(&first, &changed));
    assert_eq!((changed.version(), changed.tag()), (2, "U.M:2".to_owned()));
    assert_eq!(models.bind(&desc(HASH_SPARSE, Vec::new())).err().map(|e| e.code), Some(Code::SyntaxRule), "another shape");
    let stand_in = models.bind(&desc("LATER", Vec::new())).unwrap();
    assert_eq!(stand_in.shape().dims, HASH_DENSE_DIMS);
    assert_eq!(stand_in.embed(&["x"]).err().map(|e| e.code), Some(Code::SyntaxRule));
    models.register(Arc::new(HashedEmbedder::new("LATER", HASH_DENSE_DIMS, Layout::Dense, 1))).unwrap();
    let bound = models.bind(&desc("LATER", Vec::new())).unwrap();
    assert!(!Arc::ptr_eq(&stand_in, &bound));
    assert!(bound.embed(&["x"]).is_ok());
}

/// Verifies: [EXTENSION] (vector design R12.1, R12.3, R12.5) EMBED of a
/// model bound is of VECTOR of its shape, gives the values of many rows
/// at once, the null value of a null text or purpose, and a text embedded
/// ahead as embedded then; EMBED_VERSION is its tag; each refuses
/// arguments not texts.
#[test]
fn a_bound_model_embeds_and_names_its_version() {
    let models = Embedders::built_in();
    let model = models.bind(&desc(HASH_DENSE, Vec::new())).unwrap();
    let embed = BoundEmbed { name: "EMBED", model: Arc::clone(&model) };
    let text = |s: &str| Value::Char(s.to_owned());
    assert_eq!(embed.return_type(&[DataType::varying(10)]).unwrap(), model.shape().data_type().unwrap());
    assert_eq!(embed.return_type(&[DataType::Boolean]).err().map(|e| e.code), Some(Code::SyntaxRule));
    let one = embed.call(&[text("a b")]).unwrap();
    let many =
        embed.call_many(&[vec![text("a b")], vec![Value::Null], vec![text("a b"), Value::Null], vec![text("a b"), text("query")]]).unwrap();
    assert_eq!(many.iter().map(Value::is_null).collect::<Vec<_>>(), [false, true, true, false]);
    assert_eq!(many.first().map(ToString::to_string), Some(one.to_string()));
    assert_eq!(embed.call(&[text("a"), text("neither")]).err().map(|e| e.code), Some(Code::Data));
    assert!(embed.batches() && embed.deterministic());
    model.prime(&["primed".to_owned()]).unwrap();
    assert_eq!(model.embed_one("primed").unwrap().to_string(), embed.call(&[text("primed")]).unwrap().to_string());
    model.unprime();
    let version = BoundVersion { model };
    assert_eq!(version.call(&[]).unwrap().to_string(), "U.M:1");
    assert!(version.call(&[Value::Null]).unwrap().is_null());
    assert_eq!(version.return_type(&[DataType::Boolean]).err().map(|e| e.code), Some(Code::SyntaxRule));
}
