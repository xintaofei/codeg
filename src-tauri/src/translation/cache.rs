//! Two-tier translation cache: an in-memory LRU and a per-language JSON file.
//!
//! The disk tier survives restarts so a phrase translated once stays local;
//! the in-memory tier avoids re-reading that file for every message. Both are
//! keyed by `sha256(masked_text:target_lang:provider_id)` — content, never an
//! object reference — which is what keeps a `parts` array replacement (stream
//! → promoted turn → authoritative refetch) from invalidating a hit.

use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// In-memory entry ceiling. No `lru` crate in the tree and no appetite to add
/// one, so recency is a `Vec` of keys, newest first — at 2000 entries the
/// linear `position` scan is far cheaper than the HTTP call it prevents.
const MAX_ENTRIES: usize = 2000;

/// Per-language file ceiling. Exceeding it drops the oldest entries rather
/// than the newest: recent messages are the ones about to be re-rendered.
const MAX_DISK_BYTES: u64 = 10 * 1024 * 1024;

/// Leads every cache key. Bump when a fix changes what a correct translation
/// looks like, so entries written under the old request shape miss instead of
/// being served forever — see [`TranslationCache::key_for`].
///
/// `v3-length-gate`: entries written before the expansion gate existed include
/// endpoint hallucinations (a self-written essay served for a one-line source)
/// that the gate now refuses — they must miss, not replay forever.
/// `v4-script-gate`: entries written before the echo/refusal gate existed
/// include English-in-English echoes and bare refusals served as
/// "translations" — they must miss, not replay forever.
const KEY_VERSION: &str = "v4-script-gate";

struct Lru {
    entries: HashMap<String, String>,
    /// Keys, most-recently-used first.
    recency: Vec<String>,
}

impl Lru {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            recency: Vec::new(),
        }
    }

    fn get(&mut self, key: &str) -> Option<String> {
        let value = self.entries.get(key).cloned()?;
        self.touch(key);
        Some(value)
    }

    fn insert(&mut self, key: String, value: String) {
        if self.entries.contains_key(&key) {
            self.touch(&key);
        } else {
            self.recency.insert(0, key.clone());
        }
        self.entries.insert(key, value);
        while self.entries.len() > MAX_ENTRIES {
            if let Some(evicted) = self.recency.pop() {
                self.entries.remove(&evicted);
            } else {
                break;
            }
        }
    }

    fn touch(&mut self, key: &str) {
        if let Some(pos) = self.recency.iter().position(|k| k == key) {
            let key = self.recency.remove(pos);
            self.recency.insert(0, key);
        }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.recency.clear();
    }

    /// The key that would be evicted next. Test-only view of the policy.
    #[cfg(test)]
    fn oldest(&self) -> Option<&String> {
        self.recency.last()
    }
}

/// One persisted translation. The on-disk file is a plain array of these.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CachedTranslation {
    pub key: String,
    pub text: String,
}

/// Entry counts for the settings page.
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranslationCacheStats {
    pub memory_entries: usize,
    pub disk_entries: usize,
    pub disk_bytes: u64,
}

struct Inner {
    mem: Lru,
    root: PathBuf,
}

pub struct TranslationCache {
    inner: Mutex<Inner>,
}

impl fmt::Debug for TranslationCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TranslationCache").finish_non_exhaustive()
    }
}

impl TranslationCache {
    /// Rooted at `<cache dir>/translation/`.
    pub fn new(root: PathBuf) -> Self {
        Self {
            inner: Mutex::new(Inner {
                mem: Lru::new(),
                root,
            }),
        }
    }

