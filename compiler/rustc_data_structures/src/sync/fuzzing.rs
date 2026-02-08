//! This module implements support functions for fuzzing parallel operations in the `parallel` module.
//! This can be used via `-Z thread 1 -Z parallel-fuzz-seed <seed>` to find bugs caused by executing
//! parallel sections out of order.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use rand::rngs::SmallRng;
use rand::{Rng, RngExt, SeedableRng, rng};

static ENABLED: AtomicBool = AtomicBool::new(false);
static SEED: Mutex<Option<u64>> = Mutex::new(None);

thread_local! {
    /// Thread local random number generator. We ensure that it's only accessed after fuzzing
    /// is enabled so we can always get the seed.
    static RNG: RefCell<SmallRng> = RefCell::new(SmallRng::seed_from_u64(SEED.lock().expect("available seed")));
}

fn with_rng<R>(f: impl FnOnce(&mut SmallRng) -> R) -> R {
    RNG.with(|cell| f(&mut *cell.borrow_mut()))
}

/// Enables fuzzing on parallel sections after this call.
/// All threads will use the seed if provided, otherwise a random seed is used.
pub fn enable_fuzzing(seed: Option<u64>) {
    *SEED.lock() = Some(seed.unwrap_or_else(|| SmallRng::from_rng(&mut rng()).next_u64()));
    ENABLED.store(true, Ordering::Release);
}

/// This prints the puzzing seed if fuzzing enabled. Used in the panic handler.
pub fn print_fuzzing_seed() {
    if is_fuzzing() {
        let seed = SEED.try_lock_for(Duration::from_secs(10)).map(|seed| *seed);
        eprintln!(
            "Fuzzing enabled with seed: {:?}",
            if let Some(seed) = seed {
                if let Some(seed) = seed { format!("{}", seed) } else { "<not set>".to_owned() }
            } else {
                "<locked>".to_owned()
            }
        );
    }
}

#[inline]
pub(super) fn is_fuzzing() -> bool {
    ENABLED.load(Ordering::Acquire)
}

/// Gets a random bool. Always false when fuzzing is not enabled.
#[inline]
pub(super) fn coin_flip() -> bool {
    if !is_fuzzing() {
        return false;
    }
    with_rng(|rng| rng.random())
}

pub(super) fn shuffle_slice<T>(v: &mut [T]) {
    if !is_fuzzing() {
        return;
    }
    let len = v.len();
    if len <= 1 {
        return;
    }
    with_rng(|rng| {
        for i in (1..len).rev() {
            let j = rng.random_range(0..=i);
            v.swap(i, j);
        }
    })
}
