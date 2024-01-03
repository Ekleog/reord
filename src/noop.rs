use crate::Config;
use std::future::Future;

#[inline]
pub async fn init_test(_: Config) {}

#[inline]
pub async fn new_task<T>(f: impl Future<Output = T>) -> T {
    f.await
}

#[inline]
pub async fn start() {}

#[inline]
pub async fn point() {}

/// Note: this should be dropped as soon as the underlying lock is released, and a `reord::point().await`
/// should be inserted as soon as possible after that. This is required especially for lock checking,
/// as lock checks need to have two tasks allowed to run at the same time. Similarly, just after the lock
/// is acquired, you should have a `reord::point().await`.
#[derive(Debug)]
pub struct Lock(());

impl Lock {
    #[inline]
    pub async fn take_named(_: String) -> Lock {
        Lock(())
    }
    #[inline]
    pub async fn take_addressed(_: usize) -> Lock {
        Lock(())
    }
}

impl Drop for Lock {
    #[inline]
    fn drop(&mut self) {}
}
