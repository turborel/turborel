//! The built-in feature-hashing models: determinism, shape, norm, and
//! that shared words make vectors near.

use super::*;
use crate::vector::Vector;
use crate::vector::build::Entries;

fn one(model: &HashedEmbedder, text: &str) -> Vector {
    model.embed(&[text]).unwrap().pop().unwrap()
}

fn cosine(a: &Vector, b: &Vector) -> f64 {
    let (x, y) = (a.values(), b.values());
    let dot: f64 = x.iter().zip(&y).map(|(p, q)| p * q).sum();
    let norm = |v: &[f64]| v.iter().map(|p| p * p).sum::<f64>().sqrt();
    dot / (norm(&x) * norm(&y))
}

/// Verifies: [EXTENSION] (§12.1, the `Embedder` contract) Equal texts
/// give equal images bit for bit, whatever the batch, the instance or
/// the case and punctuation of the text; each vector has the model's
/// shape and norm 1; an empty text gives the zero vector.
#[test]
fn hashed_models_are_deterministic() {
    for model in [HashedEmbedder::dense(), HashedEmbedder::sparse()] {
        let alone = one(&model, "The quick brown fox");
        let batch = model.embed(&["other words", "The quick brown fox", ""]).unwrap();
        let shape = model.shape();
        assert_eq!(alone.encode().unwrap(), batch[1].encode().unwrap());
        let other = HashedEmbedder::new(model.name(), shape.dims, shape.layout, if shape.layout == Layout::Dense { 2 } else { 1 });
        assert_eq!(alone.encode().unwrap(), one(&other, "the QUICK, brown fox!").encode().unwrap());
        assert!(batch.iter().all(|v| shape.fits(v)));
        let norm = alone.values().iter().map(|v| v * v).sum::<f64>().sqrt();
        assert!((norm - 1.0_f64).abs() < 1.0e-6_f64, "{norm}");
        assert!(batch[2].values().iter().all(|v| *v == 0.0_f64));
        assert_ne!(alone.encode().unwrap(), batch[0].encode().unwrap());
    }
}

/// Verifies: [EXTENSION] (Weinberger et al. 2009 §2) Texts sharing words
/// are nearer by cosine than texts sharing none; a sparse vector holds
/// one weight a distinct token, a repeated one weighted 1 + ln(count)
/// before the norm.
#[test]
fn shared_words_make_vectors_near() {
    let model = HashedEmbedder::dense();
    let a = one(&model, "vector search in a relational database");
    let b = one(&model, "relational database vector search engine");
    let c = one(&model, "baking bread with sourdough starter");
    assert!(cosine(&a, &b) > cosine(&a, &c) + 0.3_f64, "{} {}", cosine(&a, &b), cosine(&a, &c));
    let sparse = HashedEmbedder::sparse();
    let v = one(&sparse, "cat cat dog");
    let Entries::Sparse(pairs) = &v.entries else { panic!("a sparse model gives sparse vectors") };
    assert_eq!(pairs.len(), 2);
    let mut weights: Vec<f64> = pairs.iter().map(|(_, w)| *w).collect();
    weights.sort_by(f64::total_cmp);
    assert!((weights[1] / weights[0] - (1.0_f64 + 2.0_f64.ln())).abs() < 1.0e-6_f64);
    assert_eq!(sparse.tokens("  Hi, there!"), [(2, 4), (6, 11)]);
}

