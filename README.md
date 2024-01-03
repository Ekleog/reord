# `reord`

This crate provides testing utilities, to validate that your code comply to some concurrency conditions.

It will run multiple asynchronous tasks with an interleaved by randomized order, to validate that most execution paths verify the properties asserted by the test.

In addition, if configured to, it can check that your locks work properly, by waiting for some time after each theoretical lock collision and failing if the lock was acquired twice. This is useful for example when trying to lock using external tools, or when implementing your custom locks.

You can (and should) sprinkle `reord` function calls throughout your production code, as it will compile to noops when the `test` feature is not set.

On the other hand, given this convenience was a goal, `reord` uses global variables to proxy information on the running test. This means that a threaded test framework like `cargo test` will not work with multiple `reord` tests. You should use `cargo nextest` when having tests that use `reord`.
