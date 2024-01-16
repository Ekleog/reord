use crate::{Config, LockInfo};
use futures::FutureExt;
use rand::{rngs::StdRng, seq::SliceRandom, SeedableRng};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    ops::RangeBounds,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, RwLock,
    },
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

struct StopPoint {
    task_id: u64,
    resume: oneshot::Sender<()>,
    maybe_lock: bool,
    locks_about_to_be_acquired: Vec<LockInfo>,
}

impl std::fmt::Debug for StopPoint {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("StopPoint")
            .field("task_id", &self.task_id)
            .field("maybe_lock", &self.maybe_lock)
            .field(
                "locks_about_to_be_acquired",
                &self.locks_about_to_be_acquired,
            )
            .finish()
    }
}

impl StopPoint {
    fn without_lock(resume: oneshot::Sender<()>) -> StopPoint {
        StopPoint {
            task_id: TASK_ID.get(),
            resume,
            maybe_lock: false,
            locks_about_to_be_acquired: Vec::new(),
        }
    }

    fn with_locks(
        resume: oneshot::Sender<()>,
        locks_about_to_be_acquired: Vec<LockInfo>,
    ) -> StopPoint {
        StopPoint {
            task_id: TASK_ID.get(),
            resume,
            maybe_lock: false,
            locks_about_to_be_acquired,
        }
    }

    fn maybe_lock(resume: oneshot::Sender<()>) -> StopPoint {
        StopPoint {
            task_id: TASK_ID.get(),
            resume,
            maybe_lock: true,
            locks_about_to_be_acquired: Vec::new(),
        }
    }
}

#[derive(Debug)]
enum Message {
    NewTask(StopPoint),
    Stop(StopPoint),
    Unlock(u64, LockInfo),
    TaskEnd(u64),
}

impl Message {
    fn task_id(&self) -> u64 {
        match self {
            Message::NewTask(p) | Message::Stop(p) => p.task_id,
            Message::Unlock(t, _) | Message::TaskEnd(t) => *t,
        }
    }
}

static SENDER: RwLock<Option<mpsc::UnboundedSender<Message>>> = RwLock::new(None);
static OVERSEER: Mutex<Option<(Config, mpsc::UnboundedReceiver<Message>)>> = Mutex::new(None);
static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);

tokio::task_local! {
    static TASK_ID: u64;
}

#[derive(Debug)]
enum TaskState {
    Running,
    BlockedOn(HashSet<LockInfo>),
    BlockedOnMaybe,
    Waiting(StopPoint),
}

#[derive(Debug)]
struct Task {
    state: TaskState,
    owned_locks: HashSet<LockInfo>,
}

impl Task {
    fn new(stop: StopPoint) -> Task {
        Task {
            state: TaskState::Waiting(stop),
            owned_locks: HashSet::new(),
        }
    }
}

#[derive(Debug)]
struct Overseer {
    receiver: mpsc::UnboundedReceiver<Message>,
    sender: mpsc::UnboundedSender<Message>,
    cfg: Config,
    rng: StdRng,
    // Invariant: only one task has state BlockedOn at a time, to avoid issues with not knowing
    // which task got the lock after an unlock
    tasks: HashMap<u64, Task>,
}

impl Overseer {
    fn new(
        cfg: Config,
        receiver: mpsc::UnboundedReceiver<Message>,
        initial_tasks: Vec<StopPoint>,
    ) -> Overseer {
        Overseer {
            receiver,
            sender: SENDER.read().unwrap().clone().unwrap(),

            rng: StdRng::seed_from_u64(cfg.seed),
            cfg,

            tasks: initial_tasks
                .into_iter()
                .map(|p| (p.task_id, Task::new(p)))
                .collect(),
        }
    }

    fn handle_message(&mut self, m: Message) {
        match m {
            Message::TaskEnd(t) => {
                let remaining_locks = &self.tasks.get(&t).unwrap().owned_locks;
                assert!(remaining_locks.is_empty(), "Task completed without releasing all its locks: it still had {remaining_locks:?}");
                self.tasks.remove(&t);
            }
            Message::Unlock(t, l) => {
                self.tasks.get_mut(&t).unwrap().owned_locks.remove(&l);
                for t in self.tasks.values_mut() {
                    if let TaskState::BlockedOn(locks) = &mut t.state {
                        if locks.remove(&l) {
                            t.owned_locks.insert(l);
                            t.state = TaskState::Running;
                            break;
                        }
                    }
                }
            }
            Message::NewTask(p) => {
                self.tasks.insert(p.task_id, Task::new(p));
            }
            Message::Stop(p) => {
                let task = self.tasks.get_mut(&p.task_id).unwrap();
                task.state = TaskState::Waiting(p);
            }
        }
    }

