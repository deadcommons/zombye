//! Mods end to end (issue #25): the sample mod's item behaviour in a real world, the sandbox, the
//! limits, and that scripts leave the simulation deterministic and saves exact.

use std::path::PathBuf;

use glam::Vec2;
use zombye_mods::{Mod, apply, load_into};
use zombye_net::content;
use zombye_sim::script::Value;
use zombye_sim::{Place, PlayerInput, World, WorldConfig, ZombieState};

fn base(seed: u64) -> WorldConfig {
    let mut config = content::load_config(&content::default_data_dir(), seed).unwrap().with_map_chunks(8);
    config.zombies = Some(0);
    config.start_hour = 12.0;
    config
}

fn sample_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../mods/alarm_clock")
}

/// The game's data with the sample mod.
fn with_sample(seed: u64) -> WorldConfig {
    let mut config = base(seed);
    let mods = load_into(&mut config, &[sample_dir()]).unwrap();
    assert_eq!(mods.len(), 1);
    config
}

/// The game's data with one script as a mod called `test`.
fn with_script(seed: u64, source: &str) -> WorldConfig {
    let mut config = base(seed);
    apply(&mut config, &[Mod::new("test").with_script("test.luau", source)]).unwrap();
    config
}

fn steps(w: &mut World, n: u32) {
    for _ in 0..n {
        w.step();
    }
}

fn stored<'a>(w: &'a World, key: &str) -> Option<&'a Value> {
    w.script_get("test", key)
}

fn num(w: &World, key: &str) -> f64 {
    match stored(w, key) {
        Some(Value::Num(n)) => *n,
        other => panic!("{key}: {other:?} (errors: {:?})", w.script_errors()),
    }
}

/// A world with the sample mod, a survivor out in the open that zombies ignore, holding an alarm
/// clock, and a dozen zombies 25 m east. Returns the world, the player and where it stands.
fn clock_world(seed: u64) -> (World, u32, Vec2) {
    let mut w = World::new(with_sample(seed));
    let id = w.add_player(Vec2::new(24.5, 24.5));
    w.set_dev(id, None, Some(true));
    let at = w.player(id).unwrap().state.pos;
    assert_eq!(w.spawn_zombies(12, at + Vec2::new(25.0, 0.0), 4.0), 12);
    assert!(w.give_item(id, "alarm_clock", 1));
    (w, id, at)
}

fn zombies_within(w: &World, at: Vec2, radius: f32) -> usize {
    let z = &w.zombies;
    (0..z.len()).filter(|&i| z.state[i] != ZombieState::Dead && z.pos[i].distance(at) <= radius).count()
}

fn ground(w: &World, item: &str) -> usize {
    w.items().iter().filter(|it| w.item_def(it.stack.item).id == item).count()
}

#[test]
fn the_sample_mod_gives_the_alarm_clock_a_use() {
    // The game alone: an alarm clock is junk, and using it is refused.
    let mut plain = World::new(base(3));
    let p = plain.add_player(Vec2::new(24.5, 24.5));
    assert!(plain.give_item(p, "alarm_clock", 1));
    assert!(plain.player_use(p, Place::Carried, 0).is_err());
    assert!(!plain.has_scripts() && plain.script_mods().is_empty());

    let (mut w, id, at) = clock_world(3);
    assert!(w.has_scripts());
    assert_eq!(w.script_mods().len(), 1);
    assert!(w.item_def(w.item_index("alarm_clock").unwrap()).usable());
    w.player_use(id, Place::Carried, 0).unwrap();
    steps(&mut w, 30);
    // Used: out of the pockets, on the floor wound up, and the player was told.
    assert_eq!(w.script_count(id, "alarm_clock"), 0);
    assert_eq!(ground(&w, "alarm_clock_set"), 1, "errors: {:?}", w.script_errors());
    assert!(w.item_notes.iter().any(|n| n.player == id && n.text.contains("rings in 10 seconds")), "{:?}", w.item_notes);
    assert!(matches!(w.script_get("alarm_clock", "clocks"), Some(Value::List(l)) if l.len() == 1));
    // Nothing has rung yet: nobody came.
    steps(&mut w, 100);
    assert_eq!(zombies_within(&w, at, 8.0), 0);
    // It rings from 10 s to 30 s; the zombies that heard it walk over.
    steps(&mut w, 1100);
    assert!(w.item_notes.iter().any(|n| n.text.contains("ringing")));
    let came = zombies_within(&w, at, 8.0);
    assert!(came >= 6, "{came} of 12 zombies came to the clock; errors: {:?}", w.script_errors());
    // Run down: an ordinary clock on the floor again, and the mod has forgotten it.
    assert_eq!((ground(&w, "alarm_clock_set"), ground(&w, "alarm_clock")), (0, 1));
    assert!(w.script_get("alarm_clock", "clocks").is_none());
    assert!(w.script_errors().is_empty(), "{:?}", w.script_errors());

    // The same world with the clock left in the pocket: nobody comes.
    let (mut quiet, _, at) = clock_world(3);
    steps(&mut quiet, 1230);
    assert_eq!(zombies_within(&quiet, at, 8.0), 0);
}

