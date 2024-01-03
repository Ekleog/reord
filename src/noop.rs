#![allow(unused_variables)]

use crate::Config;
use std::future::Future;

/// Start a test
///
/// Note that this relies on global variables for convenience, and thus should only ever be used with
/// `cargo-nextest`.
#[inline]
pub async fn init_test(config: Config) {}

/// Add a task to the `reord` framework
///
/// This should be used around all the futures spawned by the test
#[inline]
pub async fn new_task<T>(f: impl Future<Output = T>) -> T {
    f.await
}

/// Start the test once `tasks` tasks are ready for execution
///
/// This should be called after at least `tasks` tasks have been spawned on the executor,
/// wrapped by `new_task`.
///
/// This will start executing the tasks in a random but reproducible order, and then return
/// as soon as the `tasks` tasks have started executing.
#[inline]
pub async fn start(tasks: usize) {}

/// Execution order randomization point
///
/// Reaching this point makes `reord` able to switch the execution to another thread.
#[inline]
pub async fn point() {}

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
pub struct Lock(());

impl Lock {
    /// Take a lock with a given name
    #[inline]
    pub async fn take_named(name: String) -> Lock {
        Lock(())
    }

    /// Take a lock at a given address
    #[inline]
    pub async fn take_addressed(address: usize) -> Lock {
        Lock(())
    }
}

impl Drop for Lock {
    #[inline]
    fn drop(&mut self) {}
}
