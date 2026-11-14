//! The performance monitor (Settings, Game: "Performance monitor"; `--perf-monitor off|fps|full`
//! for a session): a small read-out in the top right corner of the game, for players and
//! playtesters as well as developers, so it is shown in a packaged game too. `Fps` is the frame
//! rate; `Full` adds the frame time over the last second (mean and worst), the slowest 1% of frames
//! as a frame rate, the graphics preset and the size the 3D world is drawn at (`render_scale.rs`). Measured on the CPU, from the time
//! between frames (`Time<Real>`), so it is what the player sees, vsync's waits included.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::theme;
use crate::ui_scale::SettingsStore;

/// How much the monitor shows.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum PerfMonitor {
    #[default]
    Off,
    Fps,
    Full,
}

impl PerfMonitor {
    pub const ALL: [PerfMonitor; 3] = [PerfMonitor::Off, PerfMonitor::Fps, PerfMonitor::Full];

    pub fn name(self) -> &'static str {
        match self {
            PerfMonitor::Off => "Off",
            PerfMonitor::Fps => "FPS",
            PerfMonitor::Full => "Full",
        }
    }

    pub fn parse(text: &str) -> Option<PerfMonitor> {
        PerfMonitor::ALL.into_iter().find(|m| m.name().eq_ignore_ascii_case(text.trim()))
    }
}

/// `--perf-monitor` for this session, over the settings file (not saved).
#[derive(Resource, Clone, Copy)]
pub struct PerfOverride(pub PerfMonitor);

pub struct PerfMonitorPlugin;

impl Plugin for PerfMonitorPlugin {
    fn build(&self, app: &mut App) {
        let args: Vec<String> = std::env::args().collect();
        if let Some(m) = args.iter().position(|a| a == "--perf-monitor").and_then(|i| args.get(i + 1)).and_then(|v| PerfMonitor::parse(v)) {
            app.insert_resource(PerfOverride(m));
        }
        app.init_resource::<Frames>().add_systems(Startup, spawn).add_systems(Last, update);
    }
}

#[derive(Component)]
struct PerfText;

/// The last second's frame times (seconds).
#[derive(Resource, Default)]
struct Frames {
    times: Vec<f32>,
    secs: f32,
}

/// What a second of frame times comes to: frames a second, mean and worst frame (ms), and the
/// slowest 1% of frames as a frame rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Second {
    pub fps: f32,
    pub mean_ms: f32,
    pub worst_ms: f32,
    pub low_fps: f32,
}

pub fn summarise(times: &[f32]) -> Option<Second> {
    let total: f32 = times.iter().sum();
    if times.is_empty() || total <= 0.0 {
        return None;
    }
    let mut v = times.to_vec();
    v.sort_by(|a, b| b.total_cmp(a));
    // The slowest 1% (at least one frame), as frames a second.
    let n = (v.len() / 100).max(1);
    let slow = v[..n].iter().sum::<f32>() / n as f32;
    Some(Second {
        fps: times.len() as f32 / total,
        mean_ms: total / times.len() as f32 * 1000.0,
        worst_ms: v[0] * 1000.0,
        low_fps: if slow > 0.0 { 1.0 / slow } else { 0.0 },
    })
}

/// The read-out's text for `mode`.
pub fn line(mode: PerfMonitor, s: Second, preset: &str, size: UVec2, scale: f32) -> String {
    match mode {
        PerfMonitor::Off => String::new(),
        PerfMonitor::Fps => format!("{:.0} FPS", s.fps),
        PerfMonitor::Full => format!(
            "{:.0} FPS  ·  {:.1} ms (worst {:.1})  ·  1% low {:.0} FPS\n{preset}  ·  3D {} x {} ({:.0}%)",
            s.fps, s.mean_ms, s.worst_ms, s.low_fps, size.x, size.y, scale * 100.0
        ),
    }
}

fn spawn(mut commands: Commands) {
    commands.spawn((
        PerfText,
        Text::new(""),
        theme::strong(theme::SMALL),
        TextColor(theme::TEXT),
        TextLayout::justify(Justify::Right),
        BackgroundColor(theme::HUD_BG),
        Node { position_type: PositionType::Absolute, top: Val::Px(6.0), right: Val::Px(8.0), padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)), ..default() },
        GlobalZIndex(40),
        Visibility::Hidden,
    ));
}

fn update(
    time: Res<Time<Real>>,
    mut frames: ResMut<Frames>,
    store: Option<Res<SettingsStore>>,
    over: Option<Res<PerfOverride>>,
    scale: Option<Res<crate::render_scale::ScaleNow>>,
    mut text: Query<(&mut Text, &mut Visibility), With<PerfText>>,
) {
    let mode = over.map(|o| o.0).or_else(|| store.as_ref().map(|s| s.settings.perf_monitor)).unwrap_or_default();
    let Ok((mut t, mut vis)) = text.single_mut() else { return };
    let want = if mode == PerfMonitor::Off { Visibility::Hidden } else { Visibility::Inherited };
    if *vis != want {
        *vis = want;
    }
    let dt = time.delta_secs();
    if dt > 0.0 {
        frames.times.push(dt);
        frames.secs += dt;
    }
    if frames.secs < 1.0 {
        return;
    }
    if mode != PerfMonitor::Off
        && let Some(s) = summarise(&frames.times)
    {
        let g = store.as_ref().map(|s| &s.settings.graphics);
        let preset = g.map_or("".to_string(), |g| format!("{:?}", g.preset));
        let (size, share) = scale.as_ref().map_or((UVec2::ZERO, 1.0), |n| (n.size, n.scale));
        let new = line(mode, s, &preset, size, share);
        if t.0 != new {
            t.0 = new;
        }
    }
    frames.times.clear();
    frames.secs = 0.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_of_frames_sums_up() {
        // 59 frames of 16 ms and one of 50 ms.
        let mut t = vec![0.016; 59];
        t.push(0.05);
        let s = summarise(&t).unwrap();
        assert!((s.fps - 60.0 / (0.016 * 59.0 + 0.05)).abs() < 1e-3);
        assert!((s.worst_ms - 50.0).abs() < 1e-3);
        assert!((s.low_fps - 20.0).abs() < 1e-3);
        assert_eq!(line(PerfMonitor::Fps, s, "High", UVec2::new(3008, 1692), 0.5), "60 FPS");
        let full = line(PerfMonitor::Full, s, "High", UVec2::new(3008, 1692), 0.5);
        assert!(full.contains("1% low 20 FPS") && full.contains("3D 3008 x 1692 (50%)"), "{full}");
        assert_eq!(summarise(&[]), None);
    }

    #[test]
    fn the_setting_reads_and_defaults_to_off() {
        assert_eq!(PerfMonitor::parse("fps"), Some(PerfMonitor::Fps));
        assert_eq!(PerfMonitor::parse("FULL"), Some(PerfMonitor::Full));
        assert_eq!(ron::from_str::<crate::ui_scale::ClientSettings>("(ui_scale: 110)").unwrap().perf_monitor, PerfMonitor::Off);
    }
}
