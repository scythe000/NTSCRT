//! Per-frame chain-input cache, ported from
//! `Sources/CrtCore/ChainInputCache.swift`.
//!
//! Chain inputs — the NTSC stage baked in and already downscaled — kept per
//! video frame, so a clip that has played once (or been pre-rendered while
//! paused) plays back with no decode-to-NTSC work at all: the After Effects
//! RAM-preview model. Entries are small because the chain input is the
//! *downscale* resolution (~600 KB at 320 px), so a typical 10 s clip is
//! ~150 MB.
//!
//! Owned by the UI thread, except the [`CacheProbe`] handed to the playback
//! producer, which asks from its own thread whether a frame is already held
//! and skips the NTSC step when it is.
//!
//! The store is generic over what an entry holds so the bookkeeping — budget,
//! stamps, contiguous runs — is testable without a GPU; the app instantiates
//! it at [`ChainInputCache<wgpu::Texture>`].

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex};

use ntscrt_core::DownscaleSpec;

/// What an entry was rendered with; a lookup under a different stamp is a
/// miss.
///
/// The generation covers the NTSC settings, the NTSC toggle and the downscale
/// settings (all bump it); the downscale spec rides along explicitly as a
/// belt-and-braces check, exactly as in the Swift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub generation: u64,
    pub downscale: Option<DownscaleSpec>,
}

struct Entry<T> {
    value: T,
    stamp: Stamp,
    bytes: usize,
}

/// Frame → generation, readable from the producer thread.
///
/// Only the generation is shared, not the entries: the probe answers "is this
/// frame worth skipping the NTSC stage for", which is the one question the
/// producer asks and the only one that has to cross a thread boundary.
#[derive(Clone)]
pub struct CacheProbe {
    held: Arc<Mutex<HashMap<usize, u64>>>,
}

impl CacheProbe {
    /// Any thread: is `frame` held for `generation`? Cheap enough for the
    /// producer to ask once per frame.
    pub fn is_cached(&self, frame: usize, generation: u64) -> bool {
        self.held
            .lock()
            .map(|h| h.get(&frame) == Some(&generation))
            .unwrap_or(false)
    }
}

pub struct ChainInputCache<T> {
    /// Byte budget. Beyond it, insertion is refused and the clip streams live
    /// from there — no eviction, because a loop that doesn't fit would
    /// otherwise evict exactly the frames it needs next.
    capacity: usize,
    bytes: usize,
    entries: HashMap<usize, Entry<T>>,
    held: Arc<Mutex<HashMap<usize, u64>>>,
}

impl<T> ChainInputCache<T> {
    /// A quarter of physical memory, at most 1 GiB — the Swift default.
    pub fn default_capacity() -> usize {
        const GIB: usize = 1 << 30;
        GIB.min(physical_memory_bytes() / 4)
    }

