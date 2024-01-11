use crate as reord;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

#[tokio::test]
async fn basic_working_test() {
    println!("before initializing test");
    reord::init_test(reord::Config::with_random_seed()).await;
    println!("initialized test");

    let a = tokio::task::spawn(reord::new_task(async move {
        println!("started task 1");
        reord::point().await;
        println!("finished task 1");
    }));

    let b = tokio::task::spawn(reord::new_task(async move {
        println!("started task 2");
        reord::point().await;
        println!("finished task 2");
    }));

    println!("before running tests");
    let h = reord::start(2).await;
    println!("finished tests");

    tokio::try_join!(a, b, h).unwrap();
}

#[tokio::test]
async fn basic_failing_test() {
    reord::init_test(reord::Config::from_seed(Default::default())).await;

    let data = Arc::new(AtomicUsize::new(0));
    let data2 = data.clone();

    let a = tokio::task::spawn(reord::new_task(async move {
        data.fetch_add(1, Ordering::Relaxed);
        reord::point().await;
        assert!(data.load(Ordering::Relaxed) < 2);
    }));

    let b = tokio::task::spawn(reord::new_task(async move {
        data2.fetch_add(1, Ordering::Relaxed);
        reord::point().await;
        assert!(data2.load(Ordering::Relaxed) < 2);
    }));

    let h = reord::start(2).await;

    tokio::try_join!(a, b, h).unwrap_err();
}

#[tokio::test]
async fn check_failing_locks() {
    reord::init_test(reord::Config {
        check_named_locks_work_for: Some(Duration::from_secs(1)),
        ..reord::Config::from_seed(Default::default())
    })
    .await;

    let a = tokio::task::spawn(reord::new_task(async move {
        println!("before lock 1");
        {
            let _l = reord::Lock::take_named(String::from("foo")).await;
            println!("in lock 1");
            reord::point().await;
        }
        reord::point().await;
        println!("after lock 1");
    }));

    let b = tokio::task::spawn(reord::new_task(async move {
        println!("before lock 2");
        {
            let _l = reord::Lock::take_named(String::from("foo")).await;
            println!("in lock 2");
            reord::point().await;
        }
        reord::point().await;
        println!("after lock 2");
    }));

    let h = reord::start(2).await;

    let (_, _, msg) = tokio::join!(a, b, h);
    assert!(msg.unwrap_err().is_panic());
}

#[tokio::test]
async fn check_passing_locks() {
    reord::init_test(reord::Config {
        check_named_locks_work_for: Some(Duration::from_secs(1)),
        ..reord::Config::from_seed(Default::default())
    })
    .await;

    let lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let lock2 = lock.clone();
    let a = tokio::task::spawn(reord::new_task(async move {
        println!("before lock 1");
        {
            let _l = reord::Lock::take_named(String::from("foo")).await;
            println!("taking lock 1");
            let _l = lock.lock().await;
            println!("in lock 1");
            reord::point().await;
        }
        reord::point().await;
        println!("after lock 1");
    }));

    let b = tokio::task::spawn(reord::new_task(async move {
        println!("before lock 2");
        {
            let _l = reord::Lock::take_named(String::from("foo")).await;
            println!("taking lock 2");
            let _l = lock2.lock().await;
            println!("in lock 2");
            reord::point().await;
        }
        reord::point().await;
        println!("after lock 2");
    }));

    let h = reord::start(2).await;

    tokio::try_join!(a, b, h).unwrap();
}

#[tokio::test]
async fn detect_deadlock() {
    tracing::subscriber::set_global_default(
        tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::TRACE)
            .finish(),
    )
    .unwrap();

    reord::init_test(reord::Config {
        check_addressed_locks_work_for: Some(Duration::from_secs(1)),
        ..reord::Config::from_seed(Default::default())
    })
    .await;

    let lock1 = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let lock1_clone = lock1.clone();
    let lock2 = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let lock2_clone = lock2.clone();
    let a = tokio::task::spawn(reord::new_task(async move {
        {
            println!("A taking lock 1");
            let _l = reord::Lock::take_addressed(1).await;
            let _l = lock1.lock().await;
            reord::point().await;
            eprintln!("A taking lock 2");
            let _l = reord::Lock::take_addressed(2).await;
            let _l = lock2.lock().await;
            println!("A successfully taken both locks");
            reord::point().await;
        }
        println!("A after unlock");
    }));

    let b = tokio::task::spawn(reord::new_task(async move {
        {
            println!("B taking lock 2");
            let _l = reord::Lock::take_addressed(2).await;
            let _l = lock2_clone.lock().await;
            reord::point().await;
            eprintln!("B taking lock 1");
            let _l = reord::Lock::take_addressed(1).await;
            let _l = lock1_clone.lock().await;
            println!("B successfully taken both locks");
            reord::point().await;
        }
        println!("B after unlock");
    }));

    let h = reord::start(2).await;

    tokio::try_join!(a, b, h).unwrap_err();
}