    /// Content-addressed key. `provider_id` participates so switching endpoint
    /// or model never serves output produced by the previous one.
    ///
    /// [`KEY_VERSION`] leads the hash: when a request-shape fix changes what a
    /// correct translation looks like (the `max_tokens` fix that stopped
    /// relays from silently truncating), entries written under the old shape
    /// — including the truncated ones it poisoned — must miss, and only a key
    /// change does that without a manual cache wipe.
    ///
    /// Each component is length-prefixed (`<len>:<bytes>`) so two triples whose
    /// rendered strings happen to be byte-identical can never collide: the
    /// boundary between components is carried by the lengths, not by a
    /// separator that might appear inside a masked text.
    pub fn key_for(masked_text: &str, target_lang: &str, provider_id: &str) -> String {
        let mut hasher = Sha256::new();
        for component in [KEY_VERSION, masked_text, target_lang, provider_id] {
            hasher.update(component.len().to_string().as_bytes());
            hasher.update(b":");
            hasher.update(component.as_bytes());
        }
        format!("{:x}", hasher.finalize())
    }

    /// Memory first, then the language file. `None` when absent or unreadable.
    pub fn get(&self, masked_text: &str, target_lang: &str, provider_id: &str) -> Option<String> {
        let key = Self::key_for(masked_text, target_lang, provider_id);
        let mut inner = self.inner.lock().ok()?;

        if let Some(hit) = inner.mem.get(&key) {
            return Some(hit);
        }

        let path = inner.root.join(lang_file(target_lang));
        let found = load_lang_file(&path)
            .into_iter()
            .find(|entry| entry.key == key)?;
        // Promote so a message re-rendered on scroll does not re-read the file.
        inner.mem.insert(key, found.text.clone());
        Some(found.text)
    }

    /// Store in both tiers. A failed disk write is logged and dropped — the
    /// caller already has the translation and must not fail over a cache miss
    /// that costs one refetch.
    pub fn insert(
        &self,
        masked_text: &str,
        target_lang: &str,
        provider_id: &str,
        translated: &str,
    ) {
        let key = Self::key_for(masked_text, target_lang, provider_id);
        let path = {
            let Ok(mut inner) = self.inner.lock() else {
                return;
            };
            let path = inner.root.join(lang_file(target_lang));
            inner.mem.insert(key.clone(), translated.to_string());
            path
        };

        let entry = CachedTranslation {
            key,
            text: translated.to_string(),
        };
        if let Err(err) = persist(&path, entry) {
            tracing::warn!("[translation] cache write failed: {err}");
        }
    }

    pub fn stats(&self) -> TranslationCacheStats {
        let Ok(inner) = self.inner.lock() else {
            return TranslationCacheStats::default();
        };
        let mut disk_entries = 0;
        let mut disk_bytes = 0;
        if let Ok(read) = std::fs::read_dir(&inner.root) {
            for file in read.flatten() {
                let path = file.path();
                if path.extension().is_some_and(|ext| ext == "json") {
                    disk_bytes += file.metadata().map(|m| m.len()).unwrap_or(0);
                    disk_entries += load_lang_file(&path).len();
                }
            }
        }
        TranslationCacheStats {
            memory_entries: inner.mem.len(),
            disk_entries,
            disk_bytes,
        }
    }

    pub fn clear(&self) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.mem.clear();
        if let Ok(read) = std::fs::read_dir(&inner.root) {
            for file in read.flatten() {
                let _ = std::fs::remove_file(file.path());
            }
        }
    }
}

/// Language identifiers reach this from settings and could contain a path
/// separator; keep the filename to characters that cannot escape the root.
fn lang_file(target_lang: &str) -> String {
    let safe: String = target_lang
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}.json")
}

