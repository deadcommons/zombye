//! The build's version for servers and clients to tell each other (DCM-70): the package version
//! and, in a git checkout, the commit it was built from (`0.1.0 (abc1234)`), so a join refused for
//! other game data can say which build each side is. Without git (a source tarball, the Docker
//! build's context) it is the package version alone.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

fn main() {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let build = match git(&["rev-parse", "--short=7", "HEAD"]) {
        Some(commit) => {
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some();
            format!("{version} ({commit}{})", if dirty { ", changed" } else { "" })
        }
        None => version,
    };
    println!("cargo:rustc-env=ZOMBYE_BUILD={build}");
    // Built again when the commit moves (HEAD, the branch it names, and packed refs).
    if let Some(dir) = git(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        if let Some(head) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={dir}/{head}");
        }
        println!("cargo:rerun-if-changed={dir}/packed-refs");
    }
    if let Some(common) = git(&["rev-parse", "--git-common-dir"]) {
        println!("cargo:rerun-if-changed={common}/packed-refs");
        if let Some(head) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={common}/{head}");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
