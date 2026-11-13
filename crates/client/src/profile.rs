//! Per-system timings for `--bench-live` (built only with `--features profile`, which turns on
//! Bevy's trace spans): a tracing layer adds up the time spent inside each system's span (and the
//! other spans Bevy opens, such as render graph nodes), and [`report`] prints the top ones per
//! frame at the end of the bench, with totals for the sim, the client's own systems and render prep.
//!
//! Systems run in parallel on several threads, so the per-system times add up to more than a
//! frame; they say where the CPU goes, not the wall time.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bevy::log::BoxedLayer;
use bevy::prelude::*;
use tracing::span::{Attributes, Id};
use tracing::{Subscriber, field::Field, field::Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

static TOTALS: Mutex<Option<HashMap<String, (Duration, u64)>>> = Mutex::new(None);

/// The layer for `LogPlugin::custom_layer`.
pub fn layer(_app: &mut App) -> Option<BoxedLayer> {
    Some(Box::new(Timer))
}

/// Forget what was timed so far (the bench starts).
pub fn reset() {
    *TOTALS.lock().unwrap() = Some(HashMap::new());
}

struct Timer;

struct Timing {
    name: String,
    entered: Option<Instant>,
}

struct NameField(Option<String>);

impl Visit for NameField {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "name" {
            self.0 = Some(value.to_string());
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "name" && self.0.is_none() {
            self.0 = Some(format!("{value:?}").trim_matches('"').to_string());
        }
    }
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Timer {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut field = NameField(None);
        attrs.record(&mut field);
        let meta = attrs.metadata().name();
        let name = match field.0 {
            Some(n) if meta == "system" => n,
            Some(n) => format!("{meta}: {n}"),
            None => meta.to_string(),
        };
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(Timing { name, entered: None });
        }
    }

    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && let Some(t) = span.extensions_mut().get_mut::<Timing>()
        {
            t.entered = Some(Instant::now());
        }
    }

    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut ext = span.extensions_mut();
        let Some(t) = ext.get_mut::<Timing>() else { return };
        let Some(start) = t.entered.take() else { return };
        let spent = start.elapsed();
        let mut totals = TOTALS.lock().unwrap();
        if let Some(map) = totals.as_mut() {
            let e = map.entry(t.name.clone()).or_default();
            e.0 += spent;
            e.1 += 1;
        }
    }
}

/// Which part of the frame a span belongs to.
fn category(name: &str) -> &'static str {
    if name.starts_with("zombye::server_link") {
        "sim tick + snapshots (server_link)"
    } else if name.starts_with("zombye::") {
        "client systems"
    } else if name.starts_with("bevy_render") || name.starts_with("bevy_pbr") || name.starts_with("bevy_core_pipeline") || name.starts_with("bevy_ui_render") || name.starts_with("bevy_sprite_render") || name.starts_with("bevy_anti_alias") || name.starts_with("bevy_post_process") || name.starts_with("bevy_light") || name.contains("extract") || name.contains("prepare") || name.contains("queue") {
        "render prep"
    } else {
        "other engine"
    }
}

/// Print the top systems by time per frame over `frames` frames.
pub fn report(frames: u32) {
    let Some(map) = TOTALS.lock().unwrap().take() else { return };
    let frames = frames.max(1) as f64;
    let mut rows: Vec<(String, f64, f64)> = map.into_iter().map(|(n, (d, c))| (n, d.as_secs_f64() * 1000.0 / frames, c as f64 / frames)).collect();
    // Spans that only wrap others (whole schedules, the frame) are left out of the totals.
    let wrapper = |n: &str| {
        n.starts_with("sub app") || n.contains("run_render_schedule") || n.contains("main_render_schedule") || n.starts_with("multithreaded executor") || n.starts_with("finish_command_buffer") || n.starts_with("schedule") || n.starts_with("frame") || n.starts_with("par_for_each") || n.ends_with("_schedule") || n.contains("wait_for_gpu") || n == "update" || n.starts_with("system_commands") || n == "main app"
    };
    // The main world's whole update, start to end (the render world works beside it).
    let main: f64 = rows.iter().filter(|(n, ..)| n == "main app").map(|r| r.1).sum();
    println!("profile: the main world's update, start to end: {main:.3} ms");
    let mut cats: HashMap<&'static str, f64> = HashMap::new();
    for (n, ms, _) in &rows {
        if !wrapper(n) {
            *cats.entry(category(n)).or_default() += ms;
        }
    }
    let gpu_wait: f64 = rows.iter().filter(|(n, ..)| n.contains("wait_for_gpu")).map(|r| r.1).sum();
    println!("profile: waiting for the GPU at the end of the frame: {gpu_wait:.3} ms");
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    println!("profile: CPU ms per frame by part (systems in parallel add up past the frame):");
    let mut cats: Vec<_> = cats.into_iter().collect();
    cats.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (c, ms) in cats {
        println!("profile:   {ms:7.3} ms  {c}");
    }
    println!("profile: top spans (ms per frame, runs per frame):");
    let top = std::env::var("ZOMBYE_PROFILE_TOP").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    for (n, ms, runs) in rows.iter().filter(|(n, ..)| !wrapper(n)).take(top) {
        println!("profile:   {ms:7.3} ms  {runs:5.1}x  {n}");
    }
}
