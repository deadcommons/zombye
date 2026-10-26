//! Offscreen rendering (`--render-to PATH`): one picture of the game, and no window, ever.
//!
//! `main.rs` builds the app without a primary window or winit (and without audio and gamepads),
//! driven by Bevy's schedule runner; the iso camera renders into an [`Image`] instead
//! ([`OffscreenTarget`], `--size WxH`). Everything else is the normal client: the world streams in
//! around the player, fog of war, cutaways, day/night lighting and the characters all come from the
//! player this client is (`--player`, on a `--load`ed save), exactly as in the windowed game. The
//! UI isn't drawn (Bevy only puts it on a camera that renders to a window) unless `--hud` makes this
//! camera the UI's too (`camera.rs`), for pictures of the HUD and its panels.
//!
//! The local server never ticks and the player isn't predicted (see [`crate::live`]), so the
//! picture shows the loaded state itself. Visual time runs at a fixed 1/30 s a frame for
//! [`WARM_FRAMES`] (so eased things settle: a sneaking survivor crouches, a zombie lying down has
//! fallen), then stops. After that every frame is the same picture once everything is ready: the
//! render world reports how many pipelines are still waiting (shaders loading or compiling), and
//! once none have been for a few frames the image is read back. Two readbacks in a row that match
//! (all but a few bytes, see [`settled`]) are written as the PNG (or the last one, with a warning, if it never settles), then the
//! app exits.
//!
//! `ZOMBYE_FRAME_BENCH=N` times the frame cost: after the PNG it renders N more frames of the same
//! picture, waiting for the GPU to finish each one, and prints their mean and worst in ms.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::render::render_resource::{PipelineCache, PollType, TextureFormat, TextureUsages};
use bevy::render::renderer::RenderDevice;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::time::TimeUpdateStrategy;

use crate::Options;

/// Visual time per frame while warming up, and how many frames that lasts (1 s).
const FRAME_SECS: f64 = 1.0 / 30.0;
const WARM_FRAMES: u32 = 30;
/// Frames in a row with no pipeline waiting before reading the image back.
const SETTLE_FRAMES: u32 = 3;
/// Readbacks before giving up on two matching ones (the last is written, with a warning).
const MAX_CAPTURES: u32 = 8;
/// See [`settled`].
const SAME_FRACTION: usize = 10_000;
/// Wall-clock limit for the whole render.
const DEADLINE: Duration = Duration::from_secs(40);

/// The image the camera renders into.
#[derive(Resource)]
pub struct OffscreenTarget(pub Handle<Image>);

/// Pipelines the render world is still waiting for (shaders loading or compiling), counted at the
/// end of each render frame.
#[derive(Resource, Clone, Default)]
struct PendingPipelines(Arc<AtomicUsize>);

/// Set while benchmarking: the render world waits for the GPU at the end of every frame, so a
/// frame's wall time includes its GPU work.
#[derive(Resource, Clone, Default)]
struct SyncGpu(Arc<AtomicBool>);

/// The latest readback, from the screenshot observer.
#[derive(Resource, Clone, Default)]
struct Readback(Arc<Mutex<Option<Image>>>);

#[derive(PartialEq, Debug)]
enum Phase {
    Warm,
    Settle,
    Capture,
    Bench,
}

#[derive(Resource)]
struct Job {
    path: PathBuf,
    started: Instant,
    frame: u32,
    phase: Phase,
    settled: u32,
    captures: u32,
    previous: Option<Image>,
    /// Frames to time after the PNG (`ZOMBYE_FRAME_BENCH`), and the frame times so far.
    bench: u32,
    times: Vec<f32>,
    last: Instant,
}

pub struct OffscreenPlugin;

impl Plugin for OffscreenPlugin {
    fn build(&self, app: &mut App) {
        let options = app.world().resource::<Options>().clone();
        let path = options.render_to.clone().expect("the offscreen plugin is only added with --render-to");
        let size = options.size;
        let mut image = Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None);
        image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
        let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        let pending = PendingPipelines::default();
        let sync = SyncGpu::default();
        let bench = std::env::var("ZOMBYE_FRAME_BENCH").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        let now = Instant::now();
        app.insert_resource(OffscreenTarget(handle))
            .insert_resource(pending.clone())
            .insert_resource(sync.clone())
            .init_resource::<Readback>()
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(FRAME_SECS)))
            .insert_resource(Job { path, started: now, frame: 0, phase: Phase::Warm, settled: 0, captures: 0, previous: None, bench, times: Vec::new(), last: now })
            .add_systems(Last, drive);
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.insert_resource(pending).insert_resource(sync).add_systems(Render, (count_pending, wait_for_gpu).in_set(RenderSystems::Cleanup));
        }
    }
}

fn count_pending(cache: Res<PipelineCache>, pending: Res<PendingPipelines>) {
    pending.0.store(cache.waiting_pipelines().count(), Ordering::Relaxed);
}

