#![doc = include_str!("../README.md")]

use std::time::Duration;

#[cfg(any(test, feature = "test"))]
mod real;

#[cfg(any(test, feature = "test"))]
pub use real::*;

#[allow(dead_code)] // dead code when in cfg(test)
mod noop;

#[cfg(not(any(test, feature = "test")))]
pub use noop::*;

/// Configuration for a `reord`-based test
#[derive(Debug)]
#[non_exhaustive]
pub struct Config {
    /// The random seed used to choose which task to run
    ///
    /// Changing this seed will give other task orderings, but leaving it the same should (if
    /// lock tracking is correctly implemented by the application using `Lock`) keep execution
    /// reproducible.
    pub seed: [u8; 32],

    /// If set to `Some`, will allow two tasks to voluntarily collide on a named lock to validate
    /// that locking is implemented correctly. It will then wait for the time indicated by this
    /// setting, and if the next `reord::point` has not been reached by then, assume the locking
    /// worked properly and continue testing.
    pub check_named_locks_work_for: Option<Duration>,

    /// If set to `Some`, will allow two tasks to voluntarily collide on an addressed lock to
    /// validate that locking is implemented correctly. It will then wait for the time indicated
    /// by this setting, and if the next `reord::point` has not been reached by then, assume the
    /// locking worked properly and continue testing.
    pub check_addressed_locks_work_for: Option<Duration>,
}

impl Config {
    /// Generate a configuration with the default parameters and a random seed
    pub fn with_random_seed() -> Config {
        use rand::Rng;
        Config {
            seed: rand::thread_rng().gen(),
            check_addressed_locks_work_for: None,
            check_named_locks_work_for: None,
        }
    }

    /// Generate a configuration with the default parameters from a given seed
    pub fn from_seed(seed: [u8; 32]) -> Config {
        Config {
            seed,
            check_addressed_locks_work_for: None,
            check_named_locks_work_for: None,
        }
    }
}

#[cfg(all(test, not(feature = "test")))]
const _: () = panic!("Trying to test `reord` without its `test` feature");

#[cfg(test)]
mod tests;
