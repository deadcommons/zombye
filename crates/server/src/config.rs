//! `zombye-server --config FILE`: the dedicated server's settings in one RON file (#20,
//! docs/PLAN.md §3a: "the config is one file"; docs/self-hosting.md).
//!
//! The file says nothing the flags can't: it is turned into the flags it stands for
//! ([`ServerConfig::flags`]) and those are put behind the command line ([`merge`]), so every
//! setting is parsed and checked in one place (`multiplayer::HostArgs`, `main`), and a flag given
//! on the command line wins over the file. A file that doesn't exist yet is written with the
//! defaults ([`TEMPLATE`]), for the operator to edit.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The commented file a server writes when `--config` names one that isn't there. Every value in
/// it is the default (a test holds it to [`ServerConfig::default`]).
pub const TEMPLATE: &str = include_str!("../server.example.ron");

/// The save a configured server keeps its world in when the file doesn't say, beside the file.
const DEFAULT_SAVE: &str = "world.zsave";

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub name: Option<String>,
    pub listen: Option<String>,
    pub max_players: Option<u32>,
    pub password: Option<String>,
    pub pvp: Option<bool>,
    pub spawn: Option<SpawnRule>,
    pub fair_sight: Option<bool>,
    pub logout_secs: Option<f32>,
    pub logout_safe_secs: Option<f32>,
    pub spawn_protection_secs: Option<f32>,
    /// What new survivors start with (#69). Left out: the data folder's `start_kit.ron`.
    pub start_kit: Option<Vec<KitItem>>,
    pub camera: Option<CameraRule>,
    pub agents: Option<AgentsRule>,
    pub budget_kb: Option<u32>,
    pub allow_dev: bool,
    /// `None` only when the file says `save: None`: a world that is never saved.
    pub save: Option<PathBuf>,
    pub autosave_mins: Option<f64>,
    pub seed: Option<u64>,
    pub map_chunks: Option<u32>,
    pub zombies: Option<u32>,
    pub load_anyway: bool,
    pub data: Option<PathBuf>,
    pub admin: Option<AdminConfig>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            name: None,
            listen: None,
            max_players: None,
            password: None,
            pvp: None,
            spawn: None,
            fair_sight: None,
            logout_secs: None,
            logout_safe_secs: None,
            spawn_protection_secs: None,
            start_kit: None,
            camera: None,
            agents: None,
            budget_kb: None,
            allow_dev: false,
            save: Some(PathBuf::from(DEFAULT_SAVE)),
            autosave_mins: None,
            seed: None,
            map_chunks: None,
            zombies: None,
            load_anyway: false,
            data: None,
            admin: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub enum SpawnRule {
    Spread,
    Together,
}

/// The camera rule (#69, `wire::Camera`).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub enum CameraRule {
    Isometric,
    FirstPerson,
}

/// The agents rule (#69, `wire::Agents`): programs refused, let in unmarked, or let in and marked.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub enum AgentsRule {
    Refused,
    Allowed,
    Marked,
}

/// One entry of `start_kit`, as in the data folder's `start_kit.ron`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KitItem {
    pub item: String,
    #[serde(default = "one")]
    pub count: u16,
}