#[test]
fn picking_the_clock_up_stops_it() {
    let (mut w, id, at) = clock_world(4);
    w.player_use(id, Place::Carried, 0).unwrap();
    steps(&mut w, 40);
    assert_eq!(ground(&w, "alarm_clock_set"), 1);
    assert!(w.player_pick_up(id).is_some(), "the clock lies at the player's feet");
    steps(&mut w, 600);
    assert_eq!(w.script_count(id, "alarm_clock_set"), 1);
    assert!(w.script_get("alarm_clock", "clocks").is_none());
    assert_eq!(zombies_within(&w, at, 8.0), 0, "a clock in the pocket doesn't ring");
}

/// The same seed and inputs give the same world tick for tick, scripts and all.
#[test]
fn a_world_with_scripts_repeats_exactly() {
    let run = || {
        let (mut w, id, _) = clock_world(5);
        let mut hashes = Vec::new();
        for t in 0..700 {
            if t == 20 {
                w.player_use(id, Place::Carried, 0).unwrap();
            }
            if t == 100 {
                // Walk off east while it ticks.
                w.set_player_input(id, 1, PlayerInput { move_dir: Vec2::new(-1.0, 0.0), ..Default::default() });
            }
            w.step();
            hashes.push(w.state_hash());
        }
        (hashes, w.script_errors().to_vec())
    };
    let (a, errors) = run();
    let (b, _) = run();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(a, b);
    // And the scripts' state is part of that hash: forgetting it changes it.
    let (mut w, id, _) = clock_world(5);
    w.player_use(id, Place::Carried, 0).unwrap();
    steps(&mut w, 60);
    let with = w.state_hash();
    w.script_set("alarm_clock", "clocks", None).unwrap();
    assert_ne!(with, w.state_hash());
}

/// Saved while the clock ticks, and again while it rings: the loaded world carries on exactly
/// like the one that was never saved.
#[test]
fn a_loaded_world_carries_on_with_its_scripts() {
    let config = with_sample(6);
    let (mut w, id, at) = clock_world(6);
    w.player_use(id, Place::Carried, 0).unwrap();
    steps(&mut w, 90);
    for round in 0..2 {
        let bytes = w.save();
        let loaded = World::load_with(&bytes, &config, Default::default()).unwrap();
        assert!(loaded.notes.is_empty(), "{:?}", loaded.notes);
        let mut copy = loaded.world;
        // The dev toggles aren't saved.
        copy.set_dev(id, None, Some(true));
        assert_eq!(copy.state_hash(), w.state_hash());
        assert_eq!(copy.script_state(), w.script_state());
        for t in 0..260 {
            w.step();
            copy.step();
            assert_eq!(copy.state_hash(), w.state_hash(), "round {round}, tick {t} after the load");
        }
        w = copy;
    }
    steps(&mut w, 700);
    assert!(zombies_within(&w, at, 8.0) >= 6);
    assert_eq!(ground(&w, "alarm_clock"), 1);
    assert!(w.script_errors().is_empty(), "{:?}", w.script_errors());

    // Loaded without the mod: refused (the save holds an item only the mod has) unless forced...
    let (mut w, id, _) = clock_world(6);
    w.player_use(id, Place::Carried, 0).unwrap();
    steps(&mut w, 90);
    let bytes = w.save();
    assert!(World::load(&bytes, &base(6)).is_err());
    // ...and with another version of its script, it loads with a note saying so.
    let mut changed = base(6);
    let mut sample = Mod::load(&sample_dir()).unwrap();
    sample.scripts[0].1.push_str("\n-- a newer version\n");
    apply(&mut changed, &[sample]).unwrap();
    let loaded = World::load_with(&bytes, &changed, Default::default()).unwrap();
    assert!(loaded.notes.iter().any(|n| n.contains("alarm_clock") && n.contains("changed")), "{:?}", loaded.notes);
}

