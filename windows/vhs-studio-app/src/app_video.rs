//! Video state and playback control, ported from the video half of
//! `Sources/CrtApp/AppState.swift`.
//!
//! The shape is the macOS one. A background producer decodes and runs the
//! NTSC stage (`video::playback`); the UI thread pulls from it against a
//! wall-clock schedule, showing the frame due now, keeping later ones queued
//! and dropping missed ones. Finished chain inputs go into a per-frame cache
//! (`video::cache`), so the second time round a loop there is no CPU work per
//! frame at all — the After Effects RAM-preview model. While the clip sits
//! paused, the same producer keeps running to fill that cache in, which is
//! what puts a growing green bar under the scrubber.
//!
//! What stands in for what: macOS drives the consumer from an `MTKView`
//! display link, because `Task.sleep` on the main actor was measured waking
//! 30–55 ms late and capping playback at ~16 fps. egui's equivalent is to ask
//! for a repaint every frame while playing, which puts the consumer on the
//! compositor's clock in the same way. The macOS build also has to hold a
//! `latencyCritical` activity assertion to stop App Nap coalescing its
//! timers; Windows has no equivalent behaviour to defend against.

use std::ops::Range;
use std::time::{Duration, Instant};

use crate::app::VhsStudioApp;
use crate::image_io::SourceImage;
use crate::video::cache::{ChainInputCache, Stamp};
use crate::video::playback::{Config, PlaybackPipeline};
use crate::video::VideoSource;

/// How long after an edit the pre-render starts. Settings arrive per slider
/// tick and every one of them invalidates the cache, so it is debounced.
const PRERENDER_DEBOUNCE: Duration = Duration::from_millis(400);

/// The render bar is refreshed a few times a second rather than per frame, so
/// it doesn't cause a relayout on every tick.
const CACHED_RANGES_INTERVAL: Duration = Duration::from_millis(250);

/// Everything that exists only while a video is loaded.
pub struct VideoState {
    pub source: VideoSource,
    /// 0..`source.info.total_frames`.
    pub current_frame_index: usize,
    pub playing: bool,

    /// Bumped on any edit upstream of the shader chain — NTSC settings, the
    /// NTSC toggle, the downscale settings. Queued producer frames and cached
    /// chain inputs from an older generation are discarded.
    generation: u64,
    cache: ChainInputCache<wgpu::Texture>,
    /// Contiguous runs of cached frames, for the bar under the scrubber.
    pub cached_ranges: Vec<Range<usize>>,
    ranges_updated_at: Instant,

    playback: Option<PlaybackPipeline>,
    /// Where the consumer's schedule restarts, in producer-absolute frames.
    schedule_base: usize,
    clock_start: Option<Instant>,
    pub dropped: usize,
    pub cache_hits: usize,

    /// Chain input for the current frame when it came from the cache: NTSC
    /// and downscale both already applied.
    pub(crate) cached_chain_input: Option<wgpu::Texture>,
    /// Current frame with the NTSC stage already applied by the producer.
    /// The preview uses it as the source and skips the CPU stage; the compare
    /// split keeps using the clean pixels.
    pub(crate) processed_source: Option<Vec<u8>>,

    /// Producer that fills the cache while paused, and when to start it.
    prerender: Option<PlaybackPipeline>,
    prerender_due_at: Option<Instant>,
    pub prerender_active: bool,
}

impl VideoState {
    fn new(source: VideoSource) -> Self {
        Self {
            source,
            current_frame_index: 0,
            playing: false,
            generation: 1,
            cache: ChainInputCache::new(),
            cached_ranges: Vec::new(),
            ranges_updated_at: Instant::now(),
            playback: None,
            schedule_base: 0,
            clock_start: None,
            dropped: 0,
            cache_hits: 0,
            cached_chain_input: None,
            processed_source: None,
            prerender: None,
            prerender_due_at: None,
            prerender_active: false,
        }
    }

    pub fn total_frames(&self) -> usize {
        self.source.info.total_frames.max(1)
    }

    pub fn frame_rate(&self) -> f64 {
        self.source.info.frame_rate.max(1.0)
    }

    /// Position of the playhead in seconds.
    pub fn current_time(&self) -> f64 {
        self.current_frame_index as f64 / self.frame_rate()
    }

