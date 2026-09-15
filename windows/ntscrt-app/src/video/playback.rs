//! Background producer for video playback, ported from
//! `Sources/CrtCore/PlaybackPipeline.swift`.
//!
//! Decodes sequentially and runs the NTSC stage on the CPU, feeding finished
//! frames to the UI thread through a small bounded queue. This is the
//! standard player/NLE pipeline shape, and the Swift original records the
//! measurements behind it:
//!
//! - Serial playback paid for every stage in sequence — decode + NTSC + a GPU
//!   round trip, ~45 ms a frame at 2 MP (21 fps against a 24 fps clip).
//!   Pipelined, throughput is the slowest stage rather than the sum.
//! - One NTSC pass at 2 MP costs ~35 ms, too close to a 41.7 ms frame budget
//!   to ever build a cushion. Two filter instances process alternating frames
//!   concurrently, which is sound because ntsc-rs is deterministic per
//!   (settings, frame index) and frames are therefore independent.
//!
//! Two things differ from the Swift, both because of what is underneath:
//!
//! - The macOS producer decodes into `MTLBuffer`-backed textures so ntsc-rs
//!   can run in place inside texture memory with no upload step. Here frames
//!   arrive from a pipe as plain CPU buffers and the NTSC stage runs on them
//!   directly, which is the same one-copy shape without needing the pool.
//! The per-frame settings hook (`Config::set_per_frame_json`) is how a
//! keyframe timeline plays: the producer asks it for the NTSC settings of
//! each frame index instead of using the base settings, so the baked frame —
//! and therefore the cached one — is what the animation says that frame
//! should be. Settings are a pure function of (keys, frame index), which is
//! what keeps the frame cache valid while the playhead moves.
//!
//! The frame index handed to the signal stage is the clip's own frame number,
//! the same number the export paths use, so a frame scrubbed to in the
//! preview and that frame in an exported file get identical noise.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use ntscrt_core::{NtscStage, PixelFormat, Rotation};

use super::cache::CacheProbe;
use super::source::VideoSource;

/// One finished frame.
///
/// `clean` is the decoded image, which feeds the compare split; `processed`
/// has the NTSC stage baked in and is None when the stage is off, when it
/// failed, or when the frame cache already holds this frame (the producer
/// still decodes it — h264 needs the sequence — but skips the expensive part).
pub struct Output {
    pub clean: Vec<u8>,
    pub processed: Option<Vec<u8>>,
    pub size: (u32, u32),
    /// Frame number within the clip.
    pub frame_index: usize,
    /// Monotonic across loop wraps, for real-time scheduling.
    pub absolute_index: usize,
    pub generation: u64,
}

/// NTSC settings for one frame index, evaluated from a keyframe timeline.
/// None means "use the base settings".
pub type PerFrameJson = Arc<dyn Fn(usize) -> Option<String> + Send + Sync>;

/// NTSC settings snapshot, updated from the UI thread when the user edits
/// values. Bumping the generation invalidates queued frames.
pub struct Config {
    inner: Mutex<ConfigInner>,
}

struct ConfigInner {
    enabled: bool,
    settings_json: Option<String>,
    generation: u64,
    /// Applied to each frame before the signal stage, so a rotated clip is
    /// degraded and scanned as if it had been shot that way.
    rotation: Rotation,
    /// Frame-cache probe: a frame already held is decoded but not processed.
    probe: Option<CacheProbe>,
    /// Keyframed settings per frame; overrides `settings_json` when set.
    per_frame: Option<PerFrameJson>,
}

