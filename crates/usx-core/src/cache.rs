//! Disk cache for downloaded data with a size limit and least-recently-used eviction.
//!
//! Files live at `<root>/<key>`; a file's modification time is its last-use time, so
//! the cache needs no separate index on disk and survives crashes.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Marker body stored for resources the server reported missing, so we do not ask again.
pub const MISSING: &[u8] = b"\0missing";

struct Index {
    /// key -> (size, last use in ms)
    files: HashMap<String, (u64, u64)>,
    /// (last use, key) ordering for eviction
    order: BTreeSet<(u64, String)>,
    total: u64,
}

pub struct DiskCache {
    root: PathBuf,
    limit: Mutex<u64>,
    index: Mutex<Index>,
    /// Bytes read from disk and time spent reading (for the info overlay).
    read_bytes: std::sync::atomic::AtomicU64,
    read_ns: std::sync::atomic::AtomicU64,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl DiskCache {
    /// Open (and index) a cache directory. Scanning ~100k files takes well under a second.
    pub fn open(root: impl Into<PathBuf>, limit_bytes: u64) -> Self {
        let root = root.into();
        let _ = fs::create_dir_all(&root);
        let mut idx = Index { files: HashMap::new(), order: BTreeSet::new(), total: 0 };
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let Ok(md) = e.metadata() else { continue };
                if md.is_dir() {
                    stack.push(e.path());
                } else if let Ok(rel) = e.path().strip_prefix(&root) {
                    let key = rel.to_string_lossy().replace('\\', "/");
                    if key.ends_with(".tmp") {
                        let _ = fs::remove_file(e.path());
                        continue;
                    }
                    let t = md.modified().ok().and_then(|m| m.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
                    idx.total += md.len();
                    idx.order.insert((t, key.clone()));
                    idx.files.insert(key, (md.len(), t));
                }
            }
        }
        let c = Self {
            root,
            limit: Mutex::new(limit_bytes),
            index: Mutex::new(idx),
            read_bytes: Default::default(),
            read_ns: Default::default(),
        };
        c.evict();
        c
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn size(&self) -> u64 {
        self.index.lock().unwrap().total
    }

    pub fn len(&self) -> usize {
        self.index.lock().unwrap().files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn limit(&self) -> u64 {
        *self.limit.lock().unwrap()
    }

    pub fn set_limit(&self, bytes: u64) {
        *self.limit.lock().unwrap() = bytes;
        self.evict();
    }

    pub fn contains(&self, key: &str) -> bool {
        self.index.lock().unwrap().files.contains_key(key)
    }

    /// Read an entry and mark it as recently used.
    /// Total bytes read from the cache and the seconds spent reading them.
    pub fn read_stats(&self) -> (u64, f64) {
        (self.read_bytes.load(std::sync::atomic::Ordering::Relaxed), self.read_ns.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e9)
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let path = self.root.join(key);
        let started = std::time::Instant::now();
        let data = fs::read(&path).ok()?;
        self.read_ns.fetch_add(started.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        self.read_bytes.fetch_add(data.len() as u64, std::sync::atomic::Ordering::Relaxed);
        let t = now_ms();
        {
            let mut idx = self.index.lock().unwrap();
            if let Some(&(sz, old)) = idx.files.get(key) {
                idx.order.remove(&(old, key.to_string()));
                idx.order.insert((t, key.to_string()));
                idx.files.insert(key.to_string(), (sz, t));
            }
        }
        if let Ok(f) = fs::File::options().write(true).open(&path) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(data)
    }

    /// Store an entry atomically (write to a temp file, then rename).
    pub fn put(&self, key: &str, data: &[u8]) {
        let path = self.root.join(key);
        if let Some(p) = path.parent() {
            let _ = fs::create_dir_all(p);
        }
        let tmp = path.with_extension("tmp");
        if fs::write(&tmp, data).is_err() || fs::rename(&tmp, &path).is_err() {
            let _ = fs::remove_file(&tmp);
            return;
        }
        let t = now_ms();
        {
            let mut idx = self.index.lock().unwrap();
            if let Some((sz, old)) = idx.files.remove(key) {
                idx.total -= sz;
                idx.order.remove(&(old, key.to_string()));
            }
            idx.total += data.len() as u64;
            idx.order.insert((t, key.to_string()));
            idx.files.insert(key.to_string(), (data.len() as u64, t));
        }
        self.evict();
    }

    /// Remove least-recently-used entries until the cache is within 95% of its limit.
    pub fn evict(&self) {
        let limit = self.limit();
        let mut idx = self.index.lock().unwrap();
        if idx.total <= limit {
            return;
        }
        let target = limit / 20 * 19;
        while idx.total > target {
            let Some((_, key)) = idx.order.pop_first() else {
                break;
            };
            if let Some((sz, _)) = idx.files.remove(&key) {
                idx.total -= sz;
                let _ = fs::remove_file(self.root.join(&key));
            }
        }
    }

    /// Delete everything.
    pub fn clear(&self) {
        let mut idx = self.index.lock().unwrap();
        for k in idx.files.keys() {
            let _ = fs::remove_file(self.root.join(k));
        }
        idx.files.clear();
        idx.order.clear();
        idx.total = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used() {
        let dir = std::env::temp_dir().join(format!("usx-cache-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let c = DiskCache::open(&dir, 3000);
        c.put("a/1", &[0; 1000]);
        std::thread::sleep(std::time::Duration::from_millis(5));
        c.put("a/2", &[0; 1000]);
        std::thread::sleep(std::time::Duration::from_millis(5));
        c.get("a/1"); // a/1 is now newer than a/2
        std::thread::sleep(std::time::Duration::from_millis(5));
        c.put("a/3", &[0; 1500]);
        assert!(c.contains("a/1") && c.contains("a/3") && !c.contains("a/2"));
        let reopened = DiskCache::open(&dir, 3000);
        assert_eq!(reopened.size(), 2500);
        let _ = fs::remove_dir_all(&dir);
    }
}
