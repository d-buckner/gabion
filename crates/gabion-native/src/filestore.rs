//! File-backed `Store` for native hosts: one JSON object on disk.
//!
//! The map is loaded once and kept in memory, shared by every clone. That is
//! what makes a real board usable: the UI asks for one watch's history per row,
//! and re-reading and re-parsing the whole file for each ask turns a 150-row
//! status page into seconds of waiting. Reads are map lookups; every mutation
//! updates the map and rewrites the file, so the single-writer contract still
//! holds. A second process editing the file underneath this one is not
//! observed — that was already last-write-wins before the cache.
//!
//! Deliberately simple; redb is the planned v1 impl. Lives in gabion-native
//! (host side) precisely because it touches std::fs — which core may never do.

use gabion::async_trait;
use gabion::caps::{Store, StoreError};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

struct Inner {
    path: PathBuf,
    map: Option<BTreeMap<String, String>>,
}

#[derive(Clone)]
pub struct FileStore {
    inner: Arc<Mutex<Inner>>,
}

impl FileStore {
    /// Creates the parent directory if needed; a missing file is an empty store.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| StoreError::Io(e.to_string()))?;
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner { path, map: None })),
        })
    }
}

fn lock(inner: &Arc<Mutex<Inner>>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Inner {
    /// The map, read from disk on first use. A corrupt file is an error, not an
    /// empty store: silently dropping every watch is the fail-open this store's
    /// callers are written to avoid.
    fn map(&mut self) -> Result<&mut BTreeMap<String, String>, StoreError> {
        if self.map.is_none() {
            let parsed = match std::fs::read_to_string(&self.path) {
                Ok(text) => {
                    serde_json::from_str(&text).map_err(|e| StoreError::Io(e.to_string()))?
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
                Err(e) => return Err(StoreError::Io(e.to_string())),
            };
            self.map = Some(parsed);
        }
        self.map
            .as_mut()
            .ok_or_else(|| StoreError::Io("store was not loaded".to_string()))
    }

    fn flush(&mut self) -> Result<(), StoreError> {
        let text = {
            let map = self.map()?;
            serde_json::to_string_pretty(map).map_err(|e| StoreError::Io(e.to_string()))?
        };
        std::fs::write(&self.path, text).map_err(|e| StoreError::Io(e.to_string()))
    }
}

#[async_trait]
impl Store for FileStore {
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(lock(&self.inner).map()?.get(key).cloned())
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        let mut inner = lock(&self.inner);
        inner.map()?.insert(key.into(), value.into());
        inner.flush()
    }

    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        let mut inner = lock(&self.inner);
        inner.map()?.remove(key);
        inner.flush()
    }

    async fn list(&self, prefix: &str) -> Result<Vec<(String, String)>, StoreError> {
        let mut inner = lock(&self.inner);
        let map = inner.map()?;
        Ok(map
            .range(prefix.to_string()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }
}