fn one() -> u16 {
    1
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminConfig {
    pub listen: Option<String>,
    pub token: Option<String>,
    pub origins: Vec<String>,
}

/// One flag and its values, as on the command line.
type Flag = (&'static str, Vec<String>);

impl ServerConfig {
    /// Parse a config file's text. Options are written bare (`pvp: true`, not `Some(true)`).
    pub fn parse(text: &str) -> Result<Self, String> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        options.from_str(text).map_err(|e| e.to_string())
    }

    /// The flags this file stands for. Paths are taken from `dir`, the file's folder. With
    /// `env_admin_token` (`ZOMBYE_ADMIN_TOKEN` is set) the file's own admin token is left out,
    /// so the environment's wins.
    pub fn flags(&self, dir: &Path, env_admin_token: bool) -> Vec<Flag> {
        let mut flags: Vec<Flag> = Vec::new();
        let mut put = |name: &'static str, value: Option<String>| {
            if let Some(v) = value {
                flags.push((name, vec![v]));
            }
        };
        let on_off = |b: bool| if b { "on" } else { "off" }.to_string();
        let path = |p: &PathBuf| dir.join(p).to_string_lossy().into_owned();
        put("--host", Some(self.listen.clone().unwrap_or_default()).map(|l| l.trim().to_string()).filter(|l| !l.is_empty()));
        put("--name", self.name.clone());
        put("--max-players", self.max_players.map(|n| n.to_string()));
        put("--password", self.password.clone().filter(|p| !p.is_empty()));
        put("--pvp", self.pvp.map(on_off));
        put("--spawn", self.spawn.map(|s| match s {
            SpawnRule::Spread => "spread".to_string(),
            SpawnRule::Together => "together".to_string(),
        }));
        put("--fair-sight", self.fair_sight.map(on_off));
        put("--logout-secs", self.logout_secs.map(|s| s.to_string()));
        put("--logout-safe-secs", self.logout_safe_secs.map(|s| s.to_string()));
        put("--spawn-protection", self.spawn_protection_secs.map(|s| s.to_string()));
        put("--start-kit", self.start_kit.as_ref().map(|kit| if kit.is_empty() { "none".to_string() } else { kit.iter().map(|k| format!("{}:{}", k.item, k.count)).collect::<Vec<_>>().join(",") }));
        put("--camera", self.camera.map(|c| match c {
            CameraRule::Isometric => "isometric".to_string(),
            CameraRule::FirstPerson => "first-person".to_string(),
        }));
        put("--agents", self.agents.map(|a| match a {
            AgentsRule::Refused => "off".to_string(),
            AgentsRule::Allowed => "on".to_string(),
            AgentsRule::Marked => "marked".to_string(),
        }));
        put("--budget", self.budget_kb.map(|kb| kb.to_string()));
        put("--save", self.save.as_ref().filter(|p| !p.as_os_str().is_empty()).map(path));
        put("--autosave-mins", self.autosave_mins.map(|m| m.to_string()));
        put("--seed", self.seed.map(|s| s.to_string()));
        put("--map-chunks", self.map_chunks.map(|n| n.to_string()));
        put("--zombies", self.zombies.map(|n| n.to_string()));
        put("--data", self.data.as_ref().map(path));
        if let Some(admin) = &self.admin {
            put("--admin-mcp", Some(admin.listen.clone().unwrap_or_default()));
            put("--admin-token", admin.token.clone().filter(|t| !t.trim().is_empty() && !env_admin_token));
            for origin in &admin.origins {
                put("--admin-origin", Some(origin.clone()));
            }
        }
        if self.allow_dev {
            flags.push(("--allow-dev", Vec::new()));
        }
        if self.load_anyway {
            flags.push(("--load-anyway", Vec::new()));
        }
        // A flag whose value is empty is the bare flag (`--admin-mcp`: its default address).
        for (_, values) in &mut flags {
            values.retain(|v| !v.is_empty());
        }
        flags
    }
}

/// The command line with the file's flags behind it: a flag already on the command line is the
/// operator's and the file's is dropped (all of them, for one that repeats). A config file means
/// a hosting server, so `--host` is added if neither says where.
pub fn merge(cli: &[String], file: Vec<Flag>) -> Vec<String> {
    let mut args = cli.to_vec();
    for (name, values) in file {
        if cli.iter().any(|a| a == name) {
            continue;
        }
        args.push(name.to_string());
        args.extend(values);
    }
    if !args.iter().any(|a| a == "--host") {
        args.push("--host".to_string());
    }
    args
}

/// Something a config file does that the operator should hear about (printed by `main`).
#[derive(Debug, Default)]
pub struct Loaded {
    pub args: Vec<String>,
    pub notes: Vec<String>,
}