impl Config {
    pub fn new(
        enabled: bool,
        settings_json: Option<String>,
        generation: u64,
        rotation: Rotation,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(ConfigInner {
                enabled,
                settings_json,
                generation,
                rotation,
                probe: None,
                per_frame: None,
            }),
        })
    }

    pub fn set_per_frame_json(&self, per_frame: Option<PerFrameJson>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.per_frame = per_frame;
        }
    }

    fn per_frame_json(&self) -> Option<PerFrameJson> {
        self.inner.lock().ok().and_then(|inner| inner.per_frame.clone())
    }

    pub fn update(
        &self,
        enabled: bool,
        settings_json: Option<String>,
        generation: u64,
        rotation: Rotation,
    ) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.enabled = enabled;
            inner.rotation = rotation;
            inner.settings_json = settings_json;
            inner.generation = generation;
        }
    }

    pub fn set_cache_probe(&self, probe: Option<CacheProbe>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.probe = probe;
        }
    }

    fn snapshot(&self) -> (bool, Option<String>, u64, Rotation, Option<CacheProbe>) {
        match self.inner.lock() {
            Ok(inner) => (
                inner.enabled,
                inner.settings_json.clone(),
                inner.generation,
                inner.rotation,
                inner.probe.clone(),
            ),
            // A poisoned config means a producer panicked mid-frame; playing
            // on with the signal stage off beats taking the app down.
            Err(_) => (false, None, 0, Rotation::None, None),
        }
    }
}

/// How the consumer's takes are going, for `CRT_PERF_LOG`.
#[derive(Default, Clone, Copy)]
struct TakeStats {
    stale_generation: usize,
    superseded: usize,
    empty_takes: usize,
    future_only: usize,
    takes: usize,
}

struct QueueState {
    buffer: Vec<Output>,
    closed: bool,
    stats: TakeStats,
}

struct BoundedQueue {
    state: Mutex<QueueState>,
    signal: Condvar,
    capacity: usize,
}

impl BoundedQueue {
    fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(QueueState {
                buffer: Vec::new(),
                closed: false,
                stats: TakeStats::default(),
            }),
            signal: Condvar::new(),
            capacity,
        }
    }

    /// Blocks while full — this is the producer's backpressure. False once
    /// the queue is closed, which is the producer's signal to exit.
    fn push(&self, output: Output) -> bool {
        let Ok(mut state) = self.state.lock() else { return false };
        while state.buffer.len() >= self.capacity && !state.closed {
            let Ok(next) = self.signal.wait(state) else { return false };
            state = next;
        }
        if state.closed {
            return false;
        }
        state.buffer.push(output);
        self.signal.notify_all();
        true
    }

    /// The frame due at `schedule` (or the newest one before it), leaving
    /// frames still in the future queued for later ticks — taking those early
    /// would eat the producer's whole ahead-buffer as "drops" and collapse
    /// the pipeline's cushion. Superseded and stale-generation frames are
    /// discarded and counted. Non-blocking; None means nothing is due yet.
    fn take_ready(&self, schedule: usize, generation: u64) -> (Option<Output>, usize) {
        let Ok(mut state) = self.state.lock() else { return (None, 0) };
        if state.buffer.is_empty() {
            state.stats.empty_takes += 1;
            return (None, 0);
        }

        let queued = std::mem::take(&mut state.buffer);
        let before = queued.len();
        // Frames from before a settings change are thrown away outright.
        let fresh: Vec<Output> = queued.into_iter().filter(|o| o.generation == generation).collect();
        let mut dropped = before - fresh.len();
        state.stats.stale_generation += dropped;

        let (mut due, future): (Vec<Output>, Vec<Output>) =
            fresh.into_iter().partition(|o| o.absolute_index <= schedule);
        state.buffer = future;
        self.signal.notify_all();

        let Some(take) = due.pop() else {
            state.stats.future_only += 1;
            return (None, dropped);
        };
        // Whatever else was due has been superseded by the one we took.
        dropped += due.len();
        state.stats.superseded += due.len();
        state.stats.takes += 1;
        (Some(take), dropped)
    }

    /// The oldest fresh frame, in order and regardless of schedule — for
    /// pre-rendering, which wants every frame rather than the one due.
    fn take_oldest(&self, generation: u64) -> Option<Output> {
        let mut state = self.state.lock().ok()?;
        while state.buffer.first().is_some_and(|o| o.generation != generation) {
            state.buffer.remove(0);
            state.stats.stale_generation += 1;
        }
        if state.buffer.is_empty() {
            return None;
        }
        let out = state.buffer.remove(0);
        self.signal.notify_all();
        Some(out)
    }

    /// True once at least one frame is available — used to prime the
    /// consumer's clock, so the schedule doesn't start running before
    /// anything exists to show.
    fn has_output(&self) -> bool {
        self.state.lock().map(|s| !s.buffer.is_empty()).unwrap_or(false)
    }

    fn peek_first_absolute_index(&self) -> Option<usize> {
        self.state.lock().ok()?.buffer.first().map(|o| o.absolute_index)
    }

    fn stats_line(&self) -> String {
        let Ok(mut state) = self.state.lock() else { return String::new() };
        let s = std::mem::take(&mut state.stats);
        format!(
            "takes {} emptyQ {} futureOnly {} staleGen {} superseded {}",
            s.takes, s.empty_takes, s.future_only, s.stale_generation, s.superseded
        )
    }

    fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            state.buffer.clear();
        }
        self.signal.notify_all();
    }
}