fn load_lang_file(path: &Path) -> Vec<CachedTranslation> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Upsert one entry into the language file, trimming the oldest until the
/// serialized form fits under [`MAX_DISK_BYTES`].
fn persist(path: &Path, entry: CachedTranslation) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut entries = load_lang_file(path);
    match entries.iter().position(|e| e.key == entry.key) {
        Some(idx) => entries[idx] = entry,
        None => entries.push(entry),
    }

    loop {
        let bytes = serde_json::to_vec(&entries)?;
        // `len() <= 1` is the floor: a single entry larger than the cap cannot
        // be trimmed any further, and dropping it would make the file useless
        // rather than merely large.
        if bytes.len() as u64 <= MAX_DISK_BYTES || entries.len() <= 1 {
            let mut file = std::fs::File::create(path)?;
            file.write_all(&bytes)?;
            return Ok(());
        }
        entries.remove(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> (TranslationCache, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = TranslationCache::new(dir.path().to_path_buf());
        (cache, dir)
    }

    #[test]
    fn the_key_is_stable_for_the_same_input() {
        let a = TranslationCache::key_for("hello", "zh-CN", "p1");
        let b = TranslationCache::key_for("hello", "zh-CN", "p1");
        assert_eq!(a, b);
    }

    /// Each component must change the key, or a language or provider switch
    /// would serve the previous one's output.
    #[test]
    fn every_key_component_changes_the_key() {
        let base = TranslationCache::key_for("hello", "zh-CN", "p1");
        assert_ne!(base, TranslationCache::key_for("hello!", "zh-CN", "p1"));
        assert_ne!(base, TranslationCache::key_for("hello", "ja", "p1"));
        assert_ne!(base, TranslationCache::key_for("hello", "zh-CN", "p2"));
    }

    /// The separator must not let two different triples collide by shifting
    /// the boundary between components.
    #[test]
    fn component_boundaries_do_not_collide() {
        assert_ne!(
            TranslationCache::key_for("a:b", "c", "d"),
            TranslationCache::key_for("a", "b:c", "d")
        );
    }

    #[test]
    fn a_stored_translation_reads_back() {
        let (cache, _dir) = cache();
        cache.insert("hello", "zh-CN", "p1", "你好");
        assert_eq!(cache.get("hello", "zh-CN", "p1").as_deref(), Some("你好"));
    }

    #[test]
    fn a_miss_is_none() {
        let (cache, _dir) = cache();
        assert!(cache.get("nothing", "zh-CN", "p1").is_none());
    }

    /// The disk tier is the point of the cache: a fresh process must find what
    /// the previous one wrote.
    #[test]
    fn translations_survive_a_new_cache_over_the_same_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = TranslationCache::new(dir.path().to_path_buf());
        first.insert("hello", "zh-CN", "p1", "你好");

        let second = TranslationCache::new(dir.path().to_path_buf());
        assert_eq!(second.get("hello", "zh-CN", "p1").as_deref(), Some("你好"));
    }

    #[test]
    fn re_inserting_a_key_overwrites_rather_than_duplicates() {
        let (cache, dir) = cache();
        cache.insert("hello", "zh-CN", "p1", "你好");
        cache.insert("hello", "zh-CN", "p1", "您好");

        assert_eq!(cache.get("hello", "zh-CN", "p1").as_deref(), Some("您好"));
        let entries = load_lang_file(&dir.path().join("zh-CN.json"));
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn languages_are_kept_in_separate_files() {
        let (cache, dir) = cache();
        cache.insert("hello", "zh-CN", "p1", "你好");
        cache.insert("hello", "ja", "p1", "こんにちは");

        assert!(dir.path().join("zh-CN.json").exists());
        assert!(dir.path().join("ja.json").exists());
        assert_eq!(cache.get("hello", "ja", "p1").as_deref(), Some("こんにちは"));
    }

    /// A language string is user-supplied; it must not be able to write
    /// outside the cache root.
    #[test]
    fn a_traversing_language_name_cannot_escape_the_root() {
        assert_eq!(lang_file("../../evil"), "______evil.json");
        assert_eq!(lang_file("zh-CN"), "zh-CN.json");
        assert_eq!(lang_file("a/b"), "a_b.json");
    }

    #[test]
    fn the_memory_tier_stops_at_the_entry_cap() {
        let mut lru = Lru::new();
        for i in 0..MAX_ENTRIES + 500 {
            lru.insert(format!("k{i}"), format!("v{i}"));
        }
        assert_eq!(lru.len(), MAX_ENTRIES);
    }

    #[test]
    fn the_cap_boundary_is_exact() {
        for (inserted, expected) in [
            (MAX_ENTRIES - 1, MAX_ENTRIES - 1),
            (MAX_ENTRIES, MAX_ENTRIES),
            (MAX_ENTRIES + 1, MAX_ENTRIES),
        ] {
            let mut lru = Lru::new();
            for i in 0..inserted {
                lru.insert(format!("k{i}"), String::new());
            }
            assert_eq!(lru.len(), expected, "after inserting {inserted}");
        }
    }

    /// Eviction must take the least-recently-*used* entry, not the
    /// least-recently-inserted — otherwise a hot entry inserted early is
    /// thrown away while cold newer ones survive.
    #[test]
    fn eviction_takes_the_least_recently_used_entry() {
        let mut lru = Lru::new();
        for i in 0..MAX_ENTRIES {
            lru.insert(format!("k{i}"), String::new());
        }
        // Re-read the oldest insert, making "k1" the coldest instead.
        assert!(lru.get("k0").is_some());
        assert_eq!(lru.oldest(), Some(&"k1".to_string()));

        lru.insert("fresh".to_string(), String::new());
        assert!(lru.get("k0").is_some(), "the touched entry must survive");
        assert!(lru.get("k1").is_none(), "the coldest entry is evicted");
    }

    #[test]
    fn the_disk_file_is_trimmed_to_the_byte_cap() {
        let (cache, dir) = cache();
        let big = "x".repeat(64 * 1024);
        // Enough oversized entries to force the cap.
        for i in 0..200 {
            cache.insert(&format!("src{i}"), "zh-CN", "p1", &big);
        }

        let path = dir.path().join("zh-CN.json");
        let size = std::fs::metadata(&path).expect("cache file").len();
        assert!(
            size <= MAX_DISK_BYTES,
            "cache file grew to {size} bytes, past the {MAX_DISK_BYTES} cap"
        );
        // Trimming drops the oldest, so the newest write must still be there.
        assert_eq!(cache.get("src199", "zh-CN", "p1").as_deref(), Some(&big[..]));
    }

    /// A single entry over the cap cannot be trimmed further; the write must
    /// still land rather than loop or fail.
    #[test]
    fn one_oversized_entry_is_still_written() {
        let (cache, dir) = cache();
        let huge = "x".repeat(MAX_DISK_BYTES as usize + 1024);
        cache.insert("src", "zh-CN", "p1", &huge);

        assert!(dir.path().join("zh-CN.json").exists());
        assert_eq!(cache.get("src", "zh-CN", "p1").as_deref(), Some(&huge[..]));
    }

    #[test]
    fn a_corrupt_file_reads_as_empty_rather_than_failing() {
        let (cache, dir) = cache();
        std::fs::write(dir.path().join("zh-CN.json"), b"{not json").expect("seed");

        assert!(cache.get("hello", "zh-CN", "p1").is_none());
        // And a later write repairs it.
        cache.insert("hello", "zh-CN", "p1", "你好");
        assert_eq!(cache.get("hello", "zh-CN", "p1").as_deref(), Some("你好"));
    }

    #[test]
    fn stats_count_both_tiers() {
        let (cache, _dir) = cache();
        cache.insert("a", "zh-CN", "p1", "A");
        cache.insert("b", "ja", "p1", "B");

        let stats = cache.stats();
        assert_eq!(stats.memory_entries, 2);
        assert_eq!(stats.disk_entries, 2);
        assert!(stats.disk_bytes > 0);
    }

    #[test]
    fn clearing_empties_both_tiers() {
        let (cache, _dir) = cache();
        cache.insert("a", "zh-CN", "p1", "A");
        cache.clear();

        assert!(cache.get("a", "zh-CN", "p1").is_none());
        assert_eq!(cache.stats(), TranslationCacheStats::default());
    }
}