/// `--config PATH`: read the file there (writing the defaults first if there is none) and give
/// the whole command line it and `cli` make. An unreadable or mistaken file is an error naming it:
/// a server never starts on settings other than the ones its operator wrote.
pub fn load(path: &Path, cli: &[String], env_admin_token: bool) -> Result<Loaded, String> {
    let mut notes = Vec::new();
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let made = path.parent().filter(|d| !d.as_os_str().is_empty()).map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(path, TEMPLATE));
            notes.push(match made {
                Ok(()) => format!("no config at {}: wrote one with the defaults, edit it and restart to change them", path.display()),
                Err(e) => format!("no config at {} and it can't be written ({e}): going by the defaults", path.display()),
            });
            TEMPLATE.to_string()
        }
        Err(e) => return Err(format!("can't read the config {}: {e}", path.display())),
    };
    let config = ServerConfig::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    Ok(Loaded { args: merge(cli, config.flags(dir, env_admin_token)), notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiplayer::HostArgs;
    use zombye_net::host::Spawn;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    fn value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
    }

    /// The file a server writes for its operator is the defaults, and says a word on every setting.
    #[test]
    fn the_template_is_the_defaults_and_names_every_setting() {
        let config = ServerConfig::parse(TEMPLATE).expect("the shipped template parses");
        // What it sets, it sets to what the server does without it.
        let args = merge(&[], config.flags(Path::new("/srv/zombye"), false));
        let from_file = HostArgs::parse(&args, None).expect("its flags are valid");
        let plain = HostArgs::parse(&strings(&["--host"]), None).unwrap();
        assert_eq!(from_file.addr, plain.addr);
        assert_eq!(format!("{:?}", from_file.options), format!("{:?}", plain.options));
        assert_eq!((from_file.pvp, from_file.admin.is_none()), (None, true));
        assert_eq!((value(&args, "--seed"), value(&args, "--map-chunks"), value(&args, "--autosave-mins")), (Some("1"), Some("30"), Some("5")));
        assert_eq!(value(&args, "--save"), Some("/srv/zombye/world.zsave"));
        for unset in ["--zombies", "--data", "--allow-dev", "--load-anyway", "--admin-mcp", "--pvp", "--spawn", "--fair-sight", "--password"] {
            assert!(!args.iter().any(|a| a == unset), "{unset} in {args:?}");
        }
        // Every field is in it, set or as a commented example.
        for field in [
            "name", "listen", "max_players", "password", "pvp", "spawn", "fair_sight", "logout_secs", "logout_safe_secs", "spawn_protection_secs", "start_kit", "camera", "agents", "budget_kb", "allow_dev", "save",
            "autosave_mins", "seed", "map_chunks", "zombies", "load_anyway", "data", "admin", "token", "origins",
        ] {
            assert!(TEMPLATE.contains(&format!("{field}:")), "{field} is explained in the template");
        }
        // And with every commented example switched on it still parses: the examples are right.
        let all: String = TEMPLATE.lines().map(|l| l.trim_start().strip_prefix("// ").filter(|rest| rest.split_once(':').is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_lowercase() || c == '_'))).unwrap_or(l)).collect::<Vec<_>>().join("\n");
        let all = ServerConfig::parse(&all).unwrap_or_else(|e| panic!("{e} in:\n{all}"));
        assert_eq!((all.pvp, all.spawn, all.fair_sight, all.zombies), (Some(true), Some(SpawnRule::Spread), Some(true), Some(3000)));
        assert_eq!(all.admin.unwrap().token.as_deref(), Some("a-long-secret-token"));
        assert_eq!((all.camera, all.agents), (Some(CameraRule::Isometric), Some(AgentsRule::Marked)));
        assert_eq!(all.start_kit, Some(vec![KitItem { item: "water_bottle".into(), count: 1 }, KitItem { item: "rag".into(), count: 2 }]));
        // The template leaves the start kit to the game data, and its example is one the shipped items take.
        assert!(!args.iter().any(|a| a == "--start-kit") && from_file.start_kit.is_none());
        let items = zombye_net::content::load_config(&zombye_net::content::default_data_dir(), 1).expect("the shipped content").items;
        let example = merge(&[], ServerConfig { start_kit: all.start_kit.clone(), ..ServerConfig::default() }.flags(Path::new("."), false));
        let kit = HostArgs::parse(&example, None).unwrap().kit(Vec::new(), &items).expect("the example kit is of shipped items");
        assert_eq!(kit, [("water_bottle".to_string(), 1), ("rag".to_string(), 2)]);
    }

    /// The rules of #69 (the camera, agents, the start kit) are read from the file, and a kit of
    /// things the game data hasn't got is refused by name.
    #[test]
    fn the_rules_are_read_from_the_file() {
        use zombye_net::wire::{Agents, Camera};
        let items = zombye_net::content::load_config(&zombye_net::content::default_data_dir(), 1).expect("the shipped content").items;
        let from_data = vec![("matches".to_string(), 1)];
        let parse = |text: &str| HostArgs::parse(&merge(&[], ServerConfig::parse(text).unwrap().flags(Path::new("."), false)), None);
        let plain = parse("()").unwrap();
        assert_eq!((plain.options.camera, plain.options.agents), (Camera::Isometric, Agents::Marked), "the defaults");
        assert_eq!(plain.kit(from_data.clone(), &items).unwrap(), from_data, "no kit in the file: the game data's");
        let set = parse(r#"(camera: FirstPerson, agents: Refused, start_kit: [(item: "crowbar"), (item: "rag", count: 3)])"#).unwrap();
        assert_eq!((set.options.camera, set.options.agents), (Camera::FirstPerson, Agents::Refused));
        assert_eq!(set.kit(from_data.clone(), &items).unwrap(), [("crowbar".to_string(), 1), ("rag".to_string(), 3)]);
        assert_eq!(parse("(agents: Allowed)").unwrap().options.agents, Agents::Allowed);
        // An empty list is nothing at all, not the game data's.
        assert!(parse("(start_kit: [])").unwrap().kit(from_data.clone(), &items).unwrap().is_empty());
        // Mistakes: in the file, by the parser; in the kit, against the items.
        assert!(ServerConfig::parse("(camera: TopDown)").is_err() && ServerConfig::parse("(agents: Maybe)").is_err());
        assert!(ServerConfig::parse(r#"(start_kit: [(thing: "rag")])"#).unwrap_err().contains("thing"));
        let unknown = parse(r#"(start_kit: [(item: "lightsaber")])"#).unwrap().kit(from_data.clone(), &items).unwrap_err();
        assert!(unknown.contains("--start-kit") && unknown.contains("lightsaber: no such item"), "{unknown}");
        assert!(parse(r#"(start_kit: [(item: "rag", count: 50)])"#).unwrap().kit(from_data, &items).unwrap_err().contains("1 to 20"));
        // The flags say the same, and win over the file.
        let cli = strings(&["--agents", "on", "--camera", "first-person", "--start-kit", "none"]);
        let both = HostArgs::parse(&merge(&cli, ServerConfig::parse("(agents: Refused, camera: Isometric, start_kit: [(item: \"rag\")])").unwrap().flags(Path::new("."), false)), None).unwrap();
        assert_eq!((both.options.agents, both.options.camera, both.start_kit), (Agents::Allowed, Camera::FirstPerson, Some(Vec::new())));
        assert!(HostArgs::parse(&strings(&["--host", "--camera", "top-down"]), None).is_err_and(|e| e.contains("isometric or first-person")));
        assert!(HostArgs::parse(&strings(&["--host", "--agents", "some"]), None).is_err_and(|e| e.contains("off, on or marked")));
        assert!(HostArgs::parse(&strings(&["--host", "--start-kit", "rag:lots"]), None).is_err_and(|e| e.contains("rag:2")));
    }

    #[test]
    fn a_file_sets_what_the_flags_set() {
        let text = r#"(
            name: "Dead Commons",
            listen: "127.0.0.1:7001",
            max_players: 8,
            password: "let me in",
            pvp: false,
            spawn: Spread,
            fair_sight: true,
            logout_secs: 12.5,
            logout_safe_secs: 2,
            spawn_protection_secs: 0,
            budget_kb: 64,
            allow_dev: true,
            save: "worlds/town.zsave",
            autosave_mins: 0.5,
            seed: 42,
            map_chunks: 12,
            zombies: 150,
            load_anyway: true,
            data: "/opt/zombye/data",
            admin: (listen: "0.0.0.0:9000", token: "the-operators-own-token", origins: ["https://a.example", "https://b.example"]),
        )"#;
        let config = ServerConfig::parse(text).unwrap();
        let args = merge(&strings(&["--config", "x"]), config.flags(Path::new("/srv"), false));
        let host = HostArgs::parse(&args, None).unwrap();
        assert_eq!(host.addr, "127.0.0.1:7001");
        let o = &host.options;
        assert_eq!((o.name.as_str(), o.max_players, o.password.as_str(), o.allow_dev, o.budget), ("Dead Commons", 8, "let me in", true, 64_000));
        assert_eq!((o.spawn, o.fair_sight, host.pvp), (Some(Spawn::Spread), Some(true), Some(false)));
        assert_eq!((o.logout_secs, o.logout_safe_secs, o.spawn_protection_secs), (12.5, 2.0, 0.0));
        let admin = host.admin.expect("the admin endpoint");
        assert_eq!((admin.addr.as_str(), admin.token.as_deref(), admin.origins.len()), ("0.0.0.0:9000", Some("the-operators-own-token"), 2));
        assert_eq!((value(&args, "--save"), value(&args, "--data")), (Some("/srv/worlds/town.zsave"), Some("/opt/zombye/data")), "relative paths are from the file's folder, absolute ones kept");
        assert_eq!((value(&args, "--seed"), value(&args, "--map-chunks"), value(&args, "--zombies"), value(&args, "--autosave-mins")), (Some("42"), Some("12"), Some("150"), Some("0.5")));
        assert!(args.iter().any(|a| a == "--load-anyway"));
    }

    #[test]
    fn the_command_line_wins_and_the_environments_token() {
        let config = ServerConfig::parse(r#"(name: "from the file", listen: "0.0.0.0:7001", max_players: 8, admin: (token: "the-files-own-token-here", origins: ["https://a.example"]))"#).unwrap();
        let cli = strings(&["--config", "server.ron", "--host", "127.0.0.1:0", "--max-players", "4", "--admin-origin", "https://cli.example"]);
        let args = merge(&cli, config.flags(Path::new("."), true));
        let host = HostArgs::parse(&args, Some("the-environments-token".into())).unwrap();
        assert_eq!((host.addr.as_str(), host.options.max_players, host.options.name.as_str()), ("127.0.0.1:0", 4, "from the file"));
        let admin = host.admin.unwrap();
        assert_eq!(admin.token.as_deref(), Some("the-environments-token"));
        assert_eq!(admin.origins, ["https://cli.example"], "a repeated flag is the command line's alone");
        assert_eq!(admin.addr, "127.0.0.1:8788", "no address in the file: the endpoint's default");
        // Without the environment's, the file's token is the one.
        let args = merge(&cli, config.flags(Path::new("."), false));
        assert_eq!(HostArgs::parse(&args, None).unwrap().admin.unwrap().token.as_deref(), Some("the-files-own-token-here"));
    }

    #[test]
    fn mistakes_are_refused_by_name() {
        let unknown = ServerConfig::parse("(max_playerz: 8)").unwrap_err();
        assert!(unknown.contains("max_playerz"), "{unknown}");
        assert!(ServerConfig::parse("(max_players: \"many\")").is_err());
        assert!(ServerConfig::parse("(admin: (port: 1))").unwrap_err().contains("port"));
        assert!(ServerConfig::parse("(spawn: Anywhere)").is_err());
        // Values a file can hold but a server can't use are caught where the flags' are.
        let args = merge(&[], ServerConfig::parse("(max_players: 500)").unwrap().flags(Path::new("."), false));
        assert!(HostArgs::parse(&args, None).is_err_and(|e| e.contains("--max-players is 1 to 64")));
        let args = merge(&[], ServerConfig::parse("(admin: ())").unwrap().flags(Path::new("."), false));
        assert!(HostArgs::parse(&args, None).is_err_and(|e| e.contains("needs a token")));
        // An empty file is a server on the defaults; no save only when it says so.
        let empty = ServerConfig::parse("()").unwrap();
        assert_eq!(empty, ServerConfig::default());
        assert!(ServerConfig::parse("(save: None)").unwrap().flags(Path::new("."), false).iter().all(|(name, _)| *name != "--save"));
    }

    #[test]
    fn a_missing_file_is_written_with_the_defaults() {
        let dir = std::env::temp_dir().join(format!("zombye-config-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("etc").join("server.ron");
        let loaded = load(&path, &strings(&["--config", "x"]), false).unwrap();
        assert!(loaded.notes[0].contains("wrote one with the defaults"), "{:?}", loaded.notes);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TEMPLATE);
        assert_eq!(value(&loaded.args, "--save").map(PathBuf::from), Some(dir.join("etc").join("world.zsave")));
        assert!(loaded.args.iter().any(|a| a == "--host"));
        // The second start reads it, silently; a broken one stops the server, naming the file.
        assert!(load(&path, &[], false).unwrap().notes.is_empty());
        std::fs::write(&path, "(name: 7").unwrap();
        let broken = load(&path, &[], false).unwrap_err();
        assert!(broken.contains("server.ron"), "{broken}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