pub struct PlaybackPipeline {
    pub config: Arc<Config>,
    queue: Arc<BoundedQueue>,
    stop: Arc<AtomicBool>,
    /// The consumer's current schedule position; frames behind it skip the
    /// expensive NTSC step so the producer catches up at decode cost.
    target_absolute: Arc<AtomicUsize>,
    thread: Option<JoinHandle<()>>,
}

impl PlaybackPipeline {
    pub const DEFAULT_QUEUE_DEPTH: usize = 3;

    /// Open the clip at `start_frame` and start producing.
    ///
    /// The first reader is created here rather than on the thread, so a file
    /// that cannot be decoded reports that to the caller instead of leaving a
    /// producer that silently never emits.
    pub fn start(
        source: VideoSource,
        start_frame: usize,
        config: Arc<Config>,
        queue_depth: usize,
    ) -> Result<Self, String> {
        let reader = source.sequential_reader(start_frame)?;

        let queue = Arc::new(BoundedQueue::new(queue_depth.max(1)));
        let stop = Arc::new(AtomicBool::new(false));
        let target_absolute = Arc::new(AtomicUsize::new(0));

        let thread = {
            let (queue, stop, target, config) = (
                Arc::clone(&queue),
                Arc::clone(&stop),
                Arc::clone(&target_absolute),
                Arc::clone(&config),
            );
            std::thread::Builder::new()
                .name("ntscrt.playback.producer".into())
                .spawn(move || {
                    run(source, reader, start_frame, queue, stop, target, config);
                })
                .map_err(|e| format!("could not start the playback producer: {e}"))?
        };

        Ok(Self {
            config,
            queue,
            stop,
            target_absolute,
            thread: Some(thread),
        })
    }

    pub fn set_target_absolute_index(&self, index: usize) {
        self.target_absolute.store(index, Ordering::Relaxed);
    }

    pub fn take_ready(&self, schedule: usize, generation: u64) -> (Option<Output>, usize) {
        self.queue.take_ready(schedule, generation)
    }

    pub fn take_oldest(&self, generation: u64) -> Option<Output> {
        self.queue.take_oldest(generation)
    }

    pub fn has_output(&self) -> bool {
        self.queue.has_output()
    }

    pub fn first_queued_index(&self) -> Option<usize> {
        self.queue.peek_first_absolute_index()
    }

    pub fn take_stats_line(&self) -> String {
        self.queue.stats_line()
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.queue.close();
    }
}

