#![doc = include_str!("../README.md")]

use std::time::Duration;

#[cfg(test)]
mod real;

#[cfg(test)]
pub use real::*;

#[allow(dead_code)] // dead code when in cfg(test)
mod noop;

#[cfg(not(test))]
pub use noop::*;

/// Configuration for a `reord`-based test
#[derive(Debug)]
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