    /// How much of the clip is held, as a fraction — the transport bar shows
    /// it next to the render bar.
    pub fn cached_fraction(&self) -> f32 {
        let total = self.total_frames();
        self.cached_ranges.iter().map(|r| r.len()).sum::<usize>() as f32 / total as f32
    }

    fn stamp(&self, downscale: Option<vhs_studio_core::DownscaleSpec>) -> Stamp {
        Stamp { generation: self.generation, downscale }
    }
}

impl VhsStudioApp {
    /// Open a video as the source, replacing whatever was loaded.
    pub(crate) fn load_video(&mut self, path: std::path::PathBuf) {
        // ffmpeg is the one external runtime dependency in this build, so say
        // so plainly rather than letting a spawn failure surface as a decode
        // error on every file the user tries.
        if let Err(e) = crate::video::ffmpeg::probe_tools() {
            self.error = Some(e);
            return;
        }
        let video = match VideoSource::open(&path) {
            Ok(v) => v,
            Err(e) => {
                self.error = Some(format!("Could not open {}: {e}", path.display()));
                return;
            }
        };

        let first = match video.frame_at_index(0) {
            Ok(frame) => frame,
            Err(e) => {
                self.error = Some(format!("Could not decode the first frame: {e}"));
                return;
            }
        };

        let info = video.info.clone();
        self.stop_playback();
        self.video = Some(VideoState::new(video));
        self.original_source = None;
        self.source = self.rotate_decoded(first);
        self.source_path = Some(path);
        self.source_version += 1;
        self.ntsc.invalidate();
        self.status = Some(format!(
            "Loaded {}x{} video, {} frames at {:.2} fps",
            info.width, info.height, info.total_frames, info.frame_rate
        ));
        self.error = None;
        self.mark_dirty();
        self.schedule_prerender();
        self.follow_video_frame(0);
    }

    /// Something upstream of the shader chain changed — the NTSC settings or
    /// toggle, or the downscale. Every pre-baked frame is now wrong:
    /// invalidate the queued producer output and the whole cache, and start
    /// filling it again.
    pub fn mark_chain_input_edited(&mut self) {
        self.mark_dirty();
        // Every caller of this is a pipeline setting a preset owns.
        self.leave_preset();
        let Some(video) = self.video.as_mut() else { return };
        video.generation = video.generation.wrapping_add(1);
        video.processed_source = None;
        video.cached_chain_input = None;
        video.cache.invalidate_all();
        video.cached_ranges.clear();
        self.push_playback_config();
        self.schedule_prerender();
    }

    fn push_playback_config(&mut self) {
        let json = self.ntsc.settings_json().ok();
        let enabled = self.ntsc_enabled;
        let rotation = self.rotation;
        let per_frame = self.per_frame_ntsc_json();
        let Some(video) = self.video.as_ref() else { return };
        if let Some(pipeline) = &video.playback {
            pipeline.config.update(enabled, json, video.generation, rotation);
            pipeline.config.set_per_frame_json(per_frame);
        }
    }

    pub fn is_playing(&self) -> bool {
        self.video.as_ref().is_some_and(|v| v.playing)
    }

    /// Play/pause, the transport bar's button and the space bar.
    pub fn toggle_playback(&mut self) {
        if self.is_playing() {
            self.stop_playback();
            return;
        }
        let Some(video) = self.video.as_ref() else { return };
        let next = (video.current_frame_index + 1) % video.total_frames();
        if let Some(video) = self.video.as_mut() {
            video.playing = true;
            video.dropped = 0;
            video.cache_hits = 0;
        }
        // One producer at a time: playback fills the cache itself.
        self.stop_prerender();
        self.start_pipeline(next);
    }

    /// Re-decode the frame on screen so a rotation change shows at once
    /// rather than waiting for the next frame to arrive.
    pub(crate) fn refresh_rotated_video_frame(&mut self) {
        let Some(video) = self.video.as_ref() else { return };
        let frame = video.current_frame_index;
        let Ok(image) = video.source.frame_at_index(frame) else { return };
        self.source = self.rotate_decoded(image);
        if let Some(video) = self.video.as_mut() {
            video.processed_source = None;
        }
    }

