//! Making a world on several threads (#19): the per-tile passes of world generation (the walk
//! grid, the biomes, the first pass of planting the wild ground) split the map into bands of rows,
//! one a thread, with [`std::thread::scope`]. Every tile's answer depends only on the map as it
//! was before the pass and on hashes of the seed and the tile, never on a random stream or on
//! another band, so the result is the same whatever the thread count (the tests make the same
//! world on 1 and on 4 and compare the map, its hash, the state hash, the walk grid, the nav graph
//! and the save). Passes that draw from the generator's stream in turn (the town's layout) or
//! that read what they wrote a tile before (spacing trees) stay on one thread.
//!
//! How many: [`threads`], by default the cores up to 8. A program sets another for itself with
//! [`set_threads`] (the server and client read `ZOMBYE_GEN_THREADS`); a test makes a world on a
//! given number with [`with_threads`], which holds only on its own thread.

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Threads at most.
pub const MAX_THREADS: usize = 64;

static THREADS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static OVERRIDE: Cell<usize> = const { Cell::new(0) };
}

/// The cores there are, at most 8.
pub fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get()).min(8)
}

/// Make worlds on `n` threads from now on, in this program (1: on the calling thread only).
pub fn set_threads(n: usize) {
    THREADS.store(n.clamp(1, MAX_THREADS), Ordering::Relaxed);
}

/// The threads a world is made on, here and now.
pub fn threads() -> usize {
    match OVERRIDE.get() {
        0 => match THREADS.load(Ordering::Relaxed) {
            0 => default_threads(),
            n => n,
        },
        n => n,
    }
}

/// Run `f` with worlds made on `n` threads, on this thread only (tests).
pub fn with_threads<R>(n: usize, f: impl FnOnce() -> R) -> R {
    let was = OVERRIDE.replace(n.clamp(1, MAX_THREADS));
    let out = f();
    OVERRIDE.set(was);
    out
}

/// Fill `out`, `width` items a row, by calling `f(y, row)` for every row, the rows shared out in
/// bands over [`threads`] threads. `f` sees only its own row, so the order rows are done in can't
/// change what is written.
pub fn rows<T: Send>(out: &mut [T], width: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    if width == 0 || out.is_empty() {
        return;
    }
    let rows = out.len() / width;
    let n = threads().min(rows).max(1);
    if n == 1 {
        for (y, row) in out.chunks_mut(width).enumerate() {
            f(y, row);
        }
        return;
    }
    let band = rows.div_ceil(n);
    let f = &f;
    std::thread::scope(|s| {
        for (b, chunk) in out.chunks_mut(band * width).enumerate() {
            s.spawn(move || {
                for (k, row) in chunk.chunks_mut(width).enumerate() {
                    f(b * band + k, row);
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_the_same_on_any_number_of_threads() {
        let fill = |n: usize| {
            with_threads(n, || {
                let mut v = vec![0u64; 37 * 101];
                rows(&mut v, 37, |y, row| {
                    for (x, c) in row.iter_mut().enumerate() {
                        *c = (y as u64) << 32 | x as u64;
                    }
                });
                v
            })
        };
        let one = fill(1);
        for n in [2, 3, 8, 64] {
            assert_eq!(fill(n), one, "{n} threads");
        }
        assert_eq!(with_threads(3, threads), 3);
    }
}
