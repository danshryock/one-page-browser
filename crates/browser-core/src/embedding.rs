//! A small, deterministic, dependency-free text embedding used to give
//! `HistoryStore::search_similar` something real to compare via libsql's
//! native vector functions (`vector32`/`vector_distance_cos` — see
//! `history.rs`).
//!
//! **This is not a semantic embedding.** There's no local ML model or
//! network access available in this environment to generate one, and both
//! are real, separate decisions (dependency size/complexity for a local
//! model; cost, an API key, and a network dependency for a remote one) —
//! not something to pick unsupervised. Instead this uses the "hashing
//! trick" (a real, long-established technique, e.g. Vowpal Wabbit's default
//! feature representation): each token in the text is hashed into one of a
//! fixed number of dimensions and added with a hash-derived sign, then the
//! whole vector is L2-normalized. Cosine similarity between two such
//! vectors approximates *lexical* overlap (shared vocabulary, robust to
//! word order and which exact substring matched) — useful, real, and
//! meaningfully different from the exact-substring `LIKE` search
//! `HistoryStore::search` already does, but it is not going to find
//! conceptually-related pages that don't share vocabulary the way a real
//! semantic embedding would. Swapping in a real embedding model later only
//! means changing this one function — everything downstream (the SQL,
//! `search_similar`) stays the same.
//!
//! Hashing is hand-rolled (FNV-1a) rather than `std::hash::DefaultHasher`
//! deliberately: `DefaultHasher`'s algorithm is documented as *not*
//! guaranteed stable across Rust versions, but embeddings stored today need
//! to stay comparable against embeddings computed by a rebuilt binary
//! months from now — using a hash this code fully controls is what makes
//! that a real guarantee instead of an accident of the current toolchain.

/// Fixed embedding dimensionality — small enough to keep every vector tiny
/// (`4 * DIMS` bytes per row) for a personal browsing history's scale
/// (thousands, not millions, of entries), large enough to keep hash
/// collisions between unrelated common words reasonably rare.
pub const DIMS: usize = 64;

/// FNV-1a, chosen for being simple, fast, and fully specified (unlike
/// `DefaultHasher`) — see this module's doc comment for why that stability
/// matters here.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Splits `text` into lowercase alphanumeric tokens — the same shape of
/// tokenization a simple search box would use, not anything more clever.
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(str::to_string).collect()
}

/// Embeds `text` into a fixed-size, L2-normalized vector via the hashing
/// trick described in this module's doc comment. Empty or all-punctuation
/// input embeds to the zero vector (cosine similarity against it is
/// undefined/always maximally distant, which is the right behavior — there
/// was nothing to compare).
pub fn embed(text: &str) -> [f32; DIMS] {
    let mut vector = [0f32; DIMS];
    for token in &tokenize(text) {
        let hash = fnv1a(token.as_bytes());
        let index = (hash % DIMS as u64) as usize;
        // A different bits of the same hash pick the sign — using two
        // independent-ish hashes' worth of entropy from one FNV pass rather
        // than a second hash call, cheap and sufficient for this purpose.
        let sign = if (hash >> 32) & 1 == 0 { 1.0 } else { -1.0 };
        vector[index] += sign;
    }
    let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in vector.iter_mut() {
            *x /= norm;
        }
    }
    vector
}

/// Renders an embedding as the JSON-array text libsql's `vector32(...)` SQL
/// function expects (e.g. `"[0.1,-0.2,0.0]"`) — the only representation
/// this code hands to SQL; libsql packs it into its own compact `F32_BLOB`
/// storage format internally.
pub fn to_sql_literal(vector: &[f32; DIMS]) -> String {
    let mut literal = String::with_capacity(DIMS * 8);
    literal.push('[');
    for (i, x) in vector.iter().enumerate() {
        if i > 0 {
            literal.push(',');
        }
        literal.push_str(&x.to_string());
    }
    literal.push(']');
    literal
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine_similarity(a: &[f32; DIMS], b: &[f32; DIMS]) -> f32 {
        a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn embedding_is_deterministic() {
        assert_eq!(embed("Rust Programming Language"), embed("Rust Programming Language"));
    }

    #[test]
    fn embedding_is_case_insensitive() {
        assert_eq!(embed("Rust Programming"), embed("rust programming"));
    }

    #[test]
    fn embedding_is_word_order_independent() {
        // The hashing-trick bag-of-words representation can't distinguish
        // word order at all — documenting that directly as a test, not
        // just a claim in a comment.
        assert_eq!(embed("rust programming language"), embed("language programming rust"));
    }

    #[test]
    fn shared_vocabulary_is_more_similar_than_no_shared_vocabulary() {
        let a = embed("Rust Programming Language Tutorial");
        let b = embed("Rust Programming Language Guide");
        let c = embed("Baking Sourdough Bread At Home");

        let similar = cosine_similarity(&a, &b);
        let unrelated = cosine_similarity(&a, &c);
        assert!(
            similar > unrelated,
            "two titles sharing most of their vocabulary should be more similar than two sharing none \
             (similar={similar}, unrelated={unrelated})"
        );
    }

    #[test]
    fn empty_text_embeds_to_the_zero_vector() {
        assert_eq!(embed(""), [0f32; DIMS]);
        assert_eq!(embed("   !!! ..."), [0f32; DIMS]);
    }

    #[test]
    fn embedding_is_l2_normalized() {
        let v = embed("Rust Programming Language Tutorial For Beginners");
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "expected a unit vector, got norm {norm}");
    }

    #[test]
    fn sql_literal_round_trips_through_json_parsing() {
        let v = embed("Rust Programming Language");
        let literal = to_sql_literal(&v);
        assert!(literal.starts_with('['));
        assert!(literal.ends_with(']'));
        let parsed: Vec<f32> = literal[1..literal.len() - 1].split(',').map(|s| s.parse().unwrap()).collect();
        assert_eq!(parsed.len(), DIMS);
        for (a, b) in parsed.iter().zip(v.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }
}
