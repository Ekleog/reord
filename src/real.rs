use crate::Config;
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

#[derive(Debug)]
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

#[derive(Debug)]
enum Message {
    NewTask(StopPoint),
    Start,
    Stop(StopPoint),
    Unlock(LockData),
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
    r.await
        .expect("Overseer died, please check other panic messages");
    let res = f.await;
    SENDER
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .send(Message::TaskEnd)
        .expect("submitting task end");
    res
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
        let mut locks = HashSet::<LockData>::new();
        let mut pending_stops = Vec::<StopPoint>::new();
        let mut blocked_task_waiting_on = None;
        let mut skip_next_resume = false;
        while let Some(m) = receiver.recv().await {
            let should_resume = matches!(m, Message::Stop(_) | Message::Start | Message::TaskEnd);
            match m {
                Message::Start | Message::TaskEnd => (),
                Message::Unlock(l) => {
                    locks.remove(&l);
                    if blocked_task_waiting_on == Some(l) {
                        blocked_task_waiting_on = None;
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
    r.await
        .expect("Overseer died, please check other panic messages");
}

#[derive(Debug)]
pub struct Lock(LockData);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum LockData {
    Named(String),
    Addressed(usize),
}

impl Lock {
    #[inline]
    pub async fn take_named(s: String) -> Lock {
        Self::take(LockData::Named(s)).await
    }

    #[inline]
    pub async fn take_addressed(a: usize) -> Lock {
        Self::take(LockData::Addressed(a)).await
    }

    async fn take(l: LockData) -> Lock {
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
                lock_about_to_be_acquired: Some(l.clone()),
            }))
            .expect("sending stop point");
        wait.await
            .expect("Overseer died, please check other panic messages");
        Lock(l)
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // Avoid double-panic on lock failures.
        SENDER
            .read()
            .unwrap()
            .as_ref()
            .map(|s| s.send(Message::Unlock(self.0.clone())));
    }
}