    /// (Re)start the producer at `frame`.
    fn start_pipeline(&mut self, frame: usize) {
        let json = self.ntsc.settings_json().ok();
        let enabled = self.ntsc_enabled;
        let rotation = self.rotation;
        let per_frame = self.per_frame_ntsc_json();
        let Some(video) = self.video.as_mut() else { return };

        // Dropping the old pipeline stops it and joins its thread, so two
        // decoders never compete after a seek.
        video.playback = None;

        let config = Config::new(enabled, json, video.generation, rotation);
        config.set_cache_probe(Some(video.cache.probe()));
        config.set_per_frame_json(per_frame);
        match PlaybackPipeline::start(
            video.source.clone(),
            frame,
            config,
            PlaybackPipeline::DEFAULT_QUEUE_DEPTH,
        ) {
            Ok(pipeline) => {
                video.playback = Some(pipeline);
                video.schedule_base = frame;
                // Re-primed on the next tick, so the schedule cannot run
                // ahead of a producer that hasn't begun.
                video.clock_start = None;
            }
            Err(e) => {
                video.playing = false;
                self.error = Some(format!("Playback failed: {e}"));
            }
        }
    }

    pub fn stop_playback(&mut self) {
        if let Some(video) = self.video.as_mut() {
            video.playing = false;
            video.playback = None;
            video.clock_start = None;
            video.processed_source = None;
        }
        // Fill in whatever playback didn't reach.
        self.schedule_prerender();
    }

    /// Move the playhead, from the scrubber, a keyboard step or the timeline.
    pub fn seek_to_frame(&mut self, frame: usize) {
        let Some(video) = self.video.as_ref() else { return };
        let frame = frame.min(video.total_frames().saturating_sub(1));
        if frame != video.current_frame_index {
            let playing = video.playing;
            if let Some(video) = self.video.as_mut() {
                video.current_frame_index = frame;
            }
            if playing {
                // The producer is decoding somewhere else now; restart it here.
                self.start_pipeline(frame);
            } else {
                self.show_frame(frame);
            }
        }
        // Whatever moved the frame, the timeline's playhead is that frame.
        self.follow_video_frame(frame);
    }

    /// Decode and display a single frame — the paused/scrubbing path.
    fn show_frame(&mut self, frame: usize) {
        let Some(video) = self.video.as_ref() else { return };
        match video.source.frame_at_index(frame) {
            Ok(image) => {
                self.source = self.rotate_decoded(image);
                self.source_version += 1;
                self.frame_count = frame;
                if let Some(video) = self.video.as_mut() {
                    video.processed_source = None;
                    // A cache hit here skips the CPU stage even while paused,
                    // which is what makes scrubbing through pre-rendered
                    // frames feel immediate.
                    video.cached_chain_input = None;
                }
                self.refresh_cached_chain_input();
                self.error = None;
                self.mark_dirty();
            }
            Err(e) => self.error = Some(format!("Could not decode frame {frame}: {e}")),
        }
        self.schedule_prerender();
    }

    /// Look the current frame up in the cache, so a paused preview uses it.
    fn refresh_cached_chain_input(&mut self) {
        let downscale = self.downscale_spec();
        let ntsc_enabled = self.ntsc_enabled;
        let Some(video) = self.video.as_mut() else { return };
        video.cached_chain_input = ntsc_enabled
            .then(|| {
                let stamp = Stamp { generation: video.generation, downscale };
                video.cache.lookup(video.current_frame_index, stamp).cloned()
            })
            .flatten();
    }

