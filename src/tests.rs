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
