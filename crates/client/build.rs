//! The Windows executable's icon (issue #76). The game's icon is a trade mark and is never in this
//! repo, so it is embedded only when a build names one: `ZOMBYE_WINDOWS_ICON=PATH.ico`, which
//! `tools/release/package.sh windows` sets from the art repo's `release-brand/zombye.ico`. The icon
//! is compiled into a resource object with `windres` (`$WINDRES`, else MinGW's
//! `x86_64-w64-mingw32-windres`, else `windres`) and linked into `zombye.exe`. For any other target,
//! or with no icon named, nothing happens.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=ZOMBYE_WINDOWS_ICON");
    println!("cargo:rerun-if-env-changed=WINDRES");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let Some(icon) = std::env::var_os("ZOMBYE_WINDOWS_ICON").map(PathBuf::from).filter(|p| !p.as_os_str().is_empty()) else {
        return;
    };
    if target_os != "windows" || target_env != "gnu" {
        println!("cargo:warning=ZOMBYE_WINDOWS_ICON is only used for the windows-gnu target");
        return;
    }
    let icon = std::fs::canonicalize(&icon).unwrap_or_else(|e| panic!("ZOMBYE_WINDOWS_ICON {}: {e}", icon.display()));
    println!("cargo:rerun-if-changed={}", icon.display());
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let rc = out.join("zombye-icon.rc");
    // windres reads the path in a quoted string: forward slashes, quotes escaped.
    let path = icon.display().to_string().replace('\\', "/").replace('"', "\\\"");
    std::fs::write(&rc, format!("1 ICON \"{path}\"\n")).expect("write the icon's .rc");
    let obj = out.join("zombye-icon.o");
    let windres = std::env::var("WINDRES").ok().filter(|w| !w.is_empty()).unwrap_or_else(|| {
        let mingw = "x86_64-w64-mingw32-windres";
        if Command::new(mingw).arg("--version").output().is_ok() { mingw.to_string() } else { "windres".to_string() }
    });
    let status = Command::new(&windres)
        .args(["--input-format=rc", "--output-format=coff", "-i"])
        .arg(&rc)
        .arg("-o")
        .arg(&obj)
        .status()
        .unwrap_or_else(|e| panic!("can't run {windres} for the Windows icon: {e}"));
    assert!(status.success(), "{windres} failed on {}", rc.display());
    println!("cargo:rustc-link-arg-bin=zombye={}", obj.display());
}
