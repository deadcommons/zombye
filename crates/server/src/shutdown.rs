//! Ctrl-C (and SIGTERM) as a request to stop, so the server can save before it exits. Without a
//! dependency: a tiny handler that only sets a flag, which the main loop polls.

use std::sync::atomic::{AtomicBool, Ordering};

static REQUESTED: AtomicBool = AtomicBool::new(false);

/// Has Ctrl-C (or SIGTERM) been pressed (sent) since [`install`]?
pub fn requested() -> bool {
    REQUESTED.load(Ordering::SeqCst)
}

#[cfg(unix)]
pub fn install() {
    unsafe extern "C" {
        fn signal(signum: i32, handler: extern "C" fn(i32)) -> usize;
    }
    extern "C" fn on_signal(_: i32) {
        // Only an atomic store: safe in a signal handler.
        REQUESTED.store(true, Ordering::SeqCst);
    }
    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;
    // SAFETY: `signal` is the C library's; the handler only touches an atomic.
    unsafe {
        signal(SIGINT, on_signal);
        signal(SIGTERM, on_signal);
    }
}

#[cfg(windows)]
pub fn install() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
    }
    unsafe extern "system" fn on_ctrl(_: u32) -> i32 {
        REQUESTED.store(true, Ordering::SeqCst);
        1
    }
    // SAFETY: registers a handler that only touches an atomic.
    unsafe {
        SetConsoleCtrlHandler(Some(on_ctrl), 1);
    }
}

#[cfg(not(any(unix, windows)))]
pub fn install() {}
