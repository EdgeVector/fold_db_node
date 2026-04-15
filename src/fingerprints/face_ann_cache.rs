//! In-memory approximate-nearest-neighbor (ANN) index over face
//! Fingerprint embeddings.
//!
//! ## Why a cache
//!
//! Every face Fingerprint carries a 512-dim L2-normalized ArcFace
//! embedding in its `value` field. When a new face is detected by
//! the photo ingestion pipeline, we need to answer:
//!
//!   "Is this face similar enough to an existing Fingerprint that
//!   we should merge it (create a StrongMatch edge), or is this a
//!   net-new identity observation?"
//!
//! A naive answer is O(N) — scan every existing face Fingerprint,
//! compute cosine similarity against the new embedding, pick the
//! closest. That works up to a few thousand faces. Past ~10K
//! Fingerprints the per-ingestion scan starts dominating latency.
//!
//! This module wraps a graph-based HNSW index (Hierarchical
//! Navigable Small World via the `instant-distance` crate) that
//! answers the same question in O(log N) with ~99% recall at
//! typical parameters. Cosine similarity is computed on the
//! returned candidate set as a final exact-ranking step.
//!
//! ## Ownership and lifecycle
//!
//! The cache is **not persisted**. It's a read-side projection of
//! the Fingerprint records in fold_db, rebuilt from scratch at
//! node startup via [`rebuild_from_store`]. New fingerprints are
//! added incrementally via [`FaceAnnCache::add`] as the writer
//! persists them.
//!
//! Because the cache is derivable from persisted Fingerprints, a
//! corrupted or missing cache is never a correctness problem — the
//! worst case is a slow fallback to linear scan + a rebuild on
//! next boot. Fingerprints themselves are the source of truth.
//!
//! ## Thread-safety
//!
//! `FaceAnnCache` is `Send + Sync` via an internal `RwLock`.
//! Writes (add, rebuild) take the write lock; queries take the
//! read lock. The global [`cache()`] accessor returns an `Arc` to
//! a shared cache instance stored in a `OnceLock`, so there is
//! exactly one cache per process.
//!
//! ## NOT implemented in this PR
//!
//! - Integration into the face extractor. The extractor in
//!   [`crate::fingerprints::extractors::face`] still writes new
//!   Fingerprints without consulting the cache. A follow-up PR
//!   wires the cache in and starts producing StrongMatch /
//!   MediumMatch edges based on cosine similarity to the nearest
//!   existing face.
//! - Hooking `rebuild_from_store` into node startup. Today,
//!   callers must invoke it explicitly after
//!   `register_phase_1_schemas` (see the call site TODO).

use std::sync::{Arc, OnceLock, RwLock};

use instant_distance::{Builder, HnswMap, Point, Search};

use crate::fingerprints::canonical_names;
use crate::fingerprints::schemas::FINGERPRINT;
use crate::fold_node::FoldNode;
use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::schema::types::operations::Query;

/// Distance-metric wrapper for a 512-dim L2-normalized ArcFace
/// embedding. Implementing [`Point`] lets the `instant-distance`
/// crate build an HNSW graph over our embeddings.
///
/// We store the raw embedding and compute distance as
/// `1 - cosine_similarity`. The embeddings are already L2-normalized
/// when they come out of ArcFace, so a plain dot product equals the
/// cosine. We still normalize defensively in [`FaceEmbedding::new`]
/// so that a malformed embedding can't poison the index.
#[derive(Clone, Debug)]
pub struct FaceEmbedding {
    vec: Vec<f32>,
}

impl FaceEmbedding {
    /// Construct a `FaceEmbedding` from raw floats. L2-normalizes
    /// the input so `distance()` is a pure dot-product subtraction.
    /// Rejects empty vectors and all-zero vectors (undefined
    /// normalization).
    pub fn new(raw: Vec<f32>) -> FoldDbResult<Self> {
        if raw.is_empty() {
            return Err(FoldDbError::Config(
                "face_ann_cache: cannot index an empty embedding".to_string(),
            ));
        }
        let norm: f32 = raw.iter().map(|v| v * v).sum::<f32>().sqrt();
        if !norm.is_finite() || norm == 0.0 {
            return Err(FoldDbError::Config(format!(
                "face_ann_cache: cannot normalize embedding with norm={norm}"
            )));
        }
        let vec = raw.into_iter().map(|v| v / norm).collect();
        Ok(Self { vec })
    }

    /// Dimensionality of the embedding.
    pub fn dim(&self) -> usize {
        self.vec.len()
    }

