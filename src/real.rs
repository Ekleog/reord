use crate::Config;
use rand::{seq::SliceRandom, SeedableRng};
use std::{collections::HashSet, future::Future, sync::OnceLock, time::Duration};
use tokio::sync::{mpsc, oneshot};

impl Config {
    fn can_resume(
        &self,
        s: &StopPoint,
        locks: &HashSet<LockData>,
        has_blocked_task_waiting: bool,
    ) -> bool {
        match &s.lock_about_to_be_acquired {
            None => true,
            Some(l) if !locks.contains(l) => true,
            Some(_) if has_blocked_task_waiting => false,
            Some(LockData::Addressed(_)) => self.check_addressed_locks_work_for.is_some(),
            Some(LockData::Named(_)) => self.check_named_locks_work_for.is_some(),
        }
    }

    fn lock_check_time(&self, l: &LockData, locks: &HashSet<LockData>) -> Option<Duration> {
        match l {
            l if !locks.contains(l) => None,
            LockData::Addressed(_) => Some(self.check_addressed_locks_work_for.unwrap()),
            LockData::Named(_) => Some(self.check_named_locks_work_for.unwrap()),
        }
    }
}

struct StopPoint {
    resume: oneshot::Sender<()>,
    lock_about_to_be_acquired: Option<LockData>,
}

impl StopPoint {
    fn without_lock(resume: oneshot::Sender<()>) -> StopPoint {
        StopPoint {
            resume,
            lock_about_to_be_acquired: None,
        }
    }
}

enum Message {
    NewTask(StopPoint),
    Start,
    Stop(StopPoint),
    Unlock(LockData),
}

static SENDER: OnceLock<mpsc::UnboundedSender<Message>> = OnceLock::new();

pub async fn init_test(cfg: Config) {
    let mut rng = rand::rngs::StdRng::from_seed(cfg.seed);

    let (s, mut receiver) = mpsc::unbounded_channel();
    SENDER.set(s)
        .expect("The test was already initialized! Note that `reord` is only designed to work with `cargo-nextest`.");

    tokio::task::spawn(async move {
        let mut locks = HashSet::<LockData>::new();
        let mut pending_stops = Vec::<StopPoint>::new();
        let mut blocked_task_waiting_on = None;
        let mut skip_next_resume = false;
        while let Some(m) = receiver.recv().await {
            let should_resume = matches!(m, Message::Stop(_) | Message::Start);
            match m {
                Message::Start => (),
                Message::Unlock(l) => {
                    locks.remove(&l);
                    if blocked_task_waiting_on == Some(l) {
                        skip_next_resume = true;
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
                        blocked_task_waiting_on.is_some(),
                    )
                })
                .collect::<Vec<_>>();
            let resume_idx = resumable_stop_idxs
                .choose(&mut rng)
                .expect("Deadlock detected!");
            let resume = pending_stops.swap_remove(*resume_idx);
            resume.resume.send(()).expect("Failed to resume a task");
            if let Some(lock) = resume.lock_about_to_be_acquired {
                let lock_check_time = cfg.lock_check_time(&lock, &locks);
                locks.insert(lock.clone());
                if let Some(lock_check_time) = lock_check_time {
                    match tokio::time::timeout(lock_check_time, receiver.recv()).await {
                        Ok(_) => panic!(
                            "Lock {lock:?} did not actually prevent other task from executing"
                        ),
                        Err(_) => (),
                    }
                    blocked_task_waiting_on = Some(lock);
                }
            }
        }
    })
    .await
    .expect("Failed spawning the `reord` manager");
}

pub async fn new_task<T>(f: impl Future<Output = T>) -> T {
    let sender = SENDER
        .get()
        .expect("Called `new_task` without `init_test` being run before.");

    let (s, r) = oneshot::channel();
    sender
        .send(Message::NewTask(StopPoint::without_lock(s)))
        .expect("submitting credentials to run");
    r.await.expect("waiting for authorization to run");
    f.await
}

pub async fn start() {
    SENDER
        .get()
        .expect("Called `start` without `init_test` having run before.")
        .send(Message::Start)
        .expect("submitting start message");
}

pub async fn point() {
    let sender = SENDER
        .get()
        .expect("Called `new_task` without `init_test` being run before.");
    let (s, r) = oneshot::channel();
    sender
        .send(Message::Stop(StopPoint::without_lock(s)))
        .expect("submitting stop point");
    r.await.expect("waiting for auth to run");
}

#[derive(Debug)]
pub struct Lock(LockData);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum LockData {
    Named(String),
    Addressed(usize),
}

impl Lock {
    pub async fn take_named(s: String) -> Lock {
        Self::take(LockData::Named(s)).await
    }
    pub async fn take_addressed(a: usize) -> Lock {
        Self::take(LockData::Addressed(a)).await
    }

    async fn take(l: LockData) -> Lock {
        let (resume, wait) = oneshot::channel();
        SENDER
            .get()
            .expect("Called `Lock::take` without `init_test` having run before.")
            .send(Message::Stop(StopPoint {
                resume,
                lock_about_to_be_acquired: Some(l.clone()),
            }))
            .expect("sending stop point");
        wait.await.expect("waiting for authorization to run");
        Lock(l)
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        SENDER
            .get()
            .unwrap()
            .send(Message::Unlock(self.0.clone()))
            .expect("unlocking {self:?}");
    }
}
