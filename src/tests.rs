use crate as reord;
use std::time::Duration;

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
async fn check_locks() {
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