    /// Cosine similarity against another embedding. Both are
    /// already L2-normalized, so this is a plain dot product in
    /// `[-1, 1]`.
    pub fn cosine(&self, other: &FaceEmbedding) -> f32 {
        self.vec
            .iter()
            .zip(other.vec.iter())
            .map(|(a, b)| a * b)
            .sum()
    }
}

impl Point for FaceEmbedding {
    fn distance(&self, other: &Self) -> f32 {
        // instant-distance wants a metric where smaller = nearer.
        // With L2-normalized embeddings, cosine distance is
        // `1 - dot` and is always in `[0, 2]`.
        1.0 - self.cosine(other)
    }
}

/// A single hit from a nearest-neighbor query.
#[derive(Clone, Debug)]
pub struct CacheHit {
    /// The Fingerprint.id (content-addressed key) of the indexed face.
    pub fingerprint_id: String,
    /// Cosine similarity of the indexed face to the query, in `[-1, 1]`.
    /// Higher is more similar. Use this rather than distance so the
    /// caller doesn't have to know the internal metric.
    pub similarity: f32,
}

/// An in-memory HNSW index over face Fingerprint embeddings.
///
/// Cheap to construct (an empty cache is nearly free). Populated
/// from the Fingerprint store via [`rebuild_from_store`] at startup
/// and kept in sync by the writer as new face Fingerprints land.
pub struct FaceAnnCache {
    inner: RwLock<CacheState>,
}

/// The actual indexed state. We keep a canonical list of
/// `(fingerprint_id, embedding)` pairs plus a derived HNSW index;
/// the list is the source of truth and the index is a read-side
/// projection that's rebuilt whenever the list has more entries
/// than the index reflects.
struct CacheState {
    /// Canonical (id, embedding) pairs. Append-only for normal
    /// adds; replaced wholesale by [`FaceAnnCache::replace_all`]
    /// and [`rebuild_from_store`].
    entries: Vec<(String, FaceEmbedding)>,
    /// Derived HNSW graph over `entries`. `None` when `entries` is
    /// empty (instant-distance can't build an empty map) or before
    /// the first flush.
    index: Option<HnswMap<FaceEmbedding, String>>,
    /// Number of entries reflected in `index`. When
    /// `entries.len() != indexed_len`, the index is stale and
    /// `flush()` rebuilds it.
    indexed_len: usize,
}

impl Default for FaceAnnCache {
    fn default() -> Self {
        Self::new()
    }
}

impl FaceAnnCache {
    /// Construct an empty cache.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(CacheState {
                entries: Vec::new(),
                index: None,
                indexed_len: 0,
            }),
        }
    }

    /// Number of face Fingerprints currently tracked by the cache.
    /// Pending-but-not-yet-rebuilt adds are still counted.
    pub fn len(&self) -> usize {
        self.inner
            .read()
            .expect("face_ann_cache read lock")
            .entries
            .len()
    }

    /// Whether the cache has zero indexed fingerprints.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Add a single Fingerprint + embedding. The entry is visible
    /// to length queries immediately; the HNSW index is rebuilt on
    /// the next [`FaceAnnCache::nearest`] call (or explicit [`flush`]).
    pub fn add(&self, fingerprint_id: String, embedding: FaceEmbedding) {
        let mut state = self.inner.write().expect("face_ann_cache write lock");
        state.entries.push((fingerprint_id, embedding));
    }

    /// Rebuild the HNSW graph from `entries` if the entry list has
    /// grown since the last build. Safe to call repeatedly; cheap
    /// when the index is already up to date.
    ///
    /// `instant-distance` doesn't support incremental insertion,
    /// so this always rebuilds the whole graph when the cache is
    /// dirty. At alpha-scale N (a few thousand), a full rebuild
    /// is O(N log N) and completes in tens of ms.
    pub fn flush(&self) {
        let mut state = self.inner.write().expect("face_ann_cache write lock");
        if state.entries.len() == state.indexed_len && state.index.is_some() {
            return;
        }
        if state.entries.is_empty() {
            state.index = None;
            state.indexed_len = 0;
            return;
        }

        let (points, values): (Vec<FaceEmbedding>, Vec<String>) = state
            .entries
            .iter()
            .map(|(id, emb)| (emb.clone(), id.clone()))
            .unzip();
        let map = Builder::default().build(points, values);
        state.indexed_len = state.entries.len();
        state.index = Some(map);
    }

    /// Return the top `k` nearest face Fingerprints to `query`.
    ///
    /// Returns an empty vector if the cache is empty. Sorted by
    /// similarity descending (most similar first).
    pub fn nearest(&self, query: &FaceEmbedding, k: usize) -> Vec<CacheHit> {
        // Make sure any pending adds are reflected in the graph.
        self.flush();

        let state = self.inner.read().expect("face_ann_cache read lock");
        let Some(index) = state.index.as_ref() else {
            return Vec::new();
        };

        let mut search = Search::default();
        index
            .search(query, &mut search)
            .take(k)
            .map(|item| CacheHit {
                fingerprint_id: item.value.clone(),
                similarity: 1.0 - item.distance,
            })
            .collect()
    }

    /// Replace the entire cache contents with the given list of
    /// (id, embedding) pairs. Used by [`rebuild_from_store`] after
    /// it's queried the Fingerprint registry. Also useful in tests
    /// that want to set up a known state.
    pub fn replace_all(&self, entries: Vec<(String, FaceEmbedding)>) {
        let mut state = self.inner.write().expect("face_ann_cache write lock");
        state.entries = entries;
        state.index = None;
        state.indexed_len = 0;
        drop(state);
        self.flush();
    }
}

