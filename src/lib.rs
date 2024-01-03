use std::time::Duration;

#[cfg(test)]
mod real;

#[cfg(test)]
pub use real::*;

#[allow(dead_code)] // dead code when in cfg(test)
mod noop;

#[cfg(not(test))]
pub use noop::*;

#[derive(Debug)]
pub struct Config {
    pub seed: [u8; 32],
    pub check_named_locks_work_for: Option<Duration>,
    pub check_addressed_locks_work_for: Option<Duration>,
}
