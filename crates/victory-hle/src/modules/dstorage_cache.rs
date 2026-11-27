//! Persistent sharded SSD read-through cache for DirectStorage (`dstorage.rs`).
//!
//! When the title's `.rpf` archives reside on a rotational HDD and a secondary
//! SSD is available (`VICTORY_DSTORAGE_CACHE_DIR`, e.g. `F:\victory-dstorage-cache`,
//! bounded by `VICTORY_DSTORAGE_CACHE_GB`, default 64 GiB), every decompressed
//! (`ZLIB` / `BCPACK`) and large raw (`>= 16 KiB`) streaming read is cached
//! **already decompressed** in sequential 512 MiB ring shards on the SSD.
//!
//! - **Cache hit**: one `seek_read` from an already-open SSD shard handle,
//!   skipping both the HDD seek (15–30 ms) and CPU Zlib/BCPack decode.
//! - **Cache miss**: normal read (throttled by [`HddGate`] so 5400 RPM HDD
//!   seeks stay bounded while SSD hits and CPU decompression scale across all
//!   worker threads) followed by a bounded asynchronous write to the SSD writer
//!   thread (never stalls DirectStorage workers).
//! - **Ring eviction**: once shards exceed the configured GiB cap, the oldest
//!   shard file is deleted in O(1) and its entries purged from the in-memory
//!   sharded index.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
use std::time::{Instant, UNIX_EPOCH};

const RECORD_MAGIC: [u8; 4] = *b"VDS1";
const INDEX_MAGIC: [u8; 8] = *b"VDSIDX01";
const RECORD_HEADER_LEN: usize = 4 + 16 + 4 + 4; // magic + key + written_len + checksum
const DEFAULT_SHARD_BYTES: u64 = 512 << 20; // 512 MiB per ring shard
const MAX_ENTRY_BYTES: usize = 64 << 20; // 64 MiB max single cached chunk
const MIN_RAW_CACHE_BYTES: usize = 16 << 10; // cache raw reads >= 16 KiB; always cache compressed
const MAX_WRITE_QUEUE_BYTES: usize = 48 << 20; // 48 MiB async write-back RAM ceiling
const INDEX_SHARDS: usize = 16;

/// Stable identity of a DirectStorage source file across runs.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FileId {
    pub(crate) hash: u64,
    pub(crate) on_ssd: bool,
}

impl FileId {
    pub(crate) fn from_open(name: &str, file: &File, on_ssd: bool) -> Self {
        let base = name
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(name)
            .to_ascii_lowercase();
        let (len, mtime) = file
            .metadata()
            .map(|m| {
                let mt = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());
                (m.len(), mt)
            })
            .unwrap_or((0, 0));
        let mut h = blake3::Hasher::new();
        h.update(b"victory-ds-file-v1\0");
        h.update(base.as_bytes());
        h.update(&len.to_le_bytes());
        h.update(&mtime.to_le_bytes());
        let out = h.finalize();
        let hash = u64::from_le_bytes(out.as_bytes()[..8].try_into().unwrap());
        Self { hash, on_ssd }
    }
}

/// 128-bit content-addressed key for one DirectStorage file read + decode.
pub(crate) fn make_key(
    file_id: u64,
    offset: u64,
    source_size: u32,
    destination_size: u32,
    compression_options: u64,
    intermediate_size: u32,
) -> [u8; 16] {
    let mut h = blake3::Hasher::new();
    h.update(b"victory-ds-chunk-v1\0");
    h.update(&file_id.to_le_bytes());
    h.update(&offset.to_le_bytes());
    h.update(&source_size.to_le_bytes());
    h.update(&destination_size.to_le_bytes());
    h.update(&compression_options.to_le_bytes());
    h.update(&intermediate_size.to_le_bytes());
    let out = h.finalize();
    out.as_bytes()[..16].try_into().unwrap()
}