impl Drop for PlaybackPipeline {
    fn drop(&mut self) {
        self.stop();
        // Join rather than detach: the producer owns an ffmpeg, and a
        // seek-while-playing drops one pipeline and builds another
        // immediately. Letting the old decoder outlive the swap would leave
        // two of them competing for the same cores.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What a decoded frame is waiting to have done to it.
struct Pending {
    clean: Vec<u8>,
    /// Size of `clean`, which is the rotated size — not the clip's.
    size: (u32, u32),
    frame_index: usize,
    absolute: usize,
    generation: u64,
    settings_json: Option<String>,
    wants_ntsc: bool,
}

fn run(
    source: VideoSource,
    first_reader: super::source::SequentialReader,
    start_frame: usize,
    queue: Arc<BoundedQueue>,
    stop: Arc<AtomicBool>,
    target_absolute: Arc<AtomicUsize>,
    config: Arc<Config>,
) {
    // Two filter instances process alternating frames concurrently; each is
    // used by exactly one lane of a batch at a time.
    let mut filters = [NtscStage::new(), NtscStage::new()];
    let mut applied_json: [Option<String>; 2] = [None, None];

    let mut reader = Some(first_reader);
    let mut frame_index = start_frame;
    let mut absolute = start_frame;
    let total = source.info.total_frames.max(1);
    let (width, height) = (source.info.width, source.info.height);

    while !stop.load(Ordering::Relaxed) {
        // Decode up to two frames for one parallel batch.
        let mut batch: Vec<Pending> = Vec::with_capacity(2);
        while batch.len() < 2 && !stop.load(Ordering::Relaxed) {
            let Some(active) = reader.as_mut() else { return };
            let Some(pixels) = active.next_frame() else {
                // End of the clip: loop from the top with a fresh decoder.
                reader = source.sequential_reader(0).ok();
                frame_index = 0;
                if reader.is_none() {
                    return;
                }
                continue;
            };

            // Frames already behind the consumer's schedule get dropped on
            // arrival — decode them (h264 needs the sequence) but skip the
            // NTSC cost, so catch-up runs at decode speed.
            let hopeless = target_absolute.load(Ordering::Relaxed) > absolute;
            if !hopeless {
                let (enabled, json, generation, rotation, probe) = config.snapshot();
                // A keyframed clip has its own settings for every frame.
                let json = config
                    .per_frame_json()
                    .and_then(|f| f(frame_index))
                    .or(json);
                let cached = probe.is_some_and(|p| p.is_cached(frame_index, generation));
                // Rotate before anything else touches the frame: NTSC is a
                // scanline effect, so rotating afterwards would carry the
                // scanlines round with the picture.
                let (clean, pw, ph) = if rotation == Rotation::None {
                    (pixels.to_vec(), width, height)
                } else {
                    ntscrt_core::rotate_rgba(pixels, width, height, rotation)
                };
                batch.push(Pending {
                    clean,
                    size: (pw, ph),
                    frame_index,
                    absolute,
                    generation,
                    settings_json: json,
                    wants_ntsc: enabled && !cached,
                });
            }
            frame_index = (frame_index + 1) % total;
            absolute += 1;

            // Don't wait around assembling a pair while the consumer is
            // starved — ship a single immediately at startup and after seeks.
            if batch.len() == 1 && !queue.has_output() {
                break;
            }
        }
        if batch.is_empty() {
            continue;
        }

        // Process concurrently, one filter lane per frame. Each lane owns its
        // filter, its record of what that filter is configured with, and its
        // output slot, so the lanes share nothing and need no locking.
        let mut processed: [Option<Vec<u8>>; 2] = [None, None];
        std::thread::scope(|scope| {
            let lanes: Vec<_> = batch
                .iter()
                .zip(filters.iter_mut())
                .zip(applied_json.iter_mut())
                .zip(processed.iter_mut())
                .map(|(((pending, filter), applied), slot)| {
                    scope.spawn(move || {
                        if !pending.wants_ntsc {
                            return;
                        }
                        // Re-parse the preset only when it actually changed;
                        // it is the same JSON on every frame of a playthrough.
                        if pending.settings_json.is_some() && *applied != pending.settings_json {
                            let json = pending.settings_json.as_deref().unwrap_or_default();
                            if filter.set_settings_json(json).is_err() {
                                return;
                            }
                            applied.clone_from(&pending.settings_json);
                        }
                        let mut buffer = pending.clean.clone();
                        let (pw, ph) = pending.size;
                        let processed = filter.process(
                            &mut buffer,
                            PixelFormat::Rgba8,
                            pw,
                            ph,
                            pw * 4,
                            pending.frame_index as i64,
                            // Every frame is new pixels, so there is no clean
                            // copy worth reusing between them.
                            None,
                        );
                        if processed.is_ok() {
                            *slot = Some(buffer);
                        }
                    })
                })
                .collect();
            for lane in lanes {
                let _ = lane.join();
            }
        });

        // Emit in order.
        for (pending, processed) in batch.into_iter().zip(processed) {
            let output = Output {
                // The rotated size, since `clean` is the rotated frame.
                size: pending.size,
                clean: pending.clean,
                processed,
                frame_index: pending.frame_index,
                absolute_index: pending.absolute,
                generation: pending.generation,
            };
            if !queue.push(output) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(absolute: usize, generation: u64) -> Output {
        Output {
            clean: Vec::new(),
            processed: None,
            size: (4, 4),
            frame_index: absolute,
            absolute_index: absolute,
            generation,
        }
    }

    #[test]
    fn nothing_queued_yields_nothing() {
        let queue = BoundedQueue::new(4);
        let (out, dropped) = queue.take_ready(10, 1);
        assert!(out.is_none());
        assert_eq!(dropped, 0);
    }

    #[test]
    fn the_newest_due_frame_wins_and_older_ones_count_as_drops() {
        let queue = BoundedQueue::new(8);
        for i in 0..4 {
            queue.push(output(i, 1));
        }
        // Frames 0..=2 are due; 2 is shown and 0 and 1 are superseded.
        let (out, dropped) = queue.take_ready(2, 1);
        assert_eq!(out.map(|o| o.absolute_index), Some(2));
        assert_eq!(dropped, 2);
    }

    #[test]
    fn frames_in_the_future_stay_queued() {
        let queue = BoundedQueue::new(8);
        queue.push(output(5, 1));
        queue.push(output(6, 1));

        // Nothing is due yet, and taking them early would eat the producer's
        // whole cushion as drops.
        let (out, dropped) = queue.take_ready(3, 1);
        assert!(out.is_none());
        assert_eq!(dropped, 0);
        assert!(queue.has_output());
        assert_eq!(queue.peek_first_absolute_index(), Some(5));

        // Once the schedule reaches them they come out in order.
        assert_eq!(queue.take_ready(5, 1).0.map(|o| o.absolute_index), Some(5));
        assert_eq!(queue.take_ready(6, 1).0.map(|o| o.absolute_index), Some(6));
    }

    #[test]
    fn a_settings_change_discards_everything_queued_before_it() {
        let queue = BoundedQueue::new(8);
        for i in 0..3 {
            queue.push(output(i, 1));
        }
        let (out, dropped) = queue.take_ready(10, 2);
        assert!(out.is_none(), "frames from the old generation must not be shown");
        assert_eq!(dropped, 3);
        assert!(!queue.has_output());
    }

    #[test]
    fn taking_oldest_walks_past_stale_frames_in_order() {
        let queue = BoundedQueue::new(8);
        queue.push(output(0, 1));
        queue.push(output(1, 2));
        queue.push(output(2, 2));

        // Pre-rendering wants every frame of the current generation, in
        // order, regardless of any schedule.
        assert_eq!(queue.take_oldest(2).map(|o| o.absolute_index), Some(1));
        assert_eq!(queue.take_oldest(2).map(|o| o.absolute_index), Some(2));
        assert!(queue.take_oldest(2).is_none());
    }

    #[test]
    fn closing_releases_a_producer_blocked_on_a_full_queue() {
        let queue = Arc::new(BoundedQueue::new(1));
        assert!(queue.push(output(0, 1)));

        let producer = {
            let queue = Arc::clone(&queue);
            std::thread::spawn(move || queue.push(output(1, 1)))
        };
        // The queue is full, so the push above is parked; closing must wake
        // it and tell it to give up rather than leaving the thread hung.
        queue.close();
        assert!(!producer.join().unwrap());
    }

    #[test]
    fn stats_report_and_reset() {
        let queue = BoundedQueue::new(4);
        queue.take_ready(0, 1);
        let line = queue.stats_line();
        assert!(line.contains("emptyQ 1"), "got {line}");
        assert!(queue.stats_line().contains("emptyQ 0"), "stats should reset when read");
    }

    #[test]
    fn config_snapshots_what_was_last_pushed() {
        let config = Config::new(true, Some("{}".into()), 1, Rotation::None);
        let (enabled, json, generation, rotation, probe) = config.snapshot();
        assert!(enabled && json.as_deref() == Some("{}") && generation == 1 && probe.is_none());
        assert_eq!(rotation, Rotation::None);

        config.update(false, None, 7, Rotation::Cw90);
        let (enabled, json, generation, rotation, _) = config.snapshot();
        assert!(!enabled && json.is_none() && generation == 7);
        // Rotation rides with the rest so the producer picks a turn up on
        // the very next frame it decodes.
        assert_eq!(rotation, Rotation::Cw90);
    }
}
