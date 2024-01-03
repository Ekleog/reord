use crate as reord;

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
    reord::start(2).await;
    println!("finished tests");

    tokio::try_join!(a, b).unwrap();
}
