//! File-backed `Store` for native hosts: one JSON object on disk, rewritten on
//! write. Deliberately simple — redb is the planned v1 impl; this exists so the
//! CLI demo persists state across process invocations. Lives in gabion-native
//! (host side) precisely because it touches std::fs — which core may never do.

use gabion::async_trait;
use gabion::caps::{Store, StoreError};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct FileStore {
    path: Arc<Mutex<PathBuf>>,
}

impl FileStore {
    /// Creates the parent directory if needed; a missing file is an empty store.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| StoreError::Io(e.to_string()))?;
        }
        Ok(Self { path: Arc::new(Mutex::new(path)) })
    }

    fn load(&self) -> Result<BTreeMap<String, String>, StoreError> {
        let p = self.path.lock().map_err(|e| StoreError::Io(e.to_string()))?.clone();
        match std::fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| StoreError::Io(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(StoreError::Io(e.to_string())),
        }
    }

    fn save(&self, map: &BTreeMap<String, String>) -> Result<(), StoreError> {
        let p = self.path.lock().map_err(|e| StoreError::Io(e.to_string()))?.clone();
        let s = serde_json::to_string_pretty(map).map_err(|e| StoreError::Io(e.to_string()))?;
        std::fs::write(&p, s).map_err(|e| StoreError::Io(e.to_string()))
    }
}

#[async_trait]
impl Store for FileStore {
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self.load()?.get(key).cloned())
    }
    async fn set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        let mut map = self.load()?;
        map.insert(key.into(), value.into());
        self.save(&map)
    }
    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        let mut map = self.load()?;
        map.remove(key);
        self.save(&map)
    }
    async fn list(&self, prefix: &str) -> Result<Vec<(String, String)>, StoreError> {
        Ok(self
            .load()?
            .range(prefix.to_string()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }
}