/// Whether this request is a good candidate for SSD caching.
pub(crate) fn should_cache(compressed: bool, source_size: usize, destination_size: usize) -> bool {
    let payload = if compressed { destination_size } else { source_size };
    if payload == 0 || payload > MAX_ENTRY_BYTES {
        return false;
    }
    compressed || source_size >= MIN_RAW_CACHE_BYTES
}

fn payload_checksum(key: &[u8; 16], written_len: u32, data: &[u8]) -> u32 {
    let mut h = blake3::Hasher::new();
    h.update(key);
    h.update(&written_len.to_le_bytes());
    h.update(data);
    let out = h.finalize();
    u32::from_le_bytes(out.as_bytes()[..4].try_into().unwrap())
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    shard_id: u32,
    data_offset: u64,
    written_len: u32,
    checksum: u32,
}

struct WriteItem {
    key: [u8; 16],
    data: Vec<u8>,
}

struct WriteQueue {
    items: VecDeque<WriteItem>,
    bytes: usize,
    closed: bool,
}

struct WriterState {
    active_id: u32,
    active_file: Option<File>,
    active_len: u64,
    shards: BTreeMap<u32, u64>,
    since_index_save: usize,
}

/// Persistent sharded SSD ring cache.
pub(crate) struct SsdCache {
    dir: PathBuf,
    max_bytes: u64,
    shard_bytes: u64,
    index: [RwLock<HashMap<[u8; 16], Slot>>; INDEX_SHARDS],
    readers: RwLock<HashMap<u32, Arc<File>>>,
    queue: Mutex<WriteQueue>,
    has_work: Condvar,
    hits: AtomicU64,
    misses: AtomicU64,
    saved_bytes: AtomicU64,
    written_bytes: AtomicU64,
    dropped_writes: AtomicU64,
    last_log_ms: AtomicU64,
    started: Instant,
}

impl SsdCache {
    /// Opens or creates a cache in `dir` bounded to `max_bytes` using
    /// `shard_bytes` per ring shard file.
    pub(crate) fn open(dir: PathBuf, max_bytes: u64, shard_bytes: u64) -> std::io::Result<Arc<Self>> {
        std::fs::create_dir_all(&dir)?;
        let shard_bytes = shard_bytes.max(4096);
        let max_bytes = max_bytes.max(shard_bytes);
        let cache = Arc::new(Self {
            dir,
            max_bytes,
            shard_bytes,
            index: std::array::from_fn(|_| RwLock::new(HashMap::new())),
            readers: RwLock::new(HashMap::new()),
            queue: Mutex::new(WriteQueue {
                items: VecDeque::new(),
                bytes: 0,
                closed: false,
            }),
            has_work: Condvar::new(),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            saved_bytes: AtomicU64::new(0),
            written_bytes: AtomicU64::new(0),
            dropped_writes: AtomicU64::new(0),
            last_log_ms: AtomicU64::new(0),
            started: Instant::now(),
        });

        let mut shards = cache.discover_shards();
        let loaded = cache.load_index(&shards);
        if loaded == 0 && !shards.is_empty() {
            let rebuilt = cache.rebuild_index(&shards);
            tracing::info!(
                dir = %cache.dir.display(),
                shards = shards.len(),
                entries = rebuilt,
                "dstorage ssd cache: rebuilt index from shard headers"
            );
        } else {
            tracing::info!(
                dir = %cache.dir.display(),
                shards = shards.len(),
                entries = loaded,
                max_gb = format_args!("{:.1}", max_bytes as f64 / (1u64 << 30) as f64),
                "dstorage ssd cache: opened"
            );
        }

        // Open read handles for all discovered shards up front.
        {
            let mut readers = cache.readers.write().unwrap_or_else(|e| e.into_inner());
            for &id in shards.keys() {
                if let Ok(f) = File::open(cache.shard_path(id)) {
                    readers.insert(id, Arc::new(f));
                }
            }
        }

        // Resume writing into the newest shard if it still has room, else allocate next id.
        let (active_id, active_len) = match shards.iter().next_back() {
            Some((&id, &len)) if len < shard_bytes => (id, len),
            Some((&id, _)) => (id + 1, 0),
            None => (1, 0),
        };
        let active_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(cache.shard_path(active_id))
            .ok();
        if active_file.is_some() {
            shards.entry(active_id).or_insert(active_len);
            if let Ok(f) = File::open(cache.shard_path(active_id)) {
                cache
                    .readers
                    .write()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(active_id, Arc::new(f));
            }
        }

        let state = WriterState {
            active_id,
            active_file,
            active_len,
            shards,
            since_index_save: 0,
        };
        let bg = Arc::clone(&cache);
        let _ = std::thread::Builder::new()
            .name("dstorage-ssd-writer".into())
            .spawn(move || bg.writer_loop(state));

        Ok(cache)
    }

