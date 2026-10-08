//! Per-source diff cache for the web viewer.
//!
//! Every diff the viewer shows is addressed by its inputs (the `a`/`b` or
//! `diff` query parameters), so several users can look at different diffs on
//! the same server, and any replica can serve any URL by computing the diff
//! on first use. Computed diffs are kept in a small LRU cache keyed by the
//! resolved paths, their size and modification time, and the diff options, so
//! a file that changes on disk is diffed again.

use crate::diff::DiffResult;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::OnceCell;

type Slot = Arc<OnceCell<Arc<DiffResult>>>;

pub struct DiffCache {
    capacity: usize,
    /// Most recently used first.
    entries: Mutex<VecDeque<(String, Slot)>>,
}

impl DiffCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: Mutex::new(VecDeque::new()),
        }
    }

    fn slot(&self, key: &str) -> Slot {
        let mut entries = self.entries.lock().unwrap();
        let slot = match entries.iter().position(|(k, _)| k == key) {
            Some(i) => entries.remove(i).unwrap().1,
            None => Slot::default(),
        };
        entries.push_front((key.to_string(), slot.clone()));
        entries.truncate(self.capacity);
        slot
    }

    /// Return the cached diff for `key`, computing it with `compute` on a
    /// blocking thread if needed. Concurrent requests for the same key share
    /// one computation; a failed computation is not cached.
    pub async fn get_or_compute<F>(&self, key: &str, compute: F) -> anyhow::Result<Arc<DiffResult>>
    where
        F: FnOnce() -> anyhow::Result<DiffResult> + Send + 'static,
    {
        let slot = self.slot(key);
        let result = slot
            .get_or_try_init(|| async move {
                let mut d = tokio::task::spawn_blocking(compute)
                    .await
                    .map_err(|e| anyhow::anyhow!("diff task panicked: {e}"))??;
                d.sort_rows();
                Ok::<_, anyhow::Error>(Arc::new(d))
            })
            .await?;
        Ok(result.clone())
    }

    #[cfg(test)]
    fn keys(&self) -> Vec<String> {
        let entries = self.entries.lock().unwrap();
        entries.iter().map(|(k, _)| k.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{DiffInputs, compute_diff};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn inputs() -> DiffInputs {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        DiffInputs {
            file_a: dir.join("a.ttl"),
            file_b: dir.join("b.ttl"),
            format_a: None,
            format_b: None,
            graph_a: None,
            graph_b: None,
            ignore_blank_nodes: false,
        }
    }

    #[tokio::test]
    async fn computes_once_per_key_and_evicts_lru() {
        let cache = DiffCache::new(2);
        let calls = Arc::new(AtomicUsize::new(0));
        for key in ["x", "x", "y", "x", "z"] {
            let calls = calls.clone();
            cache
                .get_or_compute(key, move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    compute_diff(&inputs())
                })
                .await
                .unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(cache.keys(), vec!["z", "x"]);
    }

    #[tokio::test]
    async fn errors_are_not_cached() {
        let cache = DiffCache::new(2);
        assert!(
            cache
                .get_or_compute("k", || anyhow::bail!("boom"))
                .await
                .is_err()
        );
        assert!(
            cache
                .get_or_compute("k", || compute_diff(&inputs()))
                .await
                .is_ok()
        );
    }
}
