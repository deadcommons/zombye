//! Ctrl-C, SIGTERM and SIGHUP (closing the terminal) as a request to stop, so the server and the
//! game client can save before they exit. Without a dependency: a tiny handler that only sets a
//! flag, which the main loop polls ([`requested`]). A second signal while the first is still being
//! handled (a long `step_ticks` call, say) stops at once, unsaved. A signal that was ignored when the
//! program started (`nohup`, a background job of a script) stays ignored.
//!
//! On Windows the console's Ctrl-C and Ctrl-Break work the same way; closing the console window,
//! logging off or shutting down end the process as soon as the handler returns, so there the
//! handler waits (up to 4.5 s of the 5 s Windows allows) for the program to call [`finished`].

use std::sync::atomic::{AtomicBool, Ordering};

static REQUESTED: AtomicBool = AtomicBool::new(false);
/// Set once the program has saved on its way out (see [`finished`]).
#[cfg_attr(not(windows), allow(dead_code))]
static FINISHED: AtomicBool = AtomicBool::new(false);

/// Has Ctrl-C (or SIGTERM, or SIGHUP) arrived since [`install`]?
pub fn requested() -> bool {
    REQUESTED.load(Ordering::SeqCst)
}

/// The program has done what it does before exiting (saved). Lets a Windows console close go on.
pub fn finished() {
    FINISHED.store(true, Ordering::SeqCst);
}

#[cfg(unix)]
pub fn install() {
    unsafe extern "C" {
        // `sighandler_t signal(int, sighandler_t)`: handlers are passed and returned as addresses.
        fn signal(signum: i32, handler: usize) -> usize;
        fn _exit(status: i32) -> !;
    }
    extern "C" fn on_signal(_: i32) {
        // Only an atomic swap and `_exit`: both safe in a signal handler.
        if REQUESTED.swap(true, Ordering::SeqCst) {
            // SAFETY: `_exit` is async-signal-safe.
            unsafe { _exit(130) }
        }
    }
    const SIGHUP: i32 = 1;
    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;
    const SIG_IGN: usize = 1;
    const SIG_ERR: usize = usize::MAX;
    for sig in [SIGINT, SIGTERM, SIGHUP] {
        // SAFETY: `signal` is the C library's; the handler only touches an atomic (or exits).
        unsafe {
            let old = signal(sig, on_signal as extern "C" fn(i32) as usize);
            if old == SIG_IGN {
                signal(sig, SIG_IGN);
            } else if old == SIG_ERR {
                eprintln!("couldn't install a handler for signal {sig}: it will stop the program without saving");
            }
        }
    }
}

#[cfg(windows)]
pub fn install() {
    use std::time::{Duration, Instant};

    // `BOOL WINAPI HandlerRoutine(DWORD dwCtrlType)` and `SetConsoleCtrlHandler` (kernel32).
    type HandlerRoutine = unsafe extern "system" fn(u32) -> i32;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetConsoleCtrlHandler(handler: Option<HandlerRoutine>, add: i32) -> i32;
    }
    const CTRL_C_EVENT: u32 = 0;
    const CTRL_BREAK_EVENT: u32 = 1;
    /// Windows gives a process about 5 s after a close, logoff or shutdown event.
    const CLOSE_GRACE: Duration = Duration::from_millis(4500);

    // Windows runs this on a thread of its own, so it may wait.
    unsafe extern "system" fn on_ctrl(kind: u32) -> i32 {
        let again = REQUESTED.swap(true, Ordering::SeqCst);
        match kind {
            // Handled (TRUE): the main loop sees the flag, saves and exits. A second press is
            // left to the default handler (FALSE), which ends the process at once.
            CTRL_C_EVENT | CTRL_BREAK_EVENT => i32::from(!again),
            // CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT: the process ends as soon
            // as this returns, whatever it returns, so wait for the save first.
            _ => {
                let start = Instant::now();
                while !FINISHED.load(Ordering::SeqCst) && start.elapsed() < CLOSE_GRACE {
                    std::thread::sleep(Duration::from_millis(20));
                }
                1
            }
        }
    }
    // SAFETY: registers a handler that only touches atomics and sleeps.
    if unsafe { SetConsoleCtrlHandler(Some(on_ctrl), 1) } == 0 {
        eprintln!("couldn't install the Ctrl-C handler: Ctrl-C will stop the program without saving");
    }
}

#[cfg(not(any(unix, windows)))]
pub fn install() {}

#[cfg(test)]
mod tests {
    /// A real SIGTERM, sent to this test process, only sets the flag. (Once: a second one would
    /// end the process, as a second Ctrl-C should.)
    #[cfg(unix)]
    #[test]
    fn a_signal_only_asks_to_stop() {
        unsafe extern "C" {
            fn raise(sig: i32) -> i32;
        }
        super::install();
        assert!(!super::requested());
        // SAFETY: raising a signal whose handler only sets an atomic.
        assert_eq!(unsafe { raise(15) }, 0);
        assert!(super::requested(), "SIGTERM is a request to stop, not a kill");
    }
}