/// A world whose mods do nothing is the world without them: same hash, same save.
#[test]
fn idle_scripts_leave_the_world_as_it_was() {
    let world = |config: WorldConfig| {
        let mut w = World::new(WorldConfig { zombies: Some(300), ..config });
        let id = w.add_player(w.player_spawn);
        w.set_player_input(id, 1, PlayerInput { move_dir: Vec2::new(1.0, 0.2), run: true, ..Default::default() });
        w
    };
    let mut plain = world(base(7));
    let mut modded = world(with_script(7, "function on_tick(game) local t = game:tick() end\nfunction on_hit(game, player, zombie) end"));
    assert!(modded.has_scripts());
    for t in 0..300 {
        plain.step();
        modded.step();
        assert_eq!(plain.state_hash(), modded.state_hash(), "tick {t}");
    }
    assert!(modded.script_state().is_empty());
    // No mods at all: the config is untouched.
    let mut config = base(7);
    apply(&mut config, &[]).unwrap();
    assert!(config.scripts.is_none());
}

#[test]
fn scripts_are_sandboxed() {
    // Nothing that reads the machine, the clock or files, or loads code, is there.
    let probe = r#"
        function on_spawn(game, player)
            game:set("gone", {
                os = type(os), io = type(io), require = type(require), loadstring = type(loadstring),
                getfenv = type(getfenv), setfenv = type(setfenv), collectgarbage = type(collectgarbage),
                debug = type(debug), package = type(package), random = type(math.random),
                randomseed = type(math.randomseed), newproxy = type(newproxy),
            })
            game:set("there", { type(string.format), type(table.insert), type(math.floor), type(bit32.band), type(utf8.char) })
        end
    "#;
    let mut w = World::new(with_script(8, probe));
    w.add_player(Vec2::new(24.5, 24.5));
    w.step();
    let Some(Value::Map(gone)) = stored(&w, "gone") else { panic!("{:?}", w.script_errors()) };
    assert_eq!(gone.len(), 12);
    for (name, kind) in gone {
        assert_eq!(kind, &Value::Str("nil".into()), "{name} is reachable");
    }
    assert_eq!(stored(&w, "there"), Some(&Value::List(vec![Value::Str("function".into()); 5])));

    // The libraries can't be changed, and `game` is no use outside the call it was handed to.
    for bad in ["string.format = nil", "math.huge = 1", "table.insert(string, 1)"] {
        let mut config = base(8);
        let e = apply(&mut config, &[Mod::new("test").with_script("bad.luau", bad)]).unwrap_err();
        assert!(e.contains("mod test: bad.luau"), "{bad}: {e}");
    }
    let keeps = "local kept\nfunction on_spawn(game) kept = game end\nfunction on_tick(game) if kept and game:tick() > 2 then kept:noise(24, 24, 100) end end";
    let mut w = World::new(with_script(8, keeps));
    w.add_player(Vec2::new(24.5, 24.5));
    steps(&mut w, 6);
    assert!(!w.script_errors().is_empty(), "a kept game object must not work");

    // One mod can't see another's store or globals.
    let mut config = base(8);
    let a = Mod::new("a").with_script("a.luau", "shared = 1\nfunction on_tick(game) game:set('mine', 1) end");
    let b = Mod::new("b").with_script("b.luau", "function on_tick(game) game:set('saw', { type(shared), type(game:get('mine')) }) end");
    apply(&mut config, &[a, b]).unwrap();
    let mut w = World::new(config);
    steps(&mut w, 2);
    assert_eq!(w.script_get("b", "saw"), Some(&Value::List(vec![Value::Str("nil".into()), Value::Str("nil".into())])));
}