#[tokio::test]
#[allow(non_snake_case)]
async fn lock_LUL_vs_L_deadlocked() {
    // This bug was with the following interleaving:
    // - A takes lock 1
    // - B takes lock 1. Reord lets B go for it in order to check that B doesn't progress. It works fine, reord continues.
    // - A releases lock 1. This should give the lock to B, but here reord was then counting the lock as being entirely free.
    // - A takes lock 1. This should be prevented by reord, but reord mistakenly thought that lock 1 was free. Ergo, deadlock.
    tracing::subscriber::set_global_default(
        tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::TRACE)
            .finish(),
    )
    .unwrap();

    reord::init_test(reord::Config {
        check_addressed_locks_work_for: Some(Duration::from_secs(1)),
        ..reord::Config::from_seed(Default::default())
    })
    .await;

    let lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let lock_clone = lock.clone();
    let a = tokio::task::spawn(reord::new_task(async move {
        {
            {
                println!("A taking lock 1");
                let _l = reord::Lock::take_addressed(1).await;
                let _l = lock.lock().await;
                reord::point().await;
                eprintln!("A releasing lock 1");
            }
            reord::point().await;
            eprintln!("A taking lock 1");
            let _l = reord::Lock::take_addressed(1).await;
            let _l = lock.lock().await;
        }
        println!("A after unlock");
    }));

    let b = tokio::task::spawn(reord::new_task(async move {
        {
            eprintln!("B taking lock 1");
            let _l = reord::Lock::take_addressed(1).await;
            let _l = lock_clone.lock().await;
            // A few awaits to make sure the RNG makes B keep the lock for long enough to get back to A
            reord::point().await;
            reord::point().await;
            reord::point().await;
            reord::point().await;
            reord::point().await;
        }
        println!("B after unlock");
    }));

    let h = reord::start(2).await;

    tokio::try_join!(a, b, h).unwrap();
}

#[tokio::test]
async fn check_passing_two_locks() {
    reord::init_test(reord::Config {
        check_named_locks_work_for: Some(Duration::from_secs(1)),
        ..reord::Config::from_seed(Default::default())
    })
    .await;

    let lock1 = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let lock1_clone = lock1.clone();
    let lock2 = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let lock2_clone = lock2.clone();
    let a = tokio::task::spawn(reord::new_task(async move {
        println!("before lock 1");
        {
            let _l = reord::Lock::take_atomic(vec![
                reord::LockInfo::Named(String::from("lock1")),
                reord::LockInfo::Named(String::from("lock2")),
            ])
            .await;
            println!("taking lock 1");
            let _l = lock1.lock().await;
            let _l = lock2.lock().await;
            println!("in lock 1");
            reord::point().await;
        }
        reord::point().await;
        println!("after lock 1");
    }));

    let b = tokio::task::spawn(reord::new_task(async move {
        println!("before lock 2");
        {
            let _l = reord::Lock::take_atomic(vec![
                reord::LockInfo::Named(String::from("lock1")),
                reord::LockInfo::Named(String::from("lock2")),
            ])
            .await;
            println!("taking lock 2");
            let _l = lock1_clone.lock().await;
            let _l = lock2_clone.lock().await;
            println!("in lock 2");
            reord::point().await;
        }
        reord::point().await;
        println!("after lock 2");
    }));

    let h = reord::start(2).await;

    tokio::try_join!(a, b, h).unwrap();
}

#[tokio::test]
async fn waiting_on_two_locks_vs_one() {
    reord::init_test(reord::Config {
        check_addressed_locks_work_for: Some(std::time::Duration::from_millis(100)),
        ..reord::Config::from_seed(Default::default())
    })
    .await;

    let lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let lock_clone = lock.clone();

    let a = tokio::task::spawn(reord::new_task(async move {
        let _l = reord::Lock::take_addressed(0).await;
        let _l = lock.lock().await;
        reord::point().await;
    }));

    let b = tokio::task::spawn(reord::new_task(async move {
        let _l = reord::Lock::take_atomic(vec![
            reord::LockInfo::Addressed(0),
            reord::LockInfo::Addressed(1),
        ])
        .await;
        let _l = lock_clone.lock().await;
        reord::point().await;
    }));

    let h = reord::start(2).await;

    tokio::try_join!(a, b, h).unwrap();
}

#[tokio::test]
async fn functions_without_init_dont_break() {
    let lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let lock2 = lock.clone();
    let a = tokio::task::spawn(reord::new_task(async move {
        {
            let _l = reord::Lock::take_named(String::from("foo")).await;
            let _l = lock.lock().await;
            reord::point().await;
        }
        reord::point().await;
    }));

    let b = tokio::task::spawn(reord::new_task(async move {
        {
            let _l = reord::Lock::take_named(String::from("foo")).await;
            let _l = lock2.lock().await;
            reord::point().await;
        }
        reord::point().await;
    }));

    tokio::try_join!(a, b).unwrap();
}

#[tokio::test]
async fn two_tests_same_thread() {
    // First test
    reord::init_test(reord::Config::with_random_seed()).await;

    let a = tokio::task::spawn(reord::new_task(async move {
        reord::point().await;
    }));

    let b = tokio::task::spawn(reord::new_task(async move {
        reord::point().await;
    }));

    let h = reord::start(2).await;

    tokio::try_join!(a, b, h).unwrap();

    // Second test
    reord::init_test(reord::Config::with_random_seed()).await;

    let a = tokio::task::spawn(reord::new_task(async move {
        reord::point().await;
    }));

    let b = tokio::task::spawn(reord::new_task(async move {
        reord::point().await;
    }));

    let h = reord::start(2).await;

    tokio::try_join!(a, b, h).unwrap();
}

#[tokio::test]
#[should_panic]
async fn join_does_not_deadlock() {
    reord::init_test(reord::Config::with_random_seed()).await;

    let a = tokio::task::spawn(reord::new_task(async move {
        reord::point().await;
    }));

    let b = tokio::task::spawn(reord::new_task(async move {
        panic!("willing fail");
    }));

    let h = reord::start(2).await;

    let (a, b, h) = tokio::join!(a, b, h);
    a.unwrap();
    b.unwrap();
    h.unwrap();
}
