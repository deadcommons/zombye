//! The benchmark as a process (#29): `zombye-server --benchmark ...` lists the scenarios, plays
//! one with a baseline, serves one over MCP on stdio and writes each run's result.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("zombye-benchmark-it-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Scratch(dir)
}

fn server() -> Command {
    Command::new(env!("CARGO_BIN_EXE_zombye-server"))
}

fn results(dir: &Path) -> Vec<Value> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir).map(|d| d.map(|e| e.unwrap().path()).collect()).unwrap_or_default();
    paths.sort();
    paths.iter().map(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()).collect()
}

#[test]
fn the_scenarios_are_listed_and_a_baseline_plays_one() {
    let out = server().args(["--benchmark", "list"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    for id in ["first_night: ", "scavenger: ", "last_stand: ", "siege: "] {
        assert!(text.contains(id), "{text}");
    }

    // The same baseline twice: the same result but for the file's name.
    let scratch = scratch("baseline");
    let dir = scratch.0.to_string_lossy().into_owned();
    for _ in 0..2 {
        let out = server().args(["--benchmark", "last_stand", "--baseline", "idle", "--results", &dir]).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stdout).starts_with("last_stand [baseline:idle] "), "{}", String::from_utf8_lossy(&out.stdout));
    }
    let r = results(&scratch.0);
    assert_eq!(r.len(), 2);
    assert_eq!(r[0], r[1]);
    assert_eq!((r[0]["format"].as_str(), r[0]["scenario"].as_str(), r[0]["agent"].as_str()), (Some("zombye-benchmark/1"), Some("last_stand"), Some("baseline:idle")));
    let table = server().args(["--benchmark", "table", "--results", &dir]).output().unwrap();
    let table = String::from_utf8_lossy(&table.stdout).into_owned();
    assert!(table.contains("last_stand (fingerprint ") && table.matches("baseline:idle").count() == 2, "{table}");

    // What can't be run says why, with the scenarios there are.
    let out = server().args(["--benchmark", "nonesuch", "--baseline", "idle"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no scenario \"nonesuch\" (there are: first_night"));
    let out = server().args(["--benchmark", "siege"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("needs --mcp"));
}

/// An agent on stdio: the scenario in the instructions, the benchmark tool, a run that ends, the
/// result on disk; and one that leaves early, written as unfinished.
#[test]
fn an_agent_plays_a_scenario_over_stdio() {
    let scratch = scratch("stdio");
    let dir = scratch.0.to_string_lossy().into_owned();
    let play = |calls: &[Value]| -> Vec<Value> {
        let mut child = server()
            .args(["--mcp", "--benchmark", "last_stand", "--agent", "stdio-test", "--results", &dir])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut answers = Vec::new();
        for (i, (method, params)) in [("initialize", json!({"protocolVersion": "2025-06-18"})), ("tools/list", json!({}))]
            .into_iter()
            .chain(calls.iter().map(|c| ("tools/call", c.clone())))
            .enumerate()
        {
            writeln!(stdin, "{}", json!({"jsonrpc": "2.0", "id": i, "method": method, "params": params})).unwrap();
            let mut line = String::new();
            stdout.read_line(&mut line).unwrap();
            answers.push(serde_json::from_str::<Value>(&line).unwrap_or_else(|e| panic!("{e}: {line}")));
        }
        drop(stdin);
        assert!(child.wait().unwrap().success());
        answers
    };

    // Standing in the middle of sixty zombies for a game hour doesn't end well.
    let answers = play(&[
        json!({"name": "benchmark", "arguments": {}}),
        json!({"name": "step_ticks", "arguments": {"n": 3000}}),
        json!({"name": "step_ticks", "arguments": {"n": 20}}),
    ]);
    assert!(answers[0]["result"]["instructions"].as_str().unwrap().contains("BENCHMARK RUN: scenario \"last_stand\""));
    let tools: Vec<&str> = answers[1]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(tools.contains(&"benchmark") && !tools.contains(&"skip_time") && !tools.contains(&"save"), "{tools:?}");
    let report: Value = serde_json::from_str(answers[2]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!((report["scenario"].as_str(), report["status"].as_str(), report["hours_left"].as_f64()), (Some("last_stand"), Some("running"), Some(1.0)));
    let texts = |a: &Value| a["result"]["content"].as_array().unwrap().iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n");
    assert!(texts(&answers[3]).contains("BENCHMARK OVER: died after"), "{}", texts(&answers[3]));
    assert!(answers[4]["result"]["isError"] == true && texts(&answers[4]).contains("the benchmark run is over"), "{}", answers[4]);
    let r = results(&scratch.0);
    assert_eq!(r.len(), 1);
    assert_eq!((r[0]["agent"].as_str(), r[0]["outcome"].as_str(), r[0]["actions"].as_u64()), (Some("stdio-test"), Some("died"), Some(1)));
    // The baseline that does the same thing got the same world.
    let idle = server().args(["--benchmark", "last_stand", "--baseline", "idle", "--results", &dir]).output().unwrap();
    assert!(idle.status.success());
    let both = results(&scratch.0);
    let baseline = both.iter().find(|r| r["agent"] == "baseline:idle").unwrap();
    assert_eq!((baseline["state_hash"].clone(), baseline["ticks"].clone(), baseline["score"].clone()), (r[0]["state_hash"].clone(), r[0]["ticks"].clone(), r[0]["score"].clone()));

    // Leaving after a second of game time: unfinished, with what it had.
    play(&[json!({"name": "step_ticks", "arguments": {"n": 20}})]);
    let unfinished: Vec<Value> = results(&scratch.0).into_iter().filter(|r| r["outcome"] == "unfinished").collect();
    assert_eq!(unfinished.len(), 1);
    assert_eq!((unfinished[0]["ticks"].as_u64(), unfinished[0]["agent"].as_str()), (Some(20), Some("stdio-test")));
}
