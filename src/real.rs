use crate::{Config, LockInfo};
use futures::FutureExt;
use rand::{rngs::StdRng, seq::SliceRandom, SeedableRng};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex, RwLock,
    },
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

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
    task_id: usize,
    resume: oneshot::Sender<()>,
    maybe_lock: bool,
    locks_about_to_be_acquired: Vec<LockInfo>,
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
    Start,
    Stop(StopPoint),
    Unlock(usize, LockInfo),
    TaskEnd(usize),
}

impl Message {
    fn task_id(&self) -> usize {
        match self {
            Message::NewTask(p) | Message::Stop(p) => p.task_id,
            Message::TaskEnd(t) => *t,
            Message::Unlock(t, _) => *t,
            Message::Start => panic!("Called task_id on Message::Start"),
        }
    }
}

static SENDER: RwLock<Option<mpsc::UnboundedSender<Message>>> = RwLock::new(None);
static OVERSEER: Mutex<Option<(Config, mpsc::UnboundedReceiver<Message>)>> = Mutex::new(None);
static NEXT_TASK_ID: AtomicUsize = AtomicUsize::new(1);

tokio::task_local! {
    static TASK_ID: usize;
}

struct Task {
    // TODO: Should this replace all the various hashsets from Overseer?
}

struct Overseer {
    receiver: mpsc::UnboundedReceiver<Message>,
    sender: mpsc::UnboundedSender<Message>,
    cfg: Config,
    rng: StdRng,
    tasks: HashMap<usize, Task>,
    locks: HashSet<LockInfo>,
    pending_stops: Vec<StopPoint>,
    blocked_task_waiting_on: HashSet<LockInfo>,
    tasks_locked_in_maybe: HashSet<usize>,
    skip_next_resume: bool,
}

impl Overseer {
    fn new(cfg: Config, receiver: mpsc::UnboundedReceiver<Message>) -> Overseer {
        Overseer {
            receiver,
            sender: SENDER.read().unwrap().clone().unwrap(),

            rng: StdRng::seed_from_u64(cfg.seed),
            cfg,

            tasks: HashMap::new(),
            locks: HashSet::new(),
            pending_stops: Vec::new(),
            blocked_task_waiting_on: HashSet::new(),
            tasks_locked_in_maybe: HashSet::new(),
            skip_next_resume: false,
        }
    }

    // Returns true iff we should resume a task after this message
    fn handle_message(&mut self, m: Message) -> bool {
        match m {
            Message::Start => true,
            Message::TaskEnd(t) => {
                self.tasks.remove(&t);
                true
            }
            Message::Unlock(_, l) => {
                if self.blocked_task_waiting_on.is_empty() {
                    // There's no blocked task. Simple path.
                    self.locks.remove(&l);
                } else {
                    // There's a blocked task. Hard path.
                    // If there's already a task blocked on this lock, give the lock to the task
                    // If not, release the lock normally
                    if !self.blocked_task_waiting_on.remove(&l) {
                        self.locks.remove(&l);
                    }
                    // Skip the next resume if this unblocked the blocked task
                    self.skip_next_resume = self.blocked_task_waiting_on.is_empty();
                }
                false
            }
            Message::NewTask(p) => {
                self.tasks.insert(p.task_id, Task {});
                self.pending_stops.push(p);
                false
            }
            Message::Stop(p) => {
                let task_id = p.task_id;
                self.pending_stops.push(p);
                if self.tasks_locked_in_maybe.remove(&task_id) {
                    // The task was blocked in a "maybe_lock". Do not resume anything.
                    return false;
                }
                true
            }
        }
    }

    fn get_resumable_task(&mut self) -> StopPoint {
        let resumable_stop_idxs = (0..self.pending_stops.len())
            .filter(|s| {
                self.cfg.can_resume(
                    &self.pending_stops[*s],
                    &self.locks,
                    !self.blocked_task_waiting_on.is_empty(),
                )
            })
            .collect::<Vec<_>>();
        let resume_idx = resumable_stop_idxs
            .choose(&mut self.rng)
            .expect("Deadlock detected!");
        let resume = self.pending_stops.swap_remove(*resume_idx);
        #[cfg(feature = "tracing")]
        if self
            .cfg
            .lock_check_time(&resume.locks_about_to_be_acquired, &self.locks)
            .is_some()
        {
            tracing::debug!(
                new_locks=?resume.locks_about_to_be_acquired,
                already_locked=?self.locks,
                "tentatively resuming a task to validate it does not make progress"
            )
        }
        resume
    }