/// A script that never returns is cut off; the world ticks on, and the same way every time.
#[test]
fn a_runaway_script_is_cut_off() {
    let run = |source: &str| {
        let mut w = World::new(with_script(9, source));
        w.add_player(Vec2::new(24.5, 24.5));
        let start = std::time::Instant::now();
        steps(&mut w, 20);
        assert!(start.elapsed().as_secs_f32() < 5.0, "20 ticks took {:?}", start.elapsed());
        assert_eq!(w.tick, 20);
        (w.script_errors().to_vec(), w.state_hash(), w.script_state().clone())
    };
    let forever = "function on_tick(game) local n = game:get('n') or 0 game:set('n', n + 1) while true do end end";
    let (errors, hash, state) = run(forever);
    assert!(errors.iter().all(|e| e.contains("mod test: on_tick") && e.contains("ran too long")) && !errors.is_empty(), "{errors:?}");
    // What it did before the loop stands, each tick.
    assert_eq!(state.store["test"]["n"], Value::Num(20.0));
    assert_eq!(run(forever).1, hash);
    // Catching the error doesn't buy more time.
    let stubborn = "function on_tick(game) for i = 1, 10 do pcall(function() while true do end end) end game:set('done', true) end";
    let (errors, _, state) = run(stubborn);
    assert!(!errors.is_empty() && state.store.is_empty(), "{errors:?} {state:?}");
    // Memory has a limit too.
    let hog = "function on_tick(game) local t = {} for i = 1, 90000 do t[i] = string.rep('x', 4096) .. i end end";
    let (errors, _, _) = run(hog);
    assert!(!errors.is_empty(), "a script holding hundreds of MB must fail");
    // A script that fails to load is an error when the mods are applied, naming the file.
    let mut config = base(9);
    let e = apply(&mut config, &[Mod::new("test").with_script("broken.luau", "function on_tick(game")]).unwrap_err();
    assert!(e.contains("mod test: broken.luau"), "{e}");
    let e = apply(&mut config, &[Mod::new("test").with_script("slow.luau", "while true do end")]).unwrap_err();
    assert!(e.contains("ran too long"), "{e}");
    let e = apply(&mut config, &[Mod::new("test").with_script("hook.luau", "on_tick = 5")]).unwrap_err();
    assert!(e.contains("on_tick must be a function"), "{e}");
}