    fn is_already_locked(&self, l: &LockInfo) -> bool {
        self.tasks.values().any(|t| t.owned_locks.contains(l))
    }

    fn conflicting_locks_for(
        &self,
        locks_about_to_be_acquired: &Vec<LockInfo>,
    ) -> HashSet<LockInfo> {
        locks_about_to_be_acquired
            .iter()
            .filter(|l| self.is_already_locked(l))
            .cloned()
            .collect()
    }

    fn has_task_blocked_on_locks(&self) -> bool {
        self.tasks
            .values()
            .any(|t| matches!(t.state, TaskState::BlockedOn(_)))
    }

    /// Returns either a duration for which to check the locks, or None if it's not something we're configured to do
    fn time_to_check_locks(&self, locks: &HashSet<LockInfo>) -> Option<Duration> {
        let mut res = Duration::from_millis(0);
        for l in locks {
            match l {
                LockInfo::Addressed(_) => {
                    res = std::cmp::max(res, self.cfg.check_addressed_locks_work_for?)
                }
                LockInfo::Named(_) => {
                    res = std::cmp::max(res, self.cfg.check_named_locks_work_for?)
                }
            }
        }
        Some(res)
    }

    fn can_resume(&self, p: &StopPoint) -> bool {
        let conflicting_locks = self.conflicting_locks_for(&p.locks_about_to_be_acquired);
        conflicting_locks.is_empty()
            || (!self.has_task_blocked_on_locks()
                && self.time_to_check_locks(&conflicting_locks).is_some())
    }

    /// Returns `None` iff there is currently no resumable task
    fn get_resumable_task(&mut self) -> Option<u64> {
        let mut resumable_idxs = self
            .tasks
            .iter()
            .filter_map(|(t, s)| match &s.state {
                TaskState::Waiting(p) if self.can_resume(p) => Some(*t),
                _ => None,
            })
            .collect::<Vec<_>>();
        resumable_idxs.sort(); // make sure we're reproducible and not dependent on hashmap iteration order
        resumable_idxs.choose(&mut self.rng).copied()
    }

    // Returns true iff there is no new message from a task in range `tasks` for `check_for` time
    async fn check_if_task_is_blocked(
        &mut self,
        tasks: impl RangeBounds<u64>,
        check_for: Duration,
    ) -> bool {
        let deadline = Instant::now() + check_for;
        let mut messages_to_push_back = Vec::new();
        let res = loop {
            match tokio::time::timeout_at(deadline, self.receiver.recv()).await {
                Err(_) => {
                    // reached timeout
                    break true;
                }
                Ok(Some(m)) => {
                    // received a message, re-enqueue it before continuing
                    let task_id = m.task_id();
                    messages_to_push_back.push(m);
                    if tasks.contains(&task_id) {
                        break false;
                    }
                }
                Ok(None) => unreachable!(),
            }
        };
        for m in messages_to_push_back {
            self.sender.send(m).unwrap();
        }
        res
    }

    /// Returns true iff we NEED to handle a message RIGHT NOW
    async fn resume_one(&mut self) -> bool {
        let Some(t) = self.get_resumable_task() else {
            // This can happen if the only remaining tasks are all BlockedOnMaybe. Try waiting a bit
            // before confirming the deadlock. Task ID 0 does not
            if self
                .check_if_task_is_blocked(.., self.cfg.maybe_lock_timeout)
                .await
            {
                panic!("Deadlock detected! Task states are {:#?}", self.tasks);
            } else {
                return true; // try again
            }
        };
        let TaskState::Waiting(p) = std::mem::replace(
            &mut self.tasks.get_mut(&t).unwrap().state,
            TaskState::Running,
        ) else {
            unreachable!();
        };
        p.resume.send(()).unwrap();
        if p.maybe_lock {
            assert!(p.locks_about_to_be_acquired.is_empty());
            if self
                .check_if_task_is_blocked(p.task_id..=p.task_id, self.cfg.maybe_lock_timeout)
                .await
            {
                self.tasks.get_mut(&t).unwrap().state = TaskState::BlockedOnMaybe;
            }
        }
        if !p.locks_about_to_be_acquired.is_empty() {
            let conflicting_locks = self.conflicting_locks_for(&p.locks_about_to_be_acquired);
            if conflicting_locks.is_empty() {
                self.tasks
                    .get_mut(&t)
                    .unwrap()
                    .owned_locks
                    .extend(p.locks_about_to_be_acquired.into_iter());
            } else {
                // There were conflicting locks, we need ot check everything
                let check_locks_for = self.time_to_check_locks(&conflicting_locks).unwrap();
                #[cfg(feature = "tracing")]
                tracing::debug!(
                    ?conflicting_locks,
                    "tentatively resuming a task to validate it does not make progress"
                );
                if self
                    .check_if_task_is_blocked(p.task_id..=p.task_id, check_locks_for)
                    .await
                {
                    #[cfg(feature = "tracing")]
                    tracing::debug!(
                        ?conflicting_locks,
                        "task successfully did not make any progress"
                    );
                    let task = self.tasks.get_mut(&t).unwrap();
                    task.owned_locks.extend(
                        p.locks_about_to_be_acquired
                            .into_iter()
                            .filter(|l| !conflicting_locks.contains(&l)),
                    );
                    task.state = TaskState::BlockedOn(conflicting_locks);
                } else {
                    panic!("Locks that should have blocked let the task go through: {conflicting_locks:?}");
                }
            }
        }
        false
    }