    // Return true iff we should resume another task after this
    async fn just_resumed_maybe_lock(&mut self, task_id: usize) -> bool {
        let deadline = Instant::now() + self.cfg.maybe_lock_timeout;
        let mut should_resume_after = false;
        loop {
            match tokio::time::timeout_at(deadline, self.receiver.recv()).await {
                Err(_) => return true, // reached timeout
                Ok(Some(m)) => {
                    if m.task_id() == task_id {
                        // continued and reached stop point
                        self.sender.send(m).unwrap();
                        return should_resume_after; // will resume when handling `m`
                    } else {
                        // Never resume a task here
                        should_resume_after |= self.handle_message(m);
                    }
                }
                Ok(None) => unreachable!(),
            }
        }
    }

    // Panics if the deadlock check was unsuccessful
    async fn just_resumed_for_deadlock_check(
        &mut self,
        task_id: usize,
        check_for: Duration,
        locks: &[LockInfo],
        conflicting_locks: HashSet<LockInfo>,
    ) {
        let deadline = Instant::now() + check_for;
        loop {
            match tokio::time::timeout_at(deadline, self.receiver.recv()).await {
                Err(_) => {
                    // reached timeout
                    self.blocked_task_waiting_on = conflicting_locks;
                    return;
                }
                Ok(Some(m)) => {
                    if m.task_id() == task_id {
                        panic!("Locks {locks:?} did not actually prevent the task from executing when it should have been blocked")
                    } else {
                        // Never resume a task here
                        self.handle_message(m);
                    }
                }
                Ok(None) => unreachable!(),
            }
        }
    }

    // Returns true iff the test is over
    async fn do_resume(&mut self) -> bool {
        if self.skip_next_resume {
            self.skip_next_resume = false;
            return false;
        }
        if self.pending_stops.is_empty() {
            if self.blocked_task_waiting_on.is_empty() && self.tasks_locked_in_maybe.is_empty() {
                return true;
            }
            // Some tasks are currently blocked, wait for them
            match tokio::time::timeout(Duration::from_secs(10), self.receiver.recv()).await {
                Err(_) => panic!("10 seconds elapsed without blocked tasks making progress"),
                Ok(m) => {
                    self.sender.send(m.unwrap()).unwrap();
                    return false;
                }
            }
        }
        loop {
            let resume = self.get_resumable_task();
            resume.resume.send(()).expect("Failed to resume a task");
            let mut should_resume_again = false;
            if resume.maybe_lock {
                should_resume_again |= self.just_resumed_maybe_lock(resume.task_id).await;
            }
            if !resume.locks_about_to_be_acquired.is_empty() {
                let lock_check_time = self
                    .cfg
                    .lock_check_time(&resume.locks_about_to_be_acquired, &self.locks);
                let conflicting_locks = resume
                    .locks_about_to_be_acquired
                    .iter()
                    .filter(|l| self.locks.contains(&l))
                    .cloned()
                    .collect();
                self.locks
                    .extend(resume.locks_about_to_be_acquired.iter().cloned());
                if let Some(lock_check_time) = lock_check_time {
                    self.just_resumed_for_deadlock_check(
                        resume.task_id,
                        lock_check_time,
                        &resume.locks_about_to_be_acquired,
                        conflicting_locks,
                    )
                    .await;
                    should_resume_again = true;
                }
            }
            if !should_resume_again {
                return false;
            }
        }
    }

    async fn run(&mut self) {
        while let Some(m) = self.receiver.recv().await {
            let should_resume = self.handle_message(m);
            if should_resume {
                let test_over = self.do_resume().await;
                if test_over {
                    assert!(self.tasks.is_empty());
                    return;
                }
            }
        }
    }
}

pub async fn init_test(cfg: Config) {
    let (s, r) = mpsc::unbounded_channel();
    if let Some(s) = SENDER.write().unwrap().replace(s) {
        if !s.send(Message::Start).is_err() {
            panic!("Initializing a new test while the old test was still running! Note that `reord` is only designed to work with `cargo-nextest`.");
        }
    }

    assert!(OVERSEER.lock().unwrap().replace((cfg, r)).is_none());
    NEXT_TASK_ID.store(1, Ordering::Relaxed);
}

pub async fn new_task<T>(f: impl Future<Output = T>) -> T {
    let task_id = NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed);
    let task_fut = TASK_ID.scope(task_id, async move {
        if SENDER.read().unwrap().is_none() {
            #[cfg(feature = "tracing")]
            tracing::trace!(tid = TASK_ID.get(), "running with reord disabled");
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

    tokio::task::spawn(async move { Overseer::new(cfg, receiver).run().await })
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