#[test]
fn the_store_takes_plain_data_only() {
    let source = r#"
        function on_spawn(game, player)
            game:set("record", { name = "x", list = { 1, 2, { deep = true } }, n = 1.5 })
            game:set("empty", {})
            game:set("gone", 1)
            game:set("gone", nil)
            local back = game:get("record")
            game:set("back", { back.name, back.list[3].deep, #back.list, back.n })
            local ok = {}
            for i, bad in ipairs({
                function() game:set("f", print) end,
                function() game:set("nan", 0 / 0) end,
                function() game:set("mixed", { 1, 2, x = 3 }) end,
                function() game:set("holes", { [1] = 1, [3] = 3 }) end,
                function() game:set("keys", { [true] = 1 }) end,
                function() game:set("", 1) end,
                function() game:set("big", string.rep("x", 2000)) end,
                function() local t = {} t.t = t game:set("loop", t) end,
            }) do
                ok[i] = pcall(bad)
            end
            game:set("refused", ok)
        end
    "#;
    let mut w = World::new(with_script(10, source));
    w.add_player(Vec2::new(24.5, 24.5));
    w.step();
    assert!(w.script_errors().is_empty(), "{:?}", w.script_errors());
    assert_eq!(stored(&w, "back"), Some(&Value::List(vec![Value::Str("x".into()), Value::Bool(true), Value::Num(3.0), Value::Num(1.5)])));
    assert_eq!(stored(&w, "empty"), Some(&Value::List(Vec::new())));
    assert_eq!(stored(&w, "gone"), None);
    assert_eq!(stored(&w, "refused"), Some(&Value::List(vec![Value::Bool(false); 8])));
    for key in ["f", "nan", "mixed", "holes", "keys", "big", "loop"] {
        assert_eq!(stored(&w, key), None, "{key}");
    }
    // The store has a size limit: a mod can't grow a save without end.
    let grow = "function on_tick(game) for i = 1, 40 do game:set('k' .. game:tick() .. '_' .. i, string.rep('x', 1000)) end end";
    let mut w = World::new(with_script(10, grow));
    steps(&mut w, 10);
    let weight: usize = w.script_state().store["test"].iter().map(|(k, v)| k.len() + v.weight()).sum();
    assert!(weight <= zombye_sim::script::MAX_STORE && !w.script_errors().is_empty(), "{weight}");
}

/// Every hook is told what happened, with ids a script can use, and random numbers come from the
/// world: the same every run, and carried through a save.
#[test]
fn hooks_and_random_numbers() {
    let source = r#"
        local function count(game, key) game:set(key, (game:get(key) or 0) + 1) end
        function on_spawn(game, player) count(game, "spawns") game:set("spawned", player) end
        function on_craft(game, player, recipe) count(game, "crafts") game:set("recipe", recipe) end
        function on_use(game, player, item) count(game, "uses") game:set("used", item) end
        function on_hit(game, player, zombie, damage, killed) count(game, "hits") game:set("hit", { player, zombie > 0, damage >= 0, killed }) end
        function on_death(game, player) count(game, "deaths") end
        function on_tick(game)
            if game:tick() == 5 then
                local p = game:players()[1]
                game:set("info", { game:player(p).hp, game:player(p).max_hp, game:player(99) == nil, game:item("apple").category, game:item("nope") == nil })
                game:set("clock", { game:hour() >= 12, game:day(), game:seconds() == 0.25, game:daylight() > 0.5 })
            end
            if game:tick() < 50 then
                local rolls = game:get("rolls") or {}
                table.insert(rolls, game:random(6))
                table.insert(rolls, game:random(10, 20))
                game:set("rolls", rolls)
                game:set("unit", game:random())
            end
            if game:tick() == 300 then game:hurt(game:players()[1], 1000) end
        end
    "#;
    let config = with_script(11, source);
    let play = |w: &mut World, id: u32, from: u64, to: u64| {
        for t in from..to {
            if t == 10 {
                assert!(w.give_item(id, "apple", 1) && w.give_item(id, "t_shirt", 1));
                w.player_use(id, Place::Carried, 0).unwrap();
                let rip = w.config.recipes.iter().position(|r| r.id == "rip_t_shirt").expect("the t-shirt recipe") as u16;
                w.player_craft(id, rip).unwrap();
            }
            if t == 150 {
                // A zombie close by.
                let at = w.player(id).unwrap().state.pos;
                assert_eq!(w.spawn_zombies(1, at, 1.5), 1);
            }
            if (150..290).contains(&t) {
                // Keep facing it, close in, and punch.
                let to = w.zombies.pos[0] - w.player(id).unwrap().state.pos;
                let move_dir = if to.length() > 0.7 { to.normalize() } else { Vec2::ZERO };
                w.set_player_input(id, t as u32, PlayerInput { aim: true, facing: to.to_angle(), move_dir, ..Default::default() });
                if t % 12 == 0 {
                    w.player_attack(id);
                }
            }
            w.step();
        }
    };
    let mut w = World::new(config.clone());
    let id = w.add_player(Vec2::new(24.5, 24.5));
    w.set_dev(id, None, Some(true));
    play(&mut w, id, 0, 320);
    assert!(w.script_errors().is_empty(), "{:?}", w.script_errors());
    assert_eq!((num(&w, "spawns"), num(&w, "spawned")), (1.0, id as f64));
    assert_eq!((num(&w, "uses"), stored(&w, "used")), (1.0, Some(&Value::Str("apple".into()))));
    assert_eq!((num(&w, "crafts"), stored(&w, "recipe")), (1.0, Some(&Value::Str("rip_t_shirt".into()))));
    assert!(num(&w, "hits") >= 1.0);
    assert!(matches!(stored(&w, "hit"), Some(Value::List(l)) if l[0] == Value::Num(id as f64) && l[1] == Value::Bool(true) && l[2] == Value::Bool(true)));
    assert_eq!(num(&w, "deaths"), 1.0);
    assert!(w.player(id).unwrap().dead);
    assert_eq!(stored(&w, "info"), Some(&Value::List(vec![Value::Num(100.0), Value::Num(100.0), Value::Bool(true), Value::Str("food".into()), Value::Bool(true)])));
    assert_eq!(stored(&w, "clock"), Some(&Value::List(vec![Value::Bool(true), Value::Num(1.0), Value::Bool(true), Value::Bool(true)])));
    let Some(Value::List(rolls)) = stored(&w, "rolls").cloned() else { panic!() };
    assert_eq!(rolls.len(), 100);
    for pair in rolls.chunks(2) {
        let (Value::Num(d6), Value::Num(teen)) = (&pair[0], &pair[1]) else { panic!() };
        assert!((1.0..=6.0).contains(d6) && d6.fract() == 0.0 && (10.0..=20.0).contains(teen) && teen.fract() == 0.0, "{pair:?}");
    }
    assert!(rolls.iter().any(|r| r != &rolls[0]), "the rolls vary");
    assert!((0.0..1.0).contains(&num(&w, "unit")));

    // Again, saved and loaded part way through the rolls: the same rolls.
    let mut again = World::new(config.clone());
    let id = again.add_player(Vec2::new(24.5, 24.5));
    again.set_dev(id, None, Some(true));
    play(&mut again, id, 0, 25);
    let mut again = World::load(&again.save(), &config).unwrap();
    again.set_dev(id, None, Some(true));
    play(&mut again, id, 25, 320);
    assert_eq!(stored(&again, "rolls"), Some(&Value::List(rolls)));
    assert_eq!(again.state_hash(), w.state_hash());
}

/// What the world's functions do with what a script hands them: checked, clamped, never a panic.
#[test]
fn the_api_checks_what_it_is_given() {
    let source = r#"
        function on_spawn(game, p)
            local me = game:player(p)
            local r = {}
            r.noise = { game:noise(me.x, me.y, 30), game:noise(-5, 0, 30), game:noise(me.x, me.y, 0 / 0), game:noise(me.x, me.y, -1) }
            r.give = { game:give(p, "apple", 3), game:give(p, "no_such_item"), game:give(99, "apple"), game:count(p, "apple") }
            r.take = { game:take(p, "apple", 2), game:take(p, "apple", 10), game:count(p, "apple") }
            local id = game:spawn_item("apple", me.x, me.y, 2)
            local it = game:ground_item(id)
            r.ground = { it.item, it.count, game:remove_item(id), game:remove_item(id), game:ground_item(id) == nil, game:spawn_item("apple", 1e9, 0) == nil }
            r.needs = { game:set_need(p, "hunger", 0.5), game:set_need(p, "thirst", 7), game:set_need(p, "tiredness", 0 / 0), (pcall(game.set_need, game, p, "luck", 1)) }
            r.body = { game:hurt(p, 30), game:heal(p, 10), game:heal(p, -5), game:set_endurance(p, 0.25), game:tell(p, "hello\nthere"), game:tell(p, "") }
            r.zombies = { game:zombies_near(me.x, me.y, 30), game:spawn_zombies(me.x + 10, me.y, 500, 5), game:zombies_near(me.x, me.y, 30) }
            r.wrong = { (pcall(game.noise, game, "here")), (pcall(game.give, game, p)), (pcall(game.random, game, 0)) }
            game:set("r", r)
        end
    "#;
    let mut w = World::new(with_script(12, source));
    let id = w.add_player(Vec2::new(24.5, 24.5));
    w.step();
    assert!(w.script_errors().is_empty(), "{:?}", w.script_errors());
    let Some(Value::Map(r)) = stored(&w, "r") else { panic!() };
    let (t, f, n) = (Value::Bool(true), Value::Bool(false), Value::Num);
    assert_eq!(r["noise"], Value::List(vec![t.clone(), f.clone(), f.clone(), f.clone()]));
    assert_eq!(r["give"], Value::List(vec![t.clone(), f.clone(), f.clone(), n(3.0)]));
    assert_eq!(r["take"], Value::List(vec![n(2.0), n(1.0), n(0.0)]));
    assert_eq!(r["ground"], Value::List(vec![Value::Str("apple".into()), n(2.0), t.clone(), f.clone(), t.clone(), t.clone()]));
    assert_eq!(r["needs"], Value::List(vec![t.clone(), t.clone(), f.clone(), f.clone()]));
    assert_eq!(r["body"], Value::List(vec![f.clone(), t.clone(), f.clone(), t.clone(), t.clone(), f.clone()]));
    assert_eq!(r["zombies"], Value::List(vec![n(0.0), n(50.0), n(50.0)]));
    assert_eq!(r["wrong"], Value::List(vec![f.clone(), f.clone(), f.clone()]));
    let p = w.player(id).unwrap();
    assert_eq!((p.hp, p.needs.hunger, p.needs.thirst, p.state.endurance), (80.0, 0.5, 1.0, 0.25));
    assert!(w.item_notes.iter().any(|note| note.text == "hello there"));
}

#[test]
fn mods_add_data_as_the_game_has_it() {
    let whistle = r#"[(id: "whistle", name: "Whistle", category: Junk, weight_kg: 0.02, volume_l: 0.02, tags: ["reusable"])]"#;
    let recipe = r#"[(id: "carve_whistle", name: "Carve a whistle", secs: 5.0, inputs: [(item: "magazine", count: 1)], outputs: [(item: "whistle", count: 1)])]"#;
    let loot = r#"[(room: "bedroom", container: "any", entries: [(item: "whistle", weight: 50)])]"#;
    let blow = "function on_use(game, player, item) if item == 'whistle' then game:set('blown', (game:get('blown') or 0) + 1) end end";
    let m = Mod::new("test").with_items(whistle).with_recipes(recipe).with_loot(loot).with_script("whistle.luau", blow);
    let mut config = base(13);
    let (items, recipes) = (config.items.len(), config.recipes.len());
    apply(&mut config, &[m.clone()]).unwrap();
    assert_eq!((config.items.len(), config.recipes.len()), (items + 1, recipes + 1));
    let table = config.loot.tables.iter().find(|t| t.room == "bedroom" && t.container == "any").unwrap();
    assert!(table.entries.iter().any(|e| e.item == "whistle"));
    // A `reusable` thing is used and kept.
    let mut w = World::new(config);
    let id = w.add_player(Vec2::new(24.5, 24.5));
    assert!(w.give_item(id, "whistle", 1));
    for _ in 0..2 {
        w.player_use(id, Place::Carried, 0).unwrap();
        steps(&mut w, 30);
    }
    assert_eq!((num(&w, "blown"), w.script_count(id, "whistle")), (2.0, 1));

    // Mistakes say which mod and file.
    let bad = |m: Mod| apply(&mut base(13), &[m]).unwrap_err();
    assert!(bad(Mod::new("test").with_items("[(id: \"Bad Id\")]")).starts_with("mod test: items.ron"));
    assert!(bad(Mod::new("test").with_recipes(recipe)).contains("mod test: recipes.ron"), "a recipe for an item nobody has");
    assert!(bad(Mod::new("test").with_loot(r#"[(room: "moon", container: "any", entries: [])]"#)).contains("no loot table (moon, any)"));
    assert!(bad(Mod::new("test").with_loot(r#"[(room: "bedroom", container: "any", entries: [(item: "unobtainium")])]"#)).contains("mod test: loot.ron"));
    assert!(bad(Mod::new("Bad")).contains("mod id"));
    assert!(apply(&mut base(13), &[m.clone(), m]).unwrap_err().contains("listed twice"));
}

#[test]
fn a_mod_folder_is_read_and_checked() {
    let m = Mod::load(&sample_dir()).unwrap();
    assert_eq!((m.manifest.id.as_str(), m.scripts.len(), m.items.is_some()), ("alarm_clock", 1, true));
    assert!(zombye_mods::describe(&[m]).contains("alarm_clock 1.0 (1 script)"));
    let dir = std::env::temp_dir().join(format!("zombye-mod-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let e = Mod::load(&dir).unwrap_err();
    assert!(e.contains("mod.ron"), "{e}");
    // A script outside the mod's folder, or not a .luau file, is refused.
    for bad in ["../other.luau", "/etc/passwd", "notes.txt", ".hidden.luau"] {
        std::fs::write(dir.join("mod.ron"), format!("(id: \"m\", name: \"M\", scripts: [{bad:?}])")).unwrap();
        let e = Mod::load(&dir).unwrap_err();
        assert!(e.contains("must be a .luau file in the mod's folder"), "{bad}: {e}");
    }
    std::fs::write(dir.join("mod.ron"), "(id: \"m\", name: \"M\", scripts: [\"missing.luau\"])").unwrap();
    assert!(Mod::load(&dir).unwrap_err().contains("missing.luau"));
    std::fs::write(dir.join("mod.ron"), "(id: \"m\", name: \"M\", colour: \"red\")").unwrap();
    assert!(Mod::load(&dir).is_err(), "unknown fields are refused");
    std::fs::remove_dir_all(&dir).unwrap();
}