    /// Pull the frame due now, called once per repaint while playing.
    ///
    /// The schedule is wall-clock: the frame due *now* is shown, later frames
    /// wait, missed frames drop.
    pub(crate) fn consume_playback_frame(&mut self) {
        let downscale = self.downscale_spec();
        let Some(video) = self.video.as_mut() else { return };
        if !video.playing {
            return;
        }
        let Some(pipeline) = video.playback.as_ref() else { return };

        // Prime: the clock starts when the first frame exists.
        if video.clock_start.is_none() {
            if !pipeline.has_output() {
                return;
            }
            video.clock_start = Some(Instant::now());
            video.schedule_base = pipeline.first_queued_index().unwrap_or(video.schedule_base);
        }
        let Some(clock_start) = video.clock_start else { return };

        let fps = video.frame_rate();
        let schedule = video.schedule_base + (clock_start.elapsed().as_secs_f64() * fps) as usize;
        pipeline.set_target_absolute_index(schedule);
        let (output, dropped) = pipeline.take_ready(schedule, video.generation);
        video.dropped += dropped;
        let Some(output) = output else { return };

        video.current_frame_index = output.frame_index;
        let stamp = Stamp { generation: video.generation, downscale };
        // The producer skips the NTSC stage for frames the cache holds
        // (`processed` is None then); serve those from the cache. A miss —
        // a stamp mismatch, or an insert the budget refused — falls through
        // to processing on this thread.
        video.cached_chain_input = if output.processed.is_none() {
            video.cache.lookup(output.frame_index, stamp).cloned()
        } else {
            None
        };
        if video.cached_chain_input.is_some() {
            video.cache_hits += 1;
        }
        video.processed_source = output.processed;

        let (width, height) = output.size;
        let frame_index = output.frame_index;
        self.source = SourceImage { width, height, pixels: output.clean };
        self.source_version += 1;
        self.frame_count = frame_index;
        self.mark_dirty();

        // The playhead *is* the video position: keep the timeline and the
        // sidebar in step with the frame on screen. The producer has already
        // baked this frame from the animation, so this is display only.
        self.follow_video_frame(frame_index);
    }

