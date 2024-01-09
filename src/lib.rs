#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

use std::{future::Future, time::Duration};

#[cfg(any(test, feature = "test"))]
mod real;

#[cfg(all(test, not(feature = "test")))]
const _: () = panic!("Trying to test `reord` without its `test` feature");

// TODO: implement RwLocks
// TODO: make the seed just be a u128

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

/// Start a test
///
/// Note that this relies on global variables for convenience, and thus should only ever be used with
/// `cargo-nextest`.
#[inline]
#[allow(unused_variables)]
pub async fn init_test(config: Config) {
    #[cfg(feature = "test")]
    real::init_test(config).await
}

/// Add a task to the `reord` framework
///
/// This should be used around all the futures spawned by the test
#[inline]
pub async fn new_task<T>(f: impl Future<Output = T>) -> T {
    #[cfg(feature = "test")]
    let res = real::new_task(f).await;
    #[cfg(not(feature = "test"))]
    let res = f.await;
    res
}

/// Start the test once `tasks` tasks are ready for execution
///
/// This should be called after at least `tasks` tasks have been spawned on the executor,
/// wrapped by `new_task`.
///
/// This will start executing the tasks in a random but reproducible order, and then return
/// as soon as the `tasks` tasks have started executing.
///
/// This returns a `JoinHandle`, that you should join if you want to catch panics related to
/// lock handling.
#[inline]
#[allow(unused_variables)]
pub async fn start(tasks: usize) -> tokio::task::JoinHandle<()> {
    #[cfg(not(feature = "test"))]
    panic!("Trying to start a `reord` test, but the `test` feature is not set");
    #[cfg(feature = "test")]
    real::start(tasks).await
}

/// Execution order randomization point
///
/// Reaching this point makes `reord` able to switch the execution to another thread.
#[inline]
pub async fn point() {
    #[cfg(feature = "test")]
    real::point().await
}

/// Lock information
///
/// This can be either a user-defined name, or an address. When using `Addressed`, you should usually take
/// the address of the `Mutex`, `RwLock` or equivalent, and cast it to `usize`.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum LockInfo {
    /// A user-defined name
    Named(String),

    /// An address, usually the address of a `Mutex` or similar cast to `usize`
    Addressed(usize),
}

/// Lock handling
///
/// This records, for `reord`, which locks are currently taken by the program. The lock should be acquired
/// as close as possible before the real lock acquiring, and released as soon as possible after the real
/// lock release.
///
/// In addition, there should be a `reord::point().await` just after the real lock managed to be acquired,
/// and one ideally just after the real lock was released, though this may be harder due to early returns
/// and the current absence of `async Drop`.
///
/// If too long passes with `reord` unaware of the state of your locks, you could end up with
/// non-reproducible behavior, due to two execution threads actually running in parallel on your executor.
#[derive(Debug)]
pub struct Lock {
    #[cfg(feature = "test")]
    _data: real::Lock,
    #[cfg(not(feature = "test"))]
    _unused: (),
}

impl Lock {
    /// Take a lock with a given name
    #[inline]
    #[allow(unused_variables)]
    pub async fn take_named(name: String) -> Lock {
        #[cfg(feature = "test")]
        let res = Lock {
            _data: real::Lock::take_named(name).await,
        };
        #[cfg(not(feature = "test"))]
        let res = Lock { _unused: () };
        res
    }

    /// Take a lock at a given address
    #[inline]
    #[allow(unused_variables)]
    pub async fn take_addressed(address: usize) -> Lock {
        #[cfg(feature = "test")]
        let res = Lock {
            _data: real::Lock::take_addressed(address).await,
        };
        #[cfg(not(feature = "test"))]
        let res = Lock { _unused: () };
        res
    }

    /// Take multiple locks, atomically
    ///
    /// If you try to take multiple `reord::Lock`s one after the other before locking them atomically, then
    /// `reord` will think that your first lock failed to actually lock. In order to avoid this, you should
    /// use `reord::Lock::take_atomic` to take multiple locks.
    #[inline]
    #[allow(unused_variables)]
    pub async fn take_atomic(l: Vec<LockInfo>) -> Lock {
        #[cfg(feature = "test")]
        let res = Lock {
            _data: real::Lock::take_atomic(l).await,
        };
        #[cfg(not(feature = "test"))]
        let res = Lock { _unused: () };
        res
    }
}

impl Drop for Lock {
    #[inline]
    fn drop(&mut self) {}
}

#[cfg(test)]
mod tests;
