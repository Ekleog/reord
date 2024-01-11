use crate::{Config, LockInfo};
use futures::FutureExt;
use rand::{seq::SliceRandom, SeedableRng};
use std::{
    collections::HashSet,
    future::Future,
    sync::{Mutex, RwLock},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};

impl Config {
    fn can_resume(
        &self,
        s: &StopPoint,
        locks: &HashSet<LockInfo>,
        has_blocked_task_waiting: bool,
    ) -> bool {
        for l in s.locks_about_to_be_acquired.iter() {
            match l {
                l if !locks.contains(l) => (),
                _ if has_blocked_task_waiting => return false,
                LockInfo::Addressed(_) if self.check_addressed_locks_work_for.is_some() => (),
                LockInfo::Named(_) if self.check_named_locks_work_for.is_some() => (),
                _ => return false,
            }
        }
        true
    }

    fn lock_check_time(&self, l: &[LockInfo], locks: &HashSet<LockInfo>) -> Option<Duration> {
        let mut locks_addressed = false;
        let mut locks_named = false;
        for l in l {
            match l {
                l if !locks.contains(l) => (),
                LockInfo::Addressed(_) => locks_addressed = true,
                LockInfo::Named(_) => locks_named = true,
            }
        }
        match (locks_addressed, locks_named) {
            (false, false) => None,
            (true, false) => Some(self.check_addressed_locks_work_for.unwrap()),
            (false, true) => Some(self.check_named_locks_work_for.unwrap()),
            (true, true) => Some(std::cmp::max(
                self.check_addressed_locks_work_for.unwrap(),
                self.check_named_locks_work_for.unwrap(),
            )),
        }
    }
}

#[derive(Debug)]
struct StopPoint {
    resume: oneshot::Sender<()>,
    locks_about_to_be_acquired: Vec<LockInfo>,
}

impl StopPoint {
    fn without_lock(resume: oneshot::Sender<()>) -> StopPoint {
        StopPoint {
            resume,
            locks_about_to_be_acquired: Vec::new(),
        }
    }
}

#[derive(Debug)]
enum Message {
    NewTask(StopPoint),
    Start,
    Stop(StopPoint),
    Unlock(LockInfo),
    TaskEnd,
}

static SENDER: RwLock<Option<mpsc::UnboundedSender<Message>>> = RwLock::new(None);
static OVERSEER: Mutex<Option<(Config, mpsc::UnboundedReceiver<Message>)>> = Mutex::new(None);

pub async fn init_test(cfg: Config) {
    eprintln!("Running `reord` test with random seed {:?}", cfg.seed);

    let (s, r) = mpsc::unbounded_channel();
    if let Some(s) = SENDER.write().unwrap().replace(s) {
        if !s.send(Message::Start).is_err() {
            panic!("Initializing a new test while the old test was still running! Note that `reord` is only designed to work with `cargo-nextest`.");
        }
    }

    assert!(OVERSEER.lock().unwrap().replace((cfg, r)).is_none());
}

pub async fn new_task<T>(f: impl Future<Output = T>) -> T {
    if SENDER.read().unwrap().is_none() {
        #[cfg(feature = "tracing")]
        tracing::trace!(tid=?tokio::task::id(), "running with reord disabled");
        return f.await;
    }

    let (s, r) = oneshot::channel();
    SENDER
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .send(Message::NewTask(StopPoint::without_lock(s)))
        .expect("submitting credentials to run");
    #[cfg(feature = "tracing")]
    tracing::trace!(tid=?tokio::task::id(), "prepared for running");
    r.await
        .expect("Overseer died, please check other panic messages");
    #[cfg(feature = "tracing")]
    tracing::trace!(tid=?tokio::task::id(), "running");
    // We just pause the panic long enough to send TaskEnd, so this should be OK.
    let res = std::panic::AssertUnwindSafe(f).catch_unwind().await;
    #[cfg(feature = "tracing")]
    tracing::trace!(tid=?tokio::task::id(), "finished running");
    SENDER
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .send(Message::TaskEnd)
        .expect("submitting task end");
    match res {
        Ok(r) => r,
        Err(e) => std::panic::resume_unwind(e),
    }
}