    fn should_resume(&self) -> bool {
        !self
            .tasks
            .values()
            .any(|t| matches!(t.state, TaskState::Running))
    }

    async fn run(&mut self) {
        self.resume_one().await; // Start the system
        while let Some(m) = self.receiver.recv().await {
            self.handle_message(m);
            if self.tasks.is_empty() {
                return;
            }
            while self.should_resume() {
                if self.resume_one().await {
                    break;
                }
            }
        }
    }
}

pub async fn init_test(cfg: Config) {
    let (s, r) = mpsc::unbounded_channel();
    if let Some(s) = SENDER.write().unwrap().replace(s) {
        if !s.send(Message::TaskEnd(0)).is_err() {
            panic!("Initializing a new test while the old test was still running! Note that `reord` is only designed to work with `cargo-nextest`.");
        }
    }

    assert!(OVERSEER.lock().unwrap().replace((cfg, r)).is_none());
    NEXT_TASK_ID.store(1, Ordering::Relaxed);
}

pub async fn new_task<T>(f: impl Future<Output = T>) -> T {
    let task_id = NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed);
    assert!(task_id > 0, "Task ID wraparound detected");
    let task_fut = TASK_ID.scope(task_id, async move {
        if SENDER.read().unwrap().is_none() {
            #[cfg(feature = "tracing")]
            tracing::warn!(
                tid = TASK_ID.get(),
                "running with reord disabled, but the `tracing` feature is set"
            );
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
        tracing::trace!("prepared for running");
        r.await
            .expect("Overseer died, please check other panic messages");
        #[cfg(feature = "tracing")]
        tracing::trace!("running");
        // We just pause the panic long enough to send TaskEnd, so this should be OK.
        let res = std::panic::AssertUnwindSafe(f).catch_unwind().await;
        #[cfg(feature = "tracing")]
        tracing::trace!("finished running");
        SENDER
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .send(Message::TaskEnd(TASK_ID.get()))
            .expect("submitting task end");
        match res {
            Ok(r) => r,
            Err(e) => std::panic::resume_unwind(e),
        }
    });
    #[cfg(feature = "tracing")]
    let task_fut =
        tracing::Instrument::instrument(task_fut, tracing::info_span!("task", tid = task_id));
    task_fut.await
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

    tokio::task::spawn(async move { Overseer::new(cfg, receiver, new_tasks).run().await })
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
    tracing::trace!("pausing");
    r.await
        .expect("Overseer died, please check other panic messages");
    #[cfg(feature = "tracing")]
    tracing::trace!("resuming");
}

pub async fn maybe_lock() {
    if SENDER.read().unwrap().is_none() {
        return;
    }
    let (s, r) = oneshot::channel();
    SENDER
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .send(Message::Stop(StopPoint::maybe_lock(s)))
        .expect("submitting stop point");
    #[cfg(feature = "tracing")]
    tracing::trace!("pausing before potential lock");
    r.await
        .expect("Overseer died, please check other panic messages");
    #[cfg(feature = "tracing")]
    tracing::trace!("resuming, about to try taking potential lock");
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
            .send(Message::Stop(StopPoint::with_locks(resume, l.clone())))
            .expect("sending stop point");
        #[cfg(feature = "tracing")]
        tracing::trace!(locks=?l, "pausing waiting for locks");
        wait.await
            .expect("Overseer died, please check other panic messages");
        #[cfg(feature = "tracing")]
        tracing::trace!(locks=?l, "resuming and acquiring locks");
        Lock(l)
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        #[cfg(feature = "tracing")]
        tracing::trace!(locks=?self.0, "releasing locks");
        for l in self.0.iter() {
            // Avoid double-panic on lock failures.
            SENDER
                .read()
                .unwrap()
                .as_ref()
                .map(|s| s.send(Message::Unlock(TASK_ID.get(), l.clone())));
        }
    }
}