    fn shard_idx(key: &[u8; 16]) -> usize {
        (key[0] as usize) & (INDEX_SHARDS - 1)
    }

    fn shard_path(&self, id: u32) -> PathBuf {
        self.dir.join(format!("shard_{id:08}.bin"))
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("index.bin")
    }

    fn discover_shards(&self) -> BTreeMap<u32, u64> {
        let mut out = BTreeMap::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(s) = name.to_str() else { continue };
            let Some(rest) = s.strip_prefix("shard_").and_then(|r| r.strip_suffix(".bin")) else {
                continue;
            };
            let Ok(id) = rest.parse::<u32>() else { continue };
            // Query the live file metadata (not FindFirstFile's cached DirEntry
            // size, which can read as 0 on NTFS while another handle is open).
            let len = std::fs::metadata(entry.path()).map_or(0, |m| m.len());
            out.insert(id, len);
        }
        out
    }

    fn load_index(&self, shards: &BTreeMap<u32, u64>) -> usize {
        let Ok(bytes) = std::fs::read(self.index_path()) else {
            return 0;
        };
        if bytes.len() < 12 || bytes[..8] != INDEX_MAGIC {
            return 0;
        }
        const REC: usize = 16 + 4 + 8 + 4 + 4; // 36 bytes
        let count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        if bytes.len() < 12 + count * REC {
            return 0;
        }
        let mut loaded = 0usize;
        for chunk in bytes[12..12 + count * REC].chunks_exact(REC) {
            let key: [u8; 16] = chunk[0..16].try_into().unwrap();
            let shard_id = u32::from_le_bytes(chunk[16..20].try_into().unwrap());
            let data_offset = u64::from_le_bytes(chunk[20..28].try_into().unwrap());
            let written_len = u32::from_le_bytes(chunk[28..32].try_into().unwrap());
            let checksum = u32::from_le_bytes(chunk[32..36].try_into().unwrap());
            let Some(&shard_len) = shards.get(&shard_id) else {
                continue;
            };
            if data_offset.saturating_add(u64::from(written_len)) > shard_len {
                continue;
            }
            let slot = Slot {
                shard_id,
                data_offset,
                written_len,
                checksum,
            };
            self.index[Self::shard_idx(&key)]
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(key, slot);
            loaded += 1;
        }
        loaded
    }

    fn rebuild_index(&self, shards: &BTreeMap<u32, u64>) -> usize {
        let mut count = 0usize;
        let mut hdr = [0u8; RECORD_HEADER_LEN];
        for (&shard_id, &shard_len) in shards {
            let Ok(mut f) = File::open(self.shard_path(shard_id)) else {
                continue;
            };
            let mut pos = 0u64;
            while pos + RECORD_HEADER_LEN as u64 <= shard_len {
                if f.seek(SeekFrom::Start(pos)).is_err() || f.read_exact(&mut hdr).is_err() {
                    break;
                }
                if hdr[0..4] != RECORD_MAGIC {
                    break;
                }
                let key: [u8; 16] = hdr[4..20].try_into().unwrap();
                let written_len = u32::from_le_bytes(hdr[20..24].try_into().unwrap());
                let checksum = u32::from_le_bytes(hdr[24..28].try_into().unwrap());
                let data_offset = pos + RECORD_HEADER_LEN as u64;
                let end = data_offset.saturating_add(u64::from(written_len));
                if end > shard_len || written_len as usize > MAX_ENTRY_BYTES {
                    break;
                }
                let slot = Slot {
                    shard_id,
                    data_offset,
                    written_len,
                    checksum,
                };
                self.index[Self::shard_idx(&key)]
                    .write()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key, slot);
                count += 1;
                pos = end;
            }
        }
        count
    }

    fn save_index(&self) {
        let mut entries: Vec<([u8; 16], Slot)> = Vec::new();
        for map in &self.index {
            let guard = map.read().unwrap_or_else(|e| e.into_inner());
            entries.extend(guard.iter().map(|(&k, &v)| (k, v)));
        }
        let mut buf = Vec::with_capacity(12 + entries.len() * 36);
        buf.extend_from_slice(&INDEX_MAGIC);
        buf.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (key, slot) in entries {
            buf.extend_from_slice(&key);
            buf.extend_from_slice(&slot.shard_id.to_le_bytes());
            buf.extend_from_slice(&slot.data_offset.to_le_bytes());
            buf.extend_from_slice(&slot.written_len.to_le_bytes());
            buf.extend_from_slice(&slot.checksum.to_le_bytes());
        }
        let tmp = self.dir.join("index.bin.tmp");
        if std::fs::write(&tmp, &buf).is_ok() {
            let _ = std::fs::rename(&tmp, self.index_path());
        }
    }

    /// Attempts to read a cached decompressed chunk directly into `out`.
    /// Returns `Some(written_len)` on a verified hit, `None` on miss/error.
    pub(crate) fn get(&self, key: &[u8; 16], out: &mut [u8]) -> Option<usize> {
        let slot = {
            let guard = self.index[Self::shard_idx(key)]
                .read()
                .unwrap_or_else(|e| e.into_inner());
            guard.get(key).copied()
        };
        let Some(slot) = slot else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            self.maybe_log();
            return None;
        };
        let len = slot.written_len as usize;
        if len > out.len() {
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let file = {
            let readers = self.readers.read().unwrap_or_else(|e| e.into_inner());
            readers.get(&slot.shard_id).cloned()
        };
        let Some(file) = file else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        let dst = &mut out[..len];
        let mut done = 0usize;
        while done < len {
            match file.seek_read(&mut dst[done..], slot.data_offset + done as u64) {
                Ok(0) | Err(_) => {
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
                Ok(n) => done += n,
            }
        }
        if payload_checksum(key, slot.written_len, dst) != slot.checksum {
            // Corrupted entry: evict from index and fall back cleanly to HDD.
            self.index[Self::shard_idx(key)]
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .remove(key);
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        self.hits.fetch_add(1, Ordering::Relaxed);
        self.saved_bytes.fetch_add(len as u64, Ordering::Relaxed);
        self.maybe_log();
        Some(len)
    }

    /// Enqueues a newly-decoded chunk for asynchronous SSD persistence.
    /// Non-blocking: drops silently if the bounded write-back queue is full.
    pub(crate) fn put_async(&self, key: [u8; 16], data: &[u8]) {
        if data.is_empty() || data.len() > MAX_ENTRY_BYTES {
            return;
        }
        // Quick duplicate check before copying.
        if self.index[Self::shard_idx(&key)]
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&key)
        {
            return;
        }
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.closed || q.bytes.saturating_add(data.len()) > MAX_WRITE_QUEUE_BYTES {
            self.dropped_writes.fetch_add(1, Ordering::Relaxed);
            return;
        }
        q.bytes += data.len();
        q.items.push_back(WriteItem {
            key,
            data: data.to_vec(),
        });
        self.has_work.notify_one();
    }

    fn writer_loop(&self, mut st: WriterState) {
        loop {
            let item = {
                let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                while q.items.is_empty() && !q.closed {
                    let (guard, timed_out) = self
                        .has_work
                        .wait_timeout(q, std::time::Duration::from_secs(5))
                        .unwrap_or_else(|e| e.into_inner());
                    q = guard;
                    if timed_out.timed_out() && st.since_index_save > 0 {
                        drop(q);
                        if let Some(f) = &mut st.active_file {
                            let _ = f.flush();
                        }
                        self.save_index();
                        st.since_index_save = 0;
                        q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                    }
                }
                match q.items.pop_front() {
                    Some(item) => {
                        q.bytes = q.bytes.saturating_sub(item.data.len());
                        item
                    }
                    None => break,
                }
            };
            self.write_one(&mut st, item);
        }
        if st.since_index_save > 0 {
            self.save_index();
        }
    }

    fn write_one(&self, st: &mut WriterState, item: WriteItem) {
        if self.index[Self::shard_idx(&item.key)]
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&item.key)
        {
            return;
        }
        let rec_len = RECORD_HEADER_LEN as u64 + item.data.len() as u64;
        if st.active_file.is_none() || (st.active_len > 0 && st.active_len + rec_len > self.shard_bytes) {
            self.rotate_shard(st);
        }
        let Some(file) = st.active_file.as_mut() else {
            return;
        };
        let written_len = item.data.len() as u32;
        let checksum = payload_checksum(&item.key, written_len, &item.data);
        let mut hdr = [0u8; RECORD_HEADER_LEN];
        hdr[0..4].copy_from_slice(&RECORD_MAGIC);
        hdr[4..20].copy_from_slice(&item.key);
        hdr[20..24].copy_from_slice(&written_len.to_le_bytes());
        hdr[24..28].copy_from_slice(&checksum.to_le_bytes());
        let data_offset = st.active_len + RECORD_HEADER_LEN as u64;
        if file.write_all(&hdr).is_err() || file.write_all(&item.data).is_err() {
            return;
        }
        st.active_len += rec_len;
        st.shards.insert(st.active_id, st.active_len);
        self.written_bytes
            .fetch_add(item.data.len() as u64, Ordering::Relaxed);
        let slot = Slot {
            shard_id: st.active_id,
            data_offset,
            written_len,
            checksum,
        };
        self.index[Self::shard_idx(&item.key)]
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(item.key, slot);
        st.since_index_save += 1;
        if st.since_index_save >= 512 {
            self.save_index();
            st.since_index_save = 0;
        }
    }

    fn rotate_shard(&self, st: &mut WriterState) {
        if let Some(f) = st.active_file.take() {
            drop(f);
        }
        let next_id = st.shards.keys().next_back().map_or(1, |&id| id + 1);
        st.active_id = next_id;
        st.active_len = 0;
        let path = self.shard_path(next_id);
        st.active_file = OpenOptions::new().create(true).append(true).open(&path).ok();
        if st.active_file.is_some() {
            st.shards.insert(next_id, 0);
            if let Ok(r) = File::open(&path) {
                self.readers
                    .write()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(next_id, Arc::new(r));
            }
        }
        // Ring eviction: delete oldest shards while total exceeds budget.
        let mut total: u64 = st.shards.values().sum();
        while total > self.max_bytes && st.shards.len() > 1 {
            let Some((&oldest_id, &oldest_len)) = st.shards.iter().next() else {
                break;
            };
            if oldest_id == st.active_id {
                break;
            }
            st.shards.remove(&oldest_id);
            total = total.saturating_sub(oldest_len);
            for map in &self.index {
                map.write()
                    .unwrap_or_else(|e| e.into_inner())
                    .retain(|_, slot| slot.shard_id != oldest_id);
            }
            self.readers
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&oldest_id);
            let _ = std::fs::remove_file(self.shard_path(oldest_id));
        }
        self.save_index();
        st.since_index_save = 0;
    }

    #[cfg(test)]
    fn flush_sync(&self) {
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while Instant::now() < deadline {
            let empty = self.queue.lock().unwrap_or_else(|e| e.into_inner()).items.is_empty();
            if empty {
                std::thread::sleep(std::time::Duration::from_millis(25));
                if self.queue.lock().unwrap_or_else(|e| e.into_inner()).items.is_empty() {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        self.save_index();
    }

    fn maybe_log(&self) {
        let now_ms = self.started.elapsed().as_millis() as u64;
        let prev = self.last_log_ms.load(Ordering::Relaxed);
        if now_ms.saturating_sub(prev) < 15_000 {
            return;
        }
        if self
            .last_log_ms
            .compare_exchange(prev, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            return;
        }
        let entries: usize = self
            .index
            .iter()
            .map(|m| m.read().unwrap_or_else(|e| e.into_inner()).len())
            .sum();
        tracing::info!(
            hits,
            misses,
            hit_pct = format_args!("{:.1}", hits as f64 * 100.0 / total as f64),
            saved_mb = self.saved_bytes.load(Ordering::Relaxed) >> 20,
            written_mb = self.written_bytes.load(Ordering::Relaxed) >> 20,
            dropped = self.dropped_writes.load(Ordering::Relaxed),
            entries,
            "dstorage ssd cache"
        );
    }
}

/// Global SSD cache configured by `VICTORY_DSTORAGE_CACHE_DIR` and
/// `VICTORY_DSTORAGE_CACHE_GB` (default 64 GiB).
pub(crate) fn global() -> Option<&'static Arc<SsdCache>> {
    static CACHE: OnceLock<Option<Arc<SsdCache>>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let dir = std::env::var_os("VICTORY_DSTORAGE_CACHE_DIR")
                .filter(|v| !v.is_empty())?;
            let gb = std::env::var("VICTORY_DSTORAGE_CACHE_GB")
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(64)
                .clamp(1, 512);
            match SsdCache::open(PathBuf::from(&dir), gb << 30, DEFAULT_SHARD_BYTES) {
                Ok(cache) => Some(cache),
                Err(error) => {
                    tracing::warn!(
                        dir = %Path::new(&dir).display(),
                        %error,
                        "dstorage ssd cache: failed to open; continuing without SSD cache"
                    );
                    None
                }
            }
        })
        .as_ref()
}