    pub fn new() -> Self {
        Self::with_capacity(Self::default_capacity())
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            bytes: 0,
            entries: HashMap::new(),
            held: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// What one entry of this size costs.
    pub fn byte_count(width: u32, height: u32) -> usize {
        width as usize * height as usize * 4
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many entries are usable under `stamp` — the pre-render loop stops
    /// when this reaches the clip's frame count.
    pub fn count_matching(&self, stamp: Stamp) -> usize {
        self.entries.values().filter(|e| e.stamp == stamp).count()
    }

    pub fn has_room(&self, byte_count: usize) -> bool {
        self.bytes + byte_count <= self.capacity
    }

    pub fn lookup(&self, frame: usize, stamp: Stamp) -> Option<&T> {
        self.entries
            .get(&frame)
            .filter(|e| e.stamp == stamp)
            .map(|e| &e.value)
    }

    /// False when the budget is exhausted and nothing was stored. Replacing a
    /// frame's existing entry is always allowed when the new one is no larger.
    pub fn insert(&mut self, frame: usize, value: T, size: (u32, u32), stamp: Stamp) -> bool {
        let n = Self::byte_count(size.0, size.1);
        let old = self.entries.get(&frame).map_or(0, |e| e.bytes);
        if self.bytes - old + n > self.capacity {
            return false;
        }
        self.entries.insert(frame, Entry { value, stamp, bytes: n });
        self.bytes = self.bytes - old + n;
        if let Ok(mut held) = self.held.lock() {
            held.insert(frame, stamp.generation);
        }
        true
    }

    pub fn invalidate_all(&mut self) {
        self.entries.clear();
        self.bytes = 0;
        if let Ok(mut held) = self.held.lock() {
            held.clear();
        }
    }

    /// A handle the producer thread can keep.
    pub fn probe(&self) -> CacheProbe {
        CacheProbe { held: Arc::clone(&self.held) }
    }

    /// Contiguous runs of cached frames under `stamp`, for the render bar
    /// under the scrubber.
    pub fn cached_ranges(&self, stamp: Stamp) -> Vec<Range<usize>> {
        let mut frames: Vec<usize> = self
            .entries
            .iter()
            .filter(|(_, e)| e.stamp == stamp)
            .map(|(f, _)| *f)
            .collect();
        frames.sort_unstable();

        let mut out: Vec<Range<usize>> = Vec::new();
        for f in frames {
            match out.last_mut() {
                Some(last) if last.end == f => last.end = f + 1,
                _ => out.push(f..f + 1),
            }
        }
        out
    }
}

impl<T> Default for ChainInputCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Installed RAM, for sizing the cache budget.
///
/// `GlobalMemoryStatusEx` is declared here rather than pulled in as a crate:
/// it is one call on a library every Windows process already links, and the
/// alternative is a dependency tree for a single number.
#[cfg(windows)]
fn physical_memory_bytes() -> usize {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    let mut status = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    // SAFETY: `status` is a correctly sized, correctly initialised
    // MEMORYSTATUSEX, which is the call's only requirement.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) } != 0;
    if ok && status.total_phys > 0 {
        status.total_phys as usize
    } else {
        FALLBACK_MEMORY
    }
}

#[cfg(not(windows))]
fn physical_memory_bytes() -> usize {
    FALLBACK_MEMORY
}

/// Assume a modest machine when the query fails, rather than either refusing
/// to cache or promising a budget that doesn't exist.
const FALLBACK_MEMORY: usize = 8 << 30;

#[cfg(test)]
mod tests {
    use super::*;
    use ntscrt_core::DownscaleMethod;

    fn stamp(generation: u64) -> Stamp {
        Stamp {
            generation,
            downscale: Some(DownscaleSpec {
                width: 320,
                height: 240,
                method: DownscaleMethod::Area,
            }),
        }
    }

    /// 320x240 chain inputs, the size the comment above quotes.
    const FRAME: (u32, u32) = (320, 240);
    const FRAME_BYTES: usize = 320 * 240 * 4;

    fn cache_for(frames: usize) -> ChainInputCache<u32> {
        ChainInputCache::with_capacity(frames * FRAME_BYTES)
    }

    #[test]
    fn a_stored_frame_comes_back_under_the_same_stamp() {
        let mut cache = cache_for(4);
        assert!(cache.insert(7, 700, FRAME, stamp(1)));
        assert_eq!(cache.lookup(7, stamp(1)), Some(&700));
        assert_eq!(cache.bytes(), FRAME_BYTES);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_different_generation_is_a_miss() {
        let mut cache = cache_for(4);
        cache.insert(7, 700, FRAME, stamp(1));
        // The entry is still held, but it was made under settings that have
        // since changed, so it must not be served.
        assert_eq!(cache.lookup(7, stamp(2)), None);
    }

    #[test]
    fn a_different_downscale_is_a_miss_even_at_the_same_generation() {
        let mut cache = cache_for(4);
        cache.insert(7, 700, FRAME, stamp(1));
        let other = Stamp {
            generation: 1,
            downscale: Some(DownscaleSpec {
                width: 256,
                height: 192,
                method: DownscaleMethod::Area,
            }),
        };
        assert_eq!(cache.lookup(7, other), None);
    }

    #[test]
    fn insertion_stops_at_the_budget_rather_than_evicting() {
        let mut cache = cache_for(3);
        for f in 0..3 {
            assert!(cache.insert(f, f as u32, FRAME, stamp(1)), "frame {f} should fit");
        }
        // A loop that doesn't fit must not evict the frames it needs next.
        assert!(!cache.insert(3, 3, FRAME, stamp(1)));
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.lookup(0, stamp(1)), Some(&0));
    }

    #[test]
    fn replacing_a_frame_does_not_double_count_it() {
        let mut cache = cache_for(2);
        cache.insert(1, 10, FRAME, stamp(1));
        cache.insert(1, 20, FRAME, stamp(2));
        assert_eq!(cache.bytes(), FRAME_BYTES);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.lookup(1, stamp(2)), Some(&20));
    }

    #[test]
    fn a_full_cache_still_accepts_a_replacement_of_the_same_size() {
        let mut cache = cache_for(2);
        cache.insert(0, 0, FRAME, stamp(1));
        cache.insert(1, 1, FRAME, stamp(1));
        assert!(!cache.has_room(FRAME_BYTES));
        assert!(cache.insert(1, 99, FRAME, stamp(1)), "replacement should fit");
    }

    #[test]
    fn invalidating_releases_the_whole_budget() {
        let mut cache = cache_for(3);
        cache.insert(0, 0, FRAME, stamp(1));
        cache.insert(1, 1, FRAME, stamp(1));
        cache.invalidate_all();
        assert_eq!(cache.bytes(), 0);
        assert!(cache.is_empty());
        assert_eq!(cache.lookup(0, stamp(1)), None);
        assert!(!cache.probe().is_cached(0, 1));
    }

    #[test]
    fn contiguous_frames_collapse_into_runs() {
        let mut cache = cache_for(16);
        for f in [0, 1, 2, 5, 6, 9] {
            cache.insert(f, f as u32, FRAME, stamp(1));
        }
        assert_eq!(cache.cached_ranges(stamp(1)), vec![0..3, 5..7, 9..10]);
    }

    #[test]
    fn runs_only_count_the_stamp_asked_for() {
        let mut cache = cache_for(16);
        cache.insert(0, 0, FRAME, stamp(1));
        cache.insert(1, 1, FRAME, stamp(2));
        cache.insert(2, 2, FRAME, stamp(1));
        // Frame 1 belongs to another generation, so the run is broken there
        // rather than being drawn as cached.
        assert_eq!(cache.cached_ranges(stamp(1)), vec![0..1, 2..3]);
        assert_eq!(cache.count_matching(stamp(1)), 2);
    }

    #[test]
    fn an_empty_cache_has_no_runs() {
        let cache = cache_for(4);
        assert!(cache.cached_ranges(stamp(1)).is_empty());
    }

    #[test]
    fn the_probe_tracks_insertions_across_threads() {
        let mut cache = cache_for(4);
        let probe = cache.probe();
        assert!(!probe.is_cached(3, 1));
        cache.insert(3, 3, FRAME, stamp(1));

        // The producer reads it from its own thread.
        let handle = std::thread::spawn(move || (probe.is_cached(3, 1), probe.is_cached(3, 2)));
        assert_eq!(handle.join().unwrap(), (true, false));
    }

    #[test]
    fn the_default_budget_is_sane() {
        let capacity = ChainInputCache::<u32>::default_capacity();
        assert!(capacity <= 1 << 30, "must not exceed 1 GiB, got {capacity}");
        // Enough for a few seconds at the quoted chain-input size, or the
        // RAM preview would never hold a usable run.
        assert!(capacity >= 64 * FRAME_BYTES, "suspiciously small: {capacity}");
    }
}