pub async fn start(tasks: usize) -> tokio::task::JoinHandle<()> {
    let (cfg, mut receiver) = OVERSEER
        .lock()
        .unwrap()
        .take()
        .expect("Called `reord::start` without a `reord::init_test` call before");

    let mut new_tasks = Vec::with_capacity(tasks);
    for _ in 0..tasks {
        match receiver.recv().await.unwrap() {
            Message::NewTask(s) => new_tasks.push(s),
            m => {
                panic!("Got unexpected message {m:?} before {tasks} tasks were ready for execution")
            }
        }
    }

    let sender_lock = SENDER.read().unwrap();
    let sender = sender_lock
        .as_ref()
        .expect("Called `start` without `init_test` having run before.");
    for s in new_tasks {
        sender
            .send(Message::NewTask(s))
            .expect("re-submitting the new tasks message");
    }
    sender
        .send(Message::Start)
        .expect("submitting start message");
    std::mem::drop(sender_lock);

    let mut rng = rand::rngs::StdRng::from_seed(cfg.seed);
    tokio::task::spawn(async move {
        let mut locks = HashSet::<LockInfo>::new();
        let mut pending_stops = Vec::<StopPoint>::new();
        let mut blocked_task_waiting_on: HashSet<LockInfo> = HashSet::new();
        let mut skip_next_resume = false;
        while let Some(m) = receiver.recv().await {
            let should_resume = matches!(m, Message::Stop(_) | Message::Start | Message::TaskEnd);
            match m {
                Message::Start | Message::TaskEnd => (),
                Message::Unlock(l) => {
                    locks.remove(&l);
                    if !blocked_task_waiting_on.is_empty() {
                        blocked_task_waiting_on.remove(&l);
                        // Skip the next resume if this unblocked the blocked task
                        skip_next_resume = blocked_task_waiting_on.is_empty();
                    }
                }
                Message::NewTask(p) | Message::Stop(p) => {
                    pending_stops.push(p);
                }
            }
            if !should_resume {
                continue;
            }
            if skip_next_resume {
                skip_next_resume = false;
                continue;
            }
            if pending_stops.is_empty() {
                break;
            }
            let resumable_stop_idxs = (0..pending_stops.len())
                .filter(|s| {
                    cfg.can_resume(
                        &pending_stops[*s],
                        &locks,
                        !blocked_task_waiting_on.is_empty(),
                    )
                })
                .collect::<Vec<_>>();
            let resume_idx = resumable_stop_idxs
                .choose(&mut rng)
                .expect("Deadlock detected!");
            let resume = pending_stops.swap_remove(*resume_idx);
            #[cfg(feature = "tracing")]
            if cfg
                .lock_check_time(&resume.locks_about_to_be_acquired, &locks)
                .is_some()
            {
                tracing::debug!(
                    new_locks=?resume.locks_about_to_be_acquired,
                    already_locked=?locks,
                    "tentatively resuming a task to validate it does not make progress"
                )
            }
            resume.resume.send(()).expect("Failed to resume a task");
            if !resume.locks_about_to_be_acquired.is_empty() {
                let lock_check_time =
                    cfg.lock_check_time(&resume.locks_about_to_be_acquired, &locks);
                let conflicting_locks = resume
                    .locks_about_to_be_acquired
                    .iter()
                    .filter(|l| locks.contains(&l))
                    .cloned()
                    .collect();
                locks.extend(resume.locks_about_to_be_acquired.iter().cloned());
                if let Some(lock_check_time) = lock_check_time {
                    match tokio::time::timeout(lock_check_time, receiver.recv()).await {
                        Ok(_) => panic!(
                            "Locks {:?} did not actually prevent the task from executing when it should have been blocked",
                            resume.locks_about_to_be_acquired,
                        ),
                        Err(_) => (),
                    }
                    blocked_task_waiting_on = conflicting_locks;
                    SENDER
                        .read()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .send(Message::Start)
                        .unwrap();
                }
            }
        }
    })
}

pub async fn point() {
    if SENDER.read().unwrap().is_none() {
        return;
    }
    let (s, r) = oneshot::channel();
    SENDER
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .send(Message::Stop(StopPoint::without_lock(s)))
        .expect("submitting stop point");
    #[cfg(feature = "tracing")]
    tracing::trace!(tid=?tokio::task::id(), "pausing");
    r.await
        .expect("Overseer died, please check other panic messages");
    #[cfg(feature = "tracing")]
    tracing::trace!(tid=?tokio::task::id(), "resuming");
}

#[derive(Debug)]
pub struct Lock(Vec<LockInfo>);

impl Lock {
    #[inline]
    pub async fn take_named(s: String) -> Lock {
        Self::take_atomic(vec![LockInfo::Named(s)]).await
    }

    #[inline]
    pub async fn take_addressed(a: usize) -> Lock {
        Self::take_atomic(vec![LockInfo::Addressed(a)]).await
    }

    pub async fn take_atomic(l: Vec<LockInfo>) -> Lock {
        if SENDER.read().unwrap().is_none() {
            return Lock(l);
        }
        let (resume, wait) = oneshot::channel();
        SENDER
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .send(Message::Stop(StopPoint {
                resume,
                locks_about_to_be_acquired: l.clone(),
            }))
            .expect("sending stop point");
        #[cfg(feature = "tracing")]
        tracing::trace!(tid=?tokio::task::id(), locks=?l, "pausing waiting for locks");
        wait.await
            .expect("Overseer died, please check other panic messages");
        #[cfg(feature = "tracing")]
        tracing::trace!(tid=?tokio::task::id(), locks=?l, "resuming and acquiring locks");
        Lock(l)
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        #[cfg(feature = "tracing")]
        tracing::trace!(tid=?tokio::task::id(), locks=?self.0, "releasing locks");
        for l in self.0.iter() {
            // Avoid double-panic on lock failures.
            SENDER
                .read()
                .unwrap()
                .as_ref()
                .map(|s| s.send(Message::Unlock(l.clone())));
        }
    }
}
