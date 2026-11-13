//! `ZOMBYE_FPS_LOG=1`: print the frame rate every second and a summary on exit (stdout), for
//! measuring a windowed game the same way across builds.
use bevy::prelude::*;

pub struct FpsLogPlugin;

impl Plugin for FpsLogPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var_os("ZOMBYE_FPS_LOG").is_some() {
            app.init_resource::<FpsLog>().add_systems(Last, log_fps);
        }
    }
}

#[derive(Resource, Default)]
struct FpsLog {
    frames: u32,
    secs: f32,
    worst: f32,
    all: Vec<f32>,
    total_secs: f32,
}

fn log_fps(time: Res<Time<Real>>, mut log: ResMut<FpsLog>, mut exit: MessageReader<AppExit>) {
    let dt = time.delta_secs();
    if dt > 0.0 && time.elapsed_secs() > 4.0 {
        log.frames += 1;
        log.secs += dt;
        log.worst = log.worst.max(dt);
        log.all.push(dt * 1000.0);
        log.total_secs += dt;
        if log.secs >= 1.0 {
            println!("fps {:.1} | frame {:.2} ms mean, {:.2} ms worst", log.frames as f32 / log.secs, log.secs / log.frames as f32 * 1000.0, log.worst * 1000.0);
            (log.frames, log.secs, log.worst) = (0, 0.0, 0.0);
        }
    }
    if exit.read().next().is_some() && !log.all.is_empty() {
        let mut v = log.all.clone();
        v.sort_by(f32::total_cmp);
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        println!("fps summary: {:.1} fps, frame {:.2} ms mean, median {:.2}, p99 {:.2} over {} frames", 1000.0 / mean, mean, v[v.len() / 2], v[(v.len() * 99 / 100).min(v.len() - 1)], v.len());
    }
}