    /// Put the chain input the preview just rendered into the cache.
    ///
    /// Called after a playback frame is drawn, which is the only moment the
    /// finished chain input exists on the GPU and is worth keeping.
    pub(crate) fn cache_rendered_chain_input(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let downscale = self.downscale_spec();
        let ntsc_enabled = self.ntsc_enabled;

        // Decide whether there is anything to do while holding the borrow,
        // then let it go: the copy below needs the pipeline, which is a
        // sibling field.
        let (frame, stamp) = {
            let Some(video) = self.video.as_ref() else { return };
            let stamp = video.stamp(downscale);
            // Nothing to keep when the signal stage is off: without it the
            // chain input is one cheap downscale from the decoded frame.
            if !ntsc_enabled
                || video.cached_chain_input.is_some()
                || video.cache.lookup(video.current_frame_index, stamp).is_some()
            {
                return;
            }
            if !video.cache.has_room(ChainInputCache::<wgpu::Texture>::byte_count(
                chain_input_size(self.source.size(), downscale).0,
                chain_input_size(self.source.size(), downscale).1,
            )) {
                return;
            }
            (video.current_frame_index, stamp)
        };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vhs-studio.cache_fill"),
        });
        let Some((copy, size)) = self.pipeline.copy_last_chain_input(device, &mut encoder) else {
            return;
        };
        queue.submit(Some(encoder.finish()));

        let Some(video) = self.video.as_mut() else { return };
        video.cache.insert(frame, copy, size, stamp);
        Self::update_cached_ranges(video, stamp, false);
    }

    fn update_cached_ranges(video: &mut VideoState, stamp: Stamp, force: bool) {
        let now = Instant::now();
        if !force && now.duration_since(video.ranges_updated_at) < CACHED_RANGES_INTERVAL {
            return;
        }
        video.ranges_updated_at = now;
        video.cached_ranges = video.cache.cached_ranges(stamp);
    }

    // ---- pre-render while paused (the After Effects render bar) ----

    /// Start filling the cache shortly, debounced.
    fn schedule_prerender(&mut self) {
        self.stop_prerender();
        if let Some(video) = self.video.as_mut() {
            video.prerender_due_at = Some(Instant::now() + PRERENDER_DEBOUNCE);
        }
    }

    fn stop_prerender(&mut self) {
        if let Some(video) = self.video.as_mut() {
            video.prerender = None;
            video.prerender_due_at = None;
            video.prerender_active = false;
        }
    }

    /// Advance the pre-render, called once per repaint while paused.
    ///
    /// Consumed in order rather than by schedule: this wants every frame, not
    /// the one due now.
    pub(crate) fn tick_prerender(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let downscale = self.downscale_spec();
        let ntsc_enabled = self.ntsc_enabled;
        let json = self.ntsc.settings_json().ok();
        let rotation = self.rotation;
        let per_frame = self.per_frame_ntsc_json();

        let Some(video) = self.video.as_mut() else { return };
        if video.playing || !ntsc_enabled {
            return;
        }
        let stamp = Stamp { generation: video.generation, downscale };
        let total = video.total_frames();

        // Start it once the debounce has elapsed.
        if video.prerender.is_none() {
            let Some(due) = video.prerender_due_at else { return };
            if Instant::now() < due {
                return;
            }
            video.prerender_due_at = None;
            if video.cache.count_matching(stamp) >= total {
                Self::update_cached_ranges(video, stamp, true);
                return;
            }
            let config = Config::new(true, json, video.generation, rotation);
            config.set_cache_probe(Some(video.cache.probe()));
            config.set_per_frame_json(per_frame);
            match PlaybackPipeline::start(
                video.source.clone(),
                video.current_frame_index,
                config,
                PlaybackPipeline::DEFAULT_QUEUE_DEPTH,
            ) {
                Ok(pipeline) => {
                    video.prerender = Some(pipeline);
                    video.prerender_active = true;
                }
                // A pre-render that won't start is not worth interrupting the
                // user over: the clip still plays, just without the cache.
                Err(_) => return,
            }
        }

        // Take what is ready this tick. The bound keeps a repaint short — the
        // producer keeps running, so the rest arrives next time.
        const PER_TICK: usize = 4;
        for _ in 0..PER_TICK {
            // Each pass re-borrows: encoding a chain input needs the
            // pipeline, which is a sibling field of `video`.
            let taken = {
                let Some(video) = self.video.as_mut() else { return };
                let Some(pipeline) = video.prerender.as_ref() else { return };
                let Some(output) = pipeline.take_oldest(video.generation) else { return };
                let Some(processed) = output.processed else { continue };
                if video.cache.lookup(output.frame_index, stamp).is_some() {
                    continue;
                }
                let size = chain_input_size(output.size, downscale);
                if !video
                    .cache
                    .has_room(ChainInputCache::<wgpu::Texture>::byte_count(size.0, size.1))
                {
                    video.prerender = None;
                    video.prerender_active = false;
                    Self::update_cached_ranges(video, stamp, true);
                    return;
                }
                (output.frame_index, output.size, processed)
            };
            let (frame_index, source_size, processed) = taken;

            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("vhs-studio.prerender"),
            });
            let copied = self.pipeline.encode_chain_input_copy(
                device, queue, &mut encoder, &processed, source_size, downscale,
            );
            let Some((copy, size)) = copied else { continue };
            queue.submit(Some(encoder.finish()));

            let Some(video) = self.video.as_mut() else { return };
            video.cache.insert(frame_index, copy, size, stamp);
            Self::update_cached_ranges(video, stamp, false);

            if video.cache.count_matching(stamp) >= total {
                video.prerender = None;
                video.prerender_active = false;
                Self::update_cached_ranges(video, stamp, true);
                return;
            }
        }
    }
}

/// Duplicate of the arithmetic in `ScanlineGrid::chain_input_size`, kept here
/// so the budget check can run before any GPU work is encoded.
pub(crate) fn chain_input_size(
    source: (u32, u32),
    downscale: Option<vhs_studio_core::DownscaleSpec>,
) -> (u32, u32) {
    vhs_studio_core::ScanlineGrid::chain_input_size(source.0, source.1, downscale.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vhs_studio_core::{DownscaleMethod, DownscaleSpec};

    fn spec(width: u32) -> Option<DownscaleSpec> {
        Some(DownscaleSpec { width, height: width * 3 / 4, method: DownscaleMethod::Area })
    }

    #[test]
    fn the_budget_check_sizes_against_the_downscale_not_the_source() {
        // A 1080p frame costs 8 MB; its 320px chain input costs 300 KB, and
        // sizing the check against the wrong one would refuse to cache
        // anything on a long clip.
        assert_eq!(chain_input_size((1920, 1080), spec(320)), (320, 240));
        assert_eq!(chain_input_size((1920, 1080), None), (1920, 1080));
    }
}
