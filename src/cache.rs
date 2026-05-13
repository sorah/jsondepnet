//! On-disk cache format and the in-memory `Cache` wrapper.
//!
//! The on-disk format is intentionally versioned: a `version` field gates
//! deserialisation, and unknown versions are rejected outright so a future v2
//! cache won't silently misread.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use crate::paths::RelPath;

pub const CACHE_VERSION: u32 = 1;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct CacheFile {
    pub version: u32,
    pub data: CacheData,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct CacheData {
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileEntry {
    pub path: RelPath,
    pub mtime: i64,
    pub imports: Vec<RelPath>,
}

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("unsupported cache version: {0} (expected {1})")]
    UnsupportedVersion(u32, u32),
    #[error("failed to parse cache file: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("io error on {path:?}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// In-memory representation of the cache, keyed by relative path for O(log n)
/// lookups. Serialises back to a sorted list on disk for stable diffs.
#[derive(Debug, Default, Clone)]
pub struct Cache {
    entries: BTreeMap<RelPath, FileEntry>,
}

impl Cache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_json(s: &str) -> Result<Self, CacheError> {
        let parsed: CacheFile = serde_json::from_str(s)?;
        if parsed.version != CACHE_VERSION {
            return Err(CacheError::UnsupportedVersion(
                parsed.version,
                CACHE_VERSION,
            ));
        }
        let mut entries = BTreeMap::new();
        for entry in parsed.data.files {
            entries.insert(entry.path.clone(), entry);
        }
        Ok(Self { entries })
    }

    pub fn to_json_pretty(&self) -> String {
        let mut files: Vec<FileEntry> = self.entries.values().cloned().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let file = CacheFile {
            version: CACHE_VERSION,
            data: CacheData { files },
        };
        serde_json::to_string_pretty(&file).expect("CacheFile is always serialisable")
    }

    pub fn load_or_default(path: &Path) -> Result<Self, CacheError> {
        match std::fs::read_to_string(path) {
            Ok(s) => Self::from_json(&s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(source) => Err(CacheError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Atomically write the cache to disk via a `<path>.tmp` -> `rename` dance.
    pub fn save_atomic(&self, path: &Path) -> Result<(), CacheError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|source| CacheError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let tmp = path.with_extension("tmp");
        let body = self.to_json_pretty();
        {
            let mut f = std::fs::File::create(&tmp).map_err(|source| CacheError::Io {
                path: tmp.clone(),
                source,
            })?;
            f.write_all(body.as_bytes())
                .map_err(|source| CacheError::Io {
                    path: tmp.clone(),
                    source,
                })?;
            f.write_all(b"\n").map_err(|source| CacheError::Io {
                path: tmp.clone(),
                source,
            })?;
            f.sync_all().map_err(|source| CacheError::Io {
                path: tmp.clone(),
                source,
            })?;
        }
        std::fs::rename(&tmp, path).map_err(|source| CacheError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }

    pub fn upsert(&mut self, entry: FileEntry) {
        self.entries.insert(entry.path.clone(), entry);
    }

    pub fn remove(&mut self, path: &RelPath) -> Option<FileEntry> {
        self.entries.remove(path)
    }

    pub fn get(&self, path: &RelPath) -> Option<&FileEntry> {
        self.entries.get(path)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&RelPath, &FileEntry)> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn paths(&self) -> impl Iterator<Item = &RelPath> {
        self.entries.keys()
    }

    /// Build a reverse index: for each path, the set of files that import it.
    /// Sorted by importer path for deterministic traversal order.
    pub fn build_reverse(&self) -> BTreeMap<RelPath, Vec<RelPath>> {
        let mut out: BTreeMap<RelPath, Vec<RelPath>> = BTreeMap::new();
        for (path, entry) in &self.entries {
            for dep in &entry.imports {
                out.entry(dep.clone()).or_default().push(path.clone());
            }
        }
        for v in out.values_mut() {
            v.sort();
            v.dedup();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, mtime: i64, imports: &[&str]) -> FileEntry {
        FileEntry {
            path: RelPath::from_components(&path.split('/').collect::<Vec<_>>()),
            mtime,
            imports: imports
                .iter()
                .map(|s| RelPath::from_components(&s.split('/').collect::<Vec<_>>()))
                .collect(),
        }
    }

    #[test]
    fn roundtrip_json() {
        let mut cache = Cache::new();
        cache.upsert(entry("a.jsonnet", 100, &["b.libsonnet", "c.libsonnet"]));
        cache.upsert(entry("b.libsonnet", 200, &[]));
        let json = cache.to_json_pretty();
        let back = Cache::from_json(&json).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(
            back.get(&RelPath::from_components(&["a.jsonnet"])).unwrap(),
            cache
                .get(&RelPath::from_components(&["a.jsonnet"]))
                .unwrap()
        );
    }

    #[test]
    fn rejects_unknown_version() {
        let bad = r#"{"version": 2, "data": {"files": []}}"#;
        let err = Cache::from_json(bad).unwrap_err();
        match err {
            CacheError::UnsupportedVersion(2, 1) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn rejects_malformed_json() {
        let err = Cache::from_json("{ not json").unwrap_err();
        assert!(matches!(err, CacheError::Parse(_)));
    }

    #[test]
    fn load_or_default_missing_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("missing.json");
        let cache = Cache::load_or_default(&p).unwrap();
        assert!(cache.is_empty());
    }

    #[test]
    fn upsert_replaces_by_path() {
        let mut cache = Cache::new();
        cache.upsert(entry("a.jsonnet", 100, &["b.libsonnet"]));
        cache.upsert(entry("a.jsonnet", 200, &["c.libsonnet"]));
        let stored = cache
            .get(&RelPath::from_components(&["a.jsonnet"]))
            .unwrap();
        assert_eq!(stored.mtime, 200);
        assert_eq!(stored.imports[0].as_str(), "c.libsonnet");
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn remove_drops_entry() {
        let mut cache = Cache::new();
        cache.upsert(entry("a.jsonnet", 100, &[]));
        let removed = cache.remove(&RelPath::from_components(&["a.jsonnet"]));
        assert!(removed.is_some());
        assert!(cache.is_empty());
    }

    #[test]
    fn build_reverse_diamond_and_isolated() {
        let mut cache = Cache::new();
        cache.upsert(entry("a.jsonnet", 0, &["b.libsonnet", "c.libsonnet"]));
        cache.upsert(entry("b.libsonnet", 0, &["d.libsonnet"]));
        cache.upsert(entry("c.libsonnet", 0, &["d.libsonnet"]));
        cache.upsert(entry("d.libsonnet", 0, &[]));
        cache.upsert(entry("isolated.libsonnet", 0, &[]));

        let rev = cache.build_reverse();
        assert_eq!(
            rev[&RelPath::from_components(&["d.libsonnet"])]
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>(),
            vec!["b.libsonnet", "c.libsonnet"]
        );
        assert_eq!(
            rev[&RelPath::from_components(&["b.libsonnet"])]
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>(),
            vec!["a.jsonnet"]
        );
        assert!(!rev.contains_key(&RelPath::from_components(&["isolated.libsonnet"])));
    }

    #[test]
    fn build_reverse_handles_cycle() {
        let mut cache = Cache::new();
        cache.upsert(entry("a.libsonnet", 0, &["b.libsonnet"]));
        cache.upsert(entry("b.libsonnet", 0, &["a.libsonnet"]));
        let rev = cache.build_reverse();
        assert_eq!(rev.len(), 2);
    }

    #[test]
    fn save_atomic_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("cache.json");
        let mut cache = Cache::new();
        cache.upsert(entry("a.jsonnet", 100, &["b.libsonnet"]));
        cache.save_atomic(&p).unwrap();
        let back = Cache::load_or_default(&p).unwrap();
        assert_eq!(back.len(), 1);
    }

    #[test]
    fn save_atomic_sorted_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("cache.json");
        let mut cache = Cache::new();
        cache.upsert(entry("z.jsonnet", 0, &[]));
        cache.upsert(entry("a.jsonnet", 0, &[]));
        cache.save_atomic(&p).unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        let pa = body.find("a.jsonnet").unwrap();
        let pz = body.find("z.jsonnet").unwrap();
        assert!(pa < pz, "expected sorted order on disk");
    }
}