fn wait_for_gpu(sync: Res<SyncGpu>, device: Res<RenderDevice>) {
    if sync.0.load(Ordering::Relaxed) {
        let _ = device.poll(PollType::wait_indefinitely());
    }
}

#[allow(clippy::too_many_arguments)]
fn drive(
    mut commands: Commands,
    mut job: ResMut<Job>,
    target: Res<OffscreenTarget>,
    pending: Res<PendingPipelines>,
    readback: Res<Readback>,
    sync: Res<SyncGpu>,
    options: Res<Options>,
    mut time: ResMut<Time<Virtual>>,
    mut exit: MessageWriter<AppExit>,
) {
    job.frame += 1;
    if job.phase == Phase::Bench {
        let now = Instant::now();
        let ms = (now - job.last).as_secs_f32() * 1000.0;
        job.last = now;
        // The first frame or two still overlap the readback.
        job.times.push(ms);
        if job.times.len() as u32 >= job.bench + 2 {
            let t = &job.times[2..];
            let mean = t.iter().sum::<f32>() / t.len() as f32;
            let worst = t.iter().copied().fold(0.0, f32::max);
            println!("frame cost: {mean:.2} ms mean, {worst:.2} ms worst over {} frames", t.len());
            exit.write(AppExit::Success);
        }
        return;
    }
    if job.started.elapsed() > DEADLINE {
        let ms = job.started.elapsed().as_millis();
        match job.previous.take() {
            Some(image) => {
                let ok = finish(&job, image, &options, &format!("; warning: still changing after {ms} ms"));
                exit.write(if ok { AppExit::Success } else { AppExit::error() });
            }
            None => {
                eprintln!("render failed: nothing to read back after {ms} ms ({} frames, {} pipelines waiting)", job.frame, pending.0.load(Ordering::Relaxed));
                exit.write(AppExit::error());
            }
        }
        return;
    }
    match job.phase {
        Phase::Warm => {
            if job.frame >= WARM_FRAMES {
                time.pause();
                job.phase = Phase::Settle;
            }
        }
        Phase::Settle => {
            job.settled = if pending.0.load(Ordering::Relaxed) == 0 { job.settled + 1 } else { 0 };
            if job.settled >= SETTLE_FRAMES {
                let slot = readback.0.clone();
                commands.spawn(Screenshot::image(target.0.clone())).observe(move |captured: On<ScreenshotCaptured>| {
                    *slot.lock().expect("readback lock") = Some(captured.image.clone());
                });
                job.phase = Phase::Capture;
            }
        }
        Phase::Capture => {
            let Some(image) = readback.0.lock().expect("readback lock").take() else { return };
            job.captures += 1;
            let same = job.previous.as_ref().is_some_and(|p| settled(p, &image));
            let note = if same {
                Some(String::new())
            } else if job.captures >= MAX_CAPTURES {
                Some(format!("; warning: no two of {} readbacks matched", job.captures))
            } else {
                None
            };
            match note {
                Some(note) => {
                    let ok = finish(&job, image, &options, &note);
                    if ok && job.bench > 0 {
                        sync.0.store(true, Ordering::Relaxed);
                        job.last = Instant::now();
                        job.phase = Phase::Bench;
                    } else {
                        exit.write(if ok { AppExit::Success } else { AppExit::error() });
                    }
                }
                None => {
                    // Read back again next frame (sooner if nothing is waiting).
                    job.previous = Some(image);
                    job.settled = SETTLE_FRAMES - 1;
                    job.phase = Phase::Settle;
                }
            }
        }
        Phase::Bench => {}
    }
}

/// Two readbacks show the same picture: at most one byte in [`SAME_FRACTION`] differs (eased
/// things creep on by a hair each frame even with time stopped; something still loading changes
/// thousands of pixels).
fn settled(a: &Image, b: &Image) -> bool {
    match (&a.data, &b.data) {
        (Some(a), Some(b)) if a.len() == b.len() => a.iter().zip(b).filter(|(x, y)| x != y).count() <= a.len() / SAME_FRACTION,
        _ => false,
    }
}

/// Write the PNG and say so on stdout (or why not on stderr); true if it was written.
fn finish(job: &Job, image: Image, options: &Options, note: &str) -> bool {
    match write_png(image, &job.path) {
        Ok(()) => {
            let s = options.size;
            println!(
                "rendered {}: {}x{}, {} frames, {} readbacks, {} ms{note}",
                job.path.display(),
                s.x,
                s.y,
                job.frame,
                job.captures,
                job.started.elapsed().as_millis()
            );
            if !note.is_empty() {
                eprintln!("{}", note.trim_start_matches("; "));
            }
            true
        }
        Err(e) => {
            eprintln!("render failed: {e}");
            false
        }
    }
}

fn write_png(image: Image, path: &Path) -> Result<(), String> {
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) {
        return Err(format!("{}: --render-to writes a .png", path.display()));
    }
    let rgb = image.try_into_dynamic().map_err(|e| format!("can't read the frame back: {e}"))?.to_rgb8();
    rgb.save(path).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}