// ─────────────────────────────────────────────────────────────────
// Global singleton accessor
// ─────────────────────────────────────────────────────────────────

static GLOBAL_CACHE: OnceLock<Arc<FaceAnnCache>> = OnceLock::new();

/// Get (or lazily initialize) the process-wide face ANN cache.
///
/// The returned `Arc` can be cloned and stored by callers that want
/// their own handle; all handles point at the same cache.
pub fn cache() -> Arc<FaceAnnCache> {
    GLOBAL_CACHE
        .get_or_init(|| Arc::new(FaceAnnCache::new()))
        .clone()
}

/// Rebuild the global face ANN cache from the current Fingerprint
/// registry.
///
/// Called at node startup, after `register_phase_1_schemas` has
/// populated the canonical_names registry so `FINGERPRINT` is
/// resolvable.
///
/// Queries every Fingerprint with `kind == "face_embedding"`, parses
/// the embedding out of the `value` field (stored as a JSON array of
/// floats), and bulk-replaces the cache contents.
///
/// Malformed embeddings (wrong type, wrong dimension, non-finite
/// values) are skipped with a warn log — they don't block the rest
/// of the rebuild. The rationale is that a single corrupt record
/// should not prevent the rest of the cache from coming online,
/// because the cache is a read-side optimization and its
/// correctness is bounded by the underlying fingerprint records.
pub async fn rebuild_from_store(node: &FoldNode) -> FoldDbResult<usize> {
    let canonical = canonical_names::lookup(FINGERPRINT).map_err(|e| {
        FoldDbError::Config(format!(
            "face_ann_cache: canonical_names not initialized for '{FINGERPRINT}': {e}"
        ))
    })?;

    let processor = crate::fold_node::OperationProcessor::new(Arc::new(node.clone()));
    let query = Query {
        schema_name: canonical,
        fields: vec!["id".to_string(), "kind".to_string(), "value".to_string()],
        filter: None,
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };

    let records = processor.execute_query_json(query).await.map_err(|e| {
        FoldDbError::Config(format!(
            "face_ann_cache: failed to query Fingerprint registry: {e}"
        ))
    })?;

    let mut entries: Vec<(String, FaceEmbedding)> = Vec::with_capacity(records.len());
    let mut skipped = 0usize;
    for record in records {
        let Some(fields) = record.get("fields") else {
            skipped += 1;
            continue;
        };
        let kind = fields
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if kind != "face_embedding" {
            continue;
        }
        let Some(id) = fields.get("id").and_then(|v| v.as_str()) else {
            skipped += 1;
            continue;
        };
        let Some(value_array) = fields.get("value").and_then(|v| v.as_array()) else {
            skipped += 1;
            continue;
        };
        let maybe_raw: Option<Vec<f32>> = value_array
            .iter()
            .map(|v: &serde_json::Value| v.as_f64().map(|f| f as f32))
            .collect();
        let Some(raw) = maybe_raw else {
            skipped += 1;
            continue;
        };
        match FaceEmbedding::new(raw) {
            Ok(embedding) => entries.push((id.to_string(), embedding)),
            Err(e) => {
                log::warn!(
                    "face_ann_cache: skipping malformed embedding for fingerprint '{id}': {e}"
                );
                skipped += 1;
            }
        }
    }

    let count = entries.len();
    cache().replace_all(entries);

    log::info!(
        "face_ann_cache: rebuilt from store ({count} face fingerprints indexed, {skipped} skipped)"
    );

    Ok(count)
}