/// Limits concurrent rotational-HDD cache-miss reads so 6+ DirectStorage
/// workers can serve SSD hits and CPU Zlib/BCPack decompression in parallel
/// without thrashing the HDD heads (`VICTORY_DSTORAGE_HDD_CONCURRENCY`,
/// default `2` when SSD cache is active, `0` = unlimited).
pub(crate) struct HddPermit(Option<&'static HddGate>);

struct HddGate {
    limit: usize,
    active: Mutex<usize>,
    cv: Condvar,
}

impl Drop for HddPermit {
    fn drop(&mut self) {
        if let Some(gate) = self.0 {
            let mut active = gate.active.lock().unwrap_or_else(|e| e.into_inner());
            *active = active.saturating_sub(1);
            gate.cv.notify_one();
        }
    }
}

pub(crate) fn acquire_hdd_permit(file_on_ssd: bool) -> HddPermit {
    if file_on_ssd {
        return HddPermit(None);
    }
    static GATE: OnceLock<Option<HddGate>> = OnceLock::new();
    let gate = GATE.get_or_init(|| {
        let limit = match std::env::var("VICTORY_DSTORAGE_HDD_CONCURRENCY") {
            Ok(v) => v.trim().parse::<usize>().unwrap_or(0),
            Err(_) => {
                // Default to 2 concurrent HDD misses when the SSD cache is enabled
                // so a larger worker pool (e.g. 6) can process SSD hits + CPU
                // decompression in parallel without thrashing a 5400 RPM HDD.
                if std::env::var_os("VICTORY_DSTORAGE_CACHE_DIR").is_some_and(|v| !v.is_empty()) {
                    2
                } else {
                    0
                }
            }
        };
        if limit == 0 {
            None
        } else {
            let limit = limit.clamp(1, 32);
            tracing::info!(limit, "dstorage: limiting concurrent HDD miss reads");
            Some(HddGate {
                limit,
                active: Mutex::new(0),
                cv: Condvar::new(),
            })
        }
    });
    let Some(gate) = gate.as_ref() else {
        return HddPermit(None);
    };
    let mut active = gate.active.lock().unwrap_or_else(|e| e.into_inner());
    while *active >= gate.limit {
        active = gate.cv.wait(active).unwrap_or_else(|e| e.into_inner());
    }
    *active += 1;
    HddPermit(Some(gate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssd_cache_roundtrip_persistence_and_ring_eviction() {
        let dir = tempfile::tempdir().unwrap();
        // Tiny 16 KiB budget with 8 KiB shards to exercise ring eviction.
        let cache = SsdCache::open(dir.path().to_path_buf(), 16 * 1024, 8 * 1024).unwrap();

        let k1 = make_key(1, 0, 1000, 4000, 1, 0);
        let d1: Vec<u8> = (0..4000).map(|i| (i * 13 + 7) as u8).collect();
        let mut out = vec![0u8; 4000];
        assert_eq!(cache.get(&k1, &mut out), None);

        cache.put_async(k1, &d1);
        cache.flush_sync();
        assert_eq!(cache.get(&k1, &mut out), Some(4000));
        assert_eq!(out, d1);

        // Reopen from disk and verify persisted index + shard hit.
        let reopened = SsdCache::open(dir.path().to_path_buf(), 16 * 1024, 8 * 1024).unwrap();
        let mut out2 = vec![0u8; 4000];
        assert_eq!(reopened.get(&k1, &mut out2), Some(4000));
        assert_eq!(out2, d1);

        // Write enough chunks to rotate past 16 KiB and evict shard 1.
        for n in 2..=8u64 {
            let kn = make_key(1, n * 4096, 2000, 5000, 1, 0);
            let dn = vec![n as u8; 5000];
            reopened.put_async(kn, &dn);
        }
        reopened.flush_sync();
        let k_last = make_key(1, 8 * 4096, 2000, 5000, 1, 0);
        let mut out_last = vec![0u8; 5000];
        assert_eq!(reopened.get(&k_last, &mut out_last), Some(5000));
        assert!(out_last.iter().all(|&b| b == 8));
        // Oldest chunk was evicted cleanly by ring rotation.
        assert_eq!(reopened.get(&k1, &mut out2), None);
    }

    #[test]
    fn ssd_cache_detects_corruption_and_rebuilds_without_index() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SsdCache::open(dir.path().to_path_buf(), 64 * 1024, 32 * 1024).unwrap();
        let k = make_key(42, 128, 512, 2048, 3, 0);
        let data: Vec<u8> = (0..2048).map(|i| (i * 31) as u8).collect();
        cache.put_async(k, &data);
        cache.flush_sync();

        // Remove index.bin and verify recovery from shard headers.
        std::fs::remove_file(dir.path().join("index.bin")).unwrap();
        let rebuilt = SsdCache::open(dir.path().to_path_buf(), 64 * 1024, 32 * 1024).unwrap();
        let mut out = vec![0u8; 2048];
        assert_eq!(rebuilt.get(&k, &mut out), Some(2048));
        assert_eq!(out, data);

        // Flip a payload byte on disk and verify checksum rejects corrupt hit.
        let shard = dir.path().join("shard_00000001.bin");
        let mut raw = std::fs::read(&shard).unwrap();
        raw[RECORD_HEADER_LEN + 10] ^= 0xFF;
        std::fs::write(&shard, &raw).unwrap();
        assert_eq!(rebuilt.get(&k, &mut out), None);
    }
}
