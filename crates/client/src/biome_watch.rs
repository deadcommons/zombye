//! `--watch-biomes`: try biome settings without leaving the game (docs/biomes.md).
//!
//! Where trees, bushes and bare patches stand is part of the map, which a run builds once, so a
//! changed `biomes.ron` can't be swapped into a running game. Instead the file in use
//! (`content::biomes_path`: `--biomes FILE`, else the data folder's) is looked at twice a second,
//! and when it has been saved the game starts again in this one's place with the same arguments,
//! standing where the survivor stands now, at the same hour, zoom and camera angle: a new world of
//! the same seed with the new biomes. A file with a mistake in it is said on the yellow line and
//! nothing restarts. Only in your own new world: never on a server or in a loaded save.

use std::path::PathBuf;
use std::time::SystemTime;

use bevy::prelude::*;
use zombye_net::content;

use crate::Options;
use crate::camera::IsoCamera;
use crate::join_screen::args_without;
use crate::player::LocalPlayer;
use crate::saving::Notice;
use crate::server_link::ServerLink;

pub struct BiomeWatchPlugin;

impl Plugin for BiomeWatchPlugin {
    fn build(&self, app: &mut App) {
        let options = app.world().resource::<Options>();
        if !options.watch_biomes || options.offscreen() || options.connect.is_some() || options.load.is_some() {
            return;
        }
        let path = content::biomes_path(&options.data);
        println!("watching {} (--watch-biomes): save it and the game starts again with it", path.display());
        app.insert_resource(Watch { stamp: stamp(&path), path, next: 0.0 }).add_systems(Update, watch);
    }
}

#[derive(Resource)]
struct Watch {
    path: PathBuf,
    stamp: Option<SystemTime>,
    next: f32,
}

fn stamp(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// This run's arguments with where and how we look now in place of what it started with.
pub fn again_args(args: &[String], pos: Vec2, hour: f32, zoom: f32, angle: i32) -> Vec<String> {
    let add = [
        "--no-menu".to_string(),
        "--at".to_string(),
        format!("{:.1},{:.1}", pos.x, pos.y),
        "--time".to_string(),
        format!("{:.2}", hour.rem_euclid(24.0)),
        "--zoom".to_string(),
        format!("{zoom:.1}"),
        "--angle".to_string(),
        format!("{}", angle.rem_euclid(4)),
    ];
    let add: Vec<&str> = add.iter().map(String::as_str).collect();
    args_without(args, &["--at", "--time", "--zoom", "--angle", "--skip-hours"], &["--no-menu", "--main-menu"], &add)
}

#[allow(clippy::too_many_arguments)]
fn watch(
    time: Res<Time<Real>>,
    game: Res<Time>,
    options: Res<Options>,
    mut watch: ResMut<Watch>,
    link: Res<ServerLink>,
    player: Option<Res<LocalPlayer>>,
    camera: Query<&IsoCamera>,
    mut notice: ResMut<Notice>,
    mut exit: MessageWriter<AppExit>,
) {
    let now = time.elapsed_secs();
    if now < watch.next {
        return;
    }
    watch.next = now + 0.5;
    let seen = stamp(&watch.path);
    if seen == watch.stamp || seen.is_none() {
        return;
    }
    watch.stamp = seen;
    if let Err(e) = content::load_biomes(&options.data) {
        notice.say(game.elapsed_secs(), format!("biomes not reloaded: {e}"));
        return;
    }
    let pos = player.as_ref().map_or(Vec2::ZERO, |p| p.curr.pos);
    let (zoom, angle) = camera.iter().next().map_or((26.0, 0), |c| (c.zoom, c.angle_index));
    let args: Vec<String> = std::env::args().skip(1).collect();
    notice.say(game.elapsed_secs(), "biomes changed: starting again".to_string());
    crate::main_menu::ask_relaunch(again_args(&args, pos, link.curr.time_of_day, zoom, angle), &options);
    exit.write(AppExit::Success);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starting_again_keeps_the_flags_and_the_place() {
        let args: Vec<String> = "--seed 7 --at 1,2 --biome meadow --watch-biomes --zoom 40".split(' ').map(String::from).collect();
        let next = again_args(&args, Vec2::new(310.25, 96.0), 15.5, 33.0, 6);
        assert_eq!(next.join(" "), "--seed 7 --biome meadow --watch-biomes --no-menu --at 310.2,96.0 --time 15.50 --zoom 33.0 --angle 2");
    }
}
