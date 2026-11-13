//! A new game's choices: who the survivor is ([`zombye_sim::origin`]) and how the world is set
//! ([`zombye_sim::difficulty`]), as the new-game screen hands them to the game.
//!
//! The game builds its world once, at start-up, so the screen is a run of its own that writes a
//! [`NewGame`] to a small RON file and starts the game again with `--new-game-from FILE`. The
//! same choices can be given as flags. [`Rules`] is the data both read: the Backgrounds,
//! Strengths and Flaws and the difficulty presets of a data folder.

use std::path::Path;

use serde::{Deserialize, Serialize};
use zombye_sim::difficulty::{DifficultyData, SandboxOptions, Setup};
use zombye_sim::origin::{Choice, OriginData, Start};
use zombye_sim::{ItemDef, WorldConfig};

use crate::content;

/// What a new game was asked for.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NewGame {
    /// The survivor. `None`: a plain one, as before there was a choice.
    pub survivor: Option<Choice>,
    /// The world's options.
    pub world: Setup,
}

/// The data a new game is chosen from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rules {
    pub origins: OriginData,
    pub difficulty: DifficultyData,
    /// What the Sandbox page offers by name.
    pub sandbox: SandboxOptions,
}

impl Rules {
    /// From `<data>`, checked against `items` (the game's, with its mods').
    pub fn load(data_dir: &Path, items: &[ItemDef]) -> Result<Self, String> {
        Ok(Self { origins: content::load_origins(data_dir, items)?, difficulty: content::load_difficulty(data_dir)?, sandbox: content::load_sandbox(data_dir)? })
    }

    /// Put `game`'s world options onto a fresh `config`, and work out its survivor. The error is
    /// a sentence for the player.
    pub fn begin(&self, game: &NewGame, config: &mut WorldConfig) -> Result<Option<Start>, String> {
        self.difficulty.apply(&game.world, config)?;
        game.survivor.as_ref().map(|c| self.origins.resolve(c, &config.items)).transpose()
    }
}

/// Read a [`NewGame`] file.
pub fn read(path: &Path) -> Result<NewGame, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    ron::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write one (whole, or not at all).
pub fn write(path: &Path, game: &NewGame) -> Result<(), String> {
    let text = ron::ser::to_string_pretty(game, ron::ser::PrettyConfig::default()).map_err(|e| e.to_string())?;
    crate::save_file::write_atomic(path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zombye_sim::Skill;

    #[test]
    fn a_new_game_goes_through_a_file_and_begins_as_chosen() {
        let data = content::default_data_dir();
        let mut config = content::load_config(&data, 3).unwrap();
        let rules = Rules::load(&data, &config.items).unwrap();
        assert!(rules.origins.backgrounds.len() >= 8 && rules.difficulty.presets.len() >= 3 && !rules.sandbox.groups.is_empty());
        let hard = rules.difficulty.presets.last().unwrap().id.clone();
        let game = NewGame {
            survivor: Some(Choice { background: "night_nurse".into(), strengths: vec!["strong_back".into()], flaws: vec!["big_appetite".into()], ..rules.origins.default_choice() }),
            world: Setup { difficulty: hard, values: [("zombie.memory_secs".to_string(), "40".to_string())].into(), ..Default::default() },
        };
        let dir = std::env::temp_dir().join(format!("zombye-newgame-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("new_game.ron");
        write(&path, &game).unwrap();
        assert_eq!(read(&path).unwrap(), game);
        std::fs::remove_dir_all(&dir).ok();

        let plain = config.population.scale;
        let start = rules.begin(&game, &mut config).unwrap().expect("a survivor");
        assert!(config.population.scale > plain);
        assert_eq!(config.zombie.memory_secs, 40.0);
        let mut server = crate::LocalServer::new(config);
        let id = server.join(None);
        assert!(server.world.begin_as(id, &start));
        let p = server.world.player(id).unwrap();
        assert_eq!(p.skills.level(Skill::FirstAid, &server.world.config.skills), 3);
        assert_eq!(p.origin.background, "night_nurse");
        assert!(p.inventory.worn.len() >= 3 && !p.inventory.carried.is_empty());
        // Too many Strengths for the points is a sentence, not a world.
        let greedy = NewGame { survivor: Some(Choice { background: "site_labourer".into(), strengths: vec!["iron_constitution".into()], ..Default::default() }), ..Default::default() };
        let mut c = content::load_config(&data, 3).unwrap();
        assert!(rules.begin(&greedy, &mut c).unwrap_err().contains("over"));
        // No file says nothing about the survivor: a plain one, in the game as shipped.
        assert_eq!(rules.begin(&NewGame::default(), &mut c).unwrap(), None);
    }
}