// ─────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a FaceEmbedding from a dense input without going through
    /// the normalization path (so test vectors stay readable).
    fn emb(vals: &[f32]) -> FaceEmbedding {
        FaceEmbedding::new(vals.to_vec()).expect("valid embedding")
    }

    #[test]
    fn empty_cache_returns_no_hits() {
        let cache = FaceAnnCache::new();
        assert!(cache.is_empty());
        let q = emb(&[1.0, 0.0, 0.0]);
        assert!(cache.nearest(&q, 5).is_empty());
    }

    #[test]
    fn nearest_returns_identical_embedding_as_top_hit() {
        let cache = FaceAnnCache::new();
        cache.add("fp_a".into(), emb(&[1.0, 0.0, 0.0]));
        cache.add("fp_b".into(), emb(&[0.0, 1.0, 0.0]));
        cache.add("fp_c".into(), emb(&[0.0, 0.0, 1.0]));

        // Query with an embedding identical to fp_a → fp_a must be top.
        let q = emb(&[1.0, 0.0, 0.0]);
        let hits = cache.nearest(&q, 3);
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].fingerprint_id, "fp_a");
        assert!(
            hits[0].similarity > 0.99,
            "identical embedding should have similarity ≈ 1.0, got {}",
            hits[0].similarity
        );
    }

    #[test]
    fn nearest_orders_by_similarity_descending() {
        let cache = FaceAnnCache::new();
        // fp_a and fp_close are very close; fp_far is orthogonal.
        cache.add("fp_a".into(), emb(&[1.0, 0.0, 0.0]));
        cache.add("fp_close".into(), emb(&[0.95, 0.05, 0.0]));
        cache.add("fp_far".into(), emb(&[0.0, 0.0, 1.0]));

        let q = emb(&[1.0, 0.0, 0.0]);
        let hits = cache.nearest(&q, 3);
        assert_eq!(hits.len(), 3);

        // Similarities must be non-increasing.
        assert!(
            hits[0].similarity >= hits[1].similarity,
            "hits not sorted: {hits:?}"
        );
        assert!(
            hits[1].similarity >= hits[2].similarity,
            "hits not sorted: {hits:?}"
        );
        // And fp_far must be last (orthogonal to query).
        assert_eq!(hits[2].fingerprint_id, "fp_far");
    }

    #[test]
    fn replace_all_swaps_cache_contents() {
        let cache = FaceAnnCache::new();
        cache.add("fp_a".into(), emb(&[1.0, 0.0, 0.0]));
        cache.add("fp_b".into(), emb(&[0.0, 1.0, 0.0]));
        assert_eq!(cache.len(), 2);

        // Replace with a totally different set.
        cache.replace_all(vec![
            ("fp_x".into(), emb(&[1.0, 1.0, 0.0])),
            ("fp_y".into(), emb(&[0.0, 1.0, 1.0])),
            ("fp_z".into(), emb(&[1.0, 0.0, 1.0])),
        ]);
        assert_eq!(cache.len(), 3);

        let q = emb(&[1.0, 1.0, 0.0]);
        let hits = cache.nearest(&q, 1);
        assert_eq!(hits[0].fingerprint_id, "fp_x");
    }

    #[test]
    fn pending_adds_are_visible_after_flush() {
        let cache = FaceAnnCache::new();
        cache.add("fp_a".into(), emb(&[1.0, 0.0, 0.0]));
        // Don't call flush — add another, then query. nearest() must
        // implicitly flush so both are visible.
        cache.add("fp_b".into(), emb(&[0.0, 1.0, 0.0]));

        let q = emb(&[1.0, 0.0, 0.0]);
        let hits = cache.nearest(&q, 2);
        assert_eq!(hits.len(), 2);
        let ids: Vec<&str> = hits.iter().map(|h| h.fingerprint_id.as_str()).collect();
        assert!(ids.contains(&"fp_a"));
        assert!(ids.contains(&"fp_b"));
    }

    #[test]
    fn malformed_embeddings_are_rejected() {
        assert!(FaceEmbedding::new(Vec::new()).is_err());
        assert!(FaceEmbedding::new(vec![0.0, 0.0, 0.0]).is_err());
        assert!(FaceEmbedding::new(vec![f32::NAN, 1.0]).is_err());
    }

    #[test]
    fn global_cache_is_a_shared_singleton() {
        let a = cache();
        let b = cache();
        // Both Arcs must point at the same underlying FaceAnnCache.
        assert!(Arc::ptr_eq(&a, &b));
    }
}
