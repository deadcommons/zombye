//! `ZOMBYE_BLACK_WATCH=1` in a window: every frame the window's picture is read back and its mean
//! brightness (0..255) noted, and every 300 frames a line on stdout says how many frames came out
//! far darker or brighter than the rest (a black or blank frame on the GPU, which no still
//! picture catches). A tool for finding flicker; it costs a readback a frame, so it is never on
//! by itself. Offscreen, `offscreen.rs` does the same over a benchmark's frames.

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

pub fn wanted() -> bool {
    std::env::var("ZOMBYE_BLACK_WATCH").is_ok_and(|v| v != "0")
}

#[derive(Resource, Clone, Default)]
struct Seen(Arc<Mutex<Vec<(u32, f32)>>>);

pub struct BlackWatchPlugin;

impl Plugin for BlackWatchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Seen>().add_systems(Last, watch);
    }
}

/// The mean of a picture's red, green and blue (a pixel in seven is enough).
pub fn brightness(data: &[u8]) -> f32 {
    let (mut sum, mut n) = (0u64, 0u64);
    for px in data.chunks_exact(4).step_by(7) {
        sum += px[0] as u64 + px[1] as u64 + px[2] as u64;
        n += 3;
    }
    sum as f32 / n.max(1) as f32
}

/// The frames whose brightness is more than a quarter off the median, as `frame:brightness`.
pub fn odd(seen: &[(u32, f32)]) -> (f32, Vec<String>) {
    let mut sorted: Vec<f32> = seen.iter().map(|s| s.1).collect();
    sorted.sort_by(f32::total_cmp);
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
    (median, seen.iter().filter(|s| (s.1 - median).abs() > 0.25 * median.max(4.0)).map(|s| format!("{}:{:.1}", s.0, s.1)).collect())
}

fn watch(mut commands: Commands, seen: Res<Seen>, mut frame: Local<u32>) {
    *frame += 1;
    let (slot, at) = (seen.0.clone(), *frame);
    commands.spawn(Screenshot::primary_window()).observe(move |captured: On<ScreenshotCaptured>| {
        if let Some(data) = &captured.image.data {
            slot.lock().expect("black watch lock").push((at, brightness(data)));
        }
    });
    if *frame % 300 == 0 {
        let taken = std::mem::take(&mut *seen.0.lock().expect("black watch lock"));
        let (median, odd) = odd(&taken);
        println!("black watch: {} frames to {}, brightness median {median:.1}; {} odd frames{}{}", taken.len(), *frame, odd.len(), if odd.is_empty() { "" } else { ": " }, odd.join(" "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_black_frame_among_others_is_odd() {
        let mut seen: Vec<(u32, f32)> = (1..=20).map(|f| (f, 90.0 + f as f32 * 0.1)).collect();
        seen[7].1 = 0.0;
        let (median, odd) = odd(&seen);
        assert!(median > 89.0);
        assert_eq!(odd, vec!["8:0.0".to_string()]);
        assert_eq!(brightness(&[10, 20, 30, 255]), 20.0);
    }
}
