# `reord`

*Run your tests multi-threaded, but in a reproducible way.*

This crate provides testing utilities, to validate that your code comply to some concurrency conditions.

It will run multiple asynchronous tasks with an interleaved by randomized order, to validate that most execution paths verify the properties asserted by the test.

In addition, if configured to, it can check that your locks work properly, by waiting for some time after each theoretical lock collision and failing if the lock was acquired twice. This is useful for example when trying to lock using external tools, or when implementing your custom locks.

## Why not `loom`?

`loom` is a complete model checker, that can fully verify that your code is correct. If your code can handle being checked by `loom`, then by all means please do it, `loom` will do a better job than `reord` at checking your crate!

However, `loom` requires you to replace all your mutex accesses with `loom` ones. This is not always possible, for example when you have some mutexes that are taken by FFI. Or worse, the reason for which this crate was written, when you are trying to verify that postgresql transactions are written with proper locking behavior.

## What does `reord` actually do?

`reord` will not model-check your code. It will just run it once, as though it were running with multiple threads. However, it will provide you with one main thing: deterministic interleaving, that will allow you to reproduce your bugs.

This makes `reord` suitable to be used inside fuzzers, to run your tests a lot of times with different code paths.

## How to use `reord`?

You can (and should) sprinkle `reord` function calls throughout your production code, as it will compile to noops unless the `reord/test` feature is set. `reord` is able to interleave your threads at any `reord::point().await` call. Note that calling any `reord` function implicitly implies `reord::point`, with the exception of lock guard release due to the absence of `async Drop`.

One thing to note is, `reord` will run only a single task at a time. So if you take locks, it must be aware of that, in order to avoid a task blocking on a lock that another task is owning. There are two ways to indicate locks. One is `reord::Lock`, which indicates a lock being taken. It should be placed just before the lock acquisition, and returns a lock guard that should be kept for as long as the real lock's lock guard, so that the `reord` lock gets unlocked a the same time as the real lock gets unlocked.

`reord` also supports "fuzzy" locks: locks where you are not really sure whether they will lock or not. For example, with postgresql, most queries inside a transaction will take a lock that is hard to quantify. For example it might take a page-level lock, at which point it is basically impossible to guess which other queries might be affected. In order to do that, you should put a `reord::maybe_lock().await` call just before the fuzzy lock, and a `reord::point().await` just after it.

`reord` also supports checking that your locks actually do lock what you're expecting them to lock. Again, this might be useful if you do not trust your lock implementation (or the external locks you are using), or the way you instrumented your locks with `reord`. To do this, you should set the appropriate configuration options.

Note however that checking locks, as well as fuzzy locks, imply waiting for some delay before making progress. Hence, try doing that as sparsely as possible, as it will make your tests (or, more importantly, your fuzzers) run way slower whenever there is actually lock contention.

Finally, when you need to debug a failing test, you can take:
1. take the seed used to execute the failing test
2. run the test again with the same seed, but with the `reord/tracing` feature enabled
3. read the logs at `TRACE` level to identify the exact interleaving pattern

`reord` is pretty verbose, and lets you know whenever it switches tasks. It also provides you with an `INFO`-level span that will let you know, even for your own logs, which task it was executing under.

## Limitations

Given that convenience was a goal, `reord` uses global variables to proxy information on the running test. This means that a threaded test framework like `cargo test` will not work with multiple `reord` tests. You should use `cargo nextest` when having tests that use `reord`.
