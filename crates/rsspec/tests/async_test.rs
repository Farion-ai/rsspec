// Placeholder test bodies use `assert!(true)` or simple arithmetic assertions
// to demonstrate async hook and decorator wiring.
#![allow(clippy::assertions_on_constants, clippy::eq_op)]

use std::sync::atomic::{AtomicU32, Ordering};

fn main() {
    rsspec::run(|ctx| {
        // =================================================================
        // Basic async_it
        // =================================================================
        ctx.describe("Async tests", |ctx| {
            ctx.async_it("runs an async test", || async {
                let value = async_add(2, 3).await;
                assert_eq!(value, 5);
            });

            ctx.async_it("supports decorators", || async {
                assert!(true);
            })
            .labels(&["async", "smoke"])
            .timeout(5000);
        });

        // =================================================================
        // Async hooks
        // =================================================================
        ctx.describe("Async hooks", |ctx| {
            static HOOK_COUNTER: AtomicU32 = AtomicU32::new(0);

            ctx.async_before_each(|| async {
                HOOK_COUNTER.fetch_add(1, Ordering::SeqCst);
            });

            ctx.async_after_each(|| async {
                // cleanup — verifies it doesn't panic
            });

            ctx.it("hook ran before this test", || {
                assert!(
                    HOOK_COUNTER.load(Ordering::SeqCst) >= 1,
                    "async_before_each should have run"
                );
            });
        });

        // =================================================================
        // Async before_all / after_all
        // =================================================================
        ctx.describe("Async before_all / after_all", |ctx| {
            static SETUP_RAN: AtomicU32 = AtomicU32::new(0);

            ctx.async_before_all(|| async {
                SETUP_RAN.fetch_add(1, Ordering::SeqCst);
            });

            ctx.async_after_all(|| async {
                // cleanup
            });

            ctx.it("before_all ran once", || {
                assert_eq!(SETUP_RAN.load(Ordering::SeqCst), 1);
            });

            ctx.it("still only ran once", || {
                assert_eq!(SETUP_RAN.load(Ordering::SeqCst), 1);
            });
        });

        // =================================================================
        // Async with retries
        // =================================================================
        ctx.describe("Async with retries", |ctx| {
            static ATTEMPTS: AtomicU32 = AtomicU32::new(0);

            ctx.async_it("retries async test", || async {
                let n = ATTEMPTS.fetch_add(1, Ordering::SeqCst);
                assert!(n >= 2, "should fail first 2 attempts, attempt {n}");
            })
            .retries(3);
        });

        // =================================================================
        // Async ordered steps
        // =================================================================
        ctx.describe("Async ordered", |ctx| {
            static STEP_COUNT: AtomicU32 = AtomicU32::new(0);

            ctx.ordered("async workflow", |oct| {
                oct.async_step("async step 1", || async {
                    STEP_COUNT.fetch_add(1, Ordering::SeqCst);
                });
                oct.async_step("async step 2", || async {
                    assert_eq!(STEP_COUNT.load(Ordering::SeqCst), 1);
                });
            });
        });

        // =================================================================
        // Async table-driven
        // =================================================================
        ctx.describe("Async table-driven", |ctx| {
            ctx.describe_table("async arithmetic")
                .case("addition", (2i32, 3i32, 5i32))
                .case("negative", (-1i32, 1i32, 0i32))
                .async_run(|data: &(i32, i32, i32)| {
                    let (a, b, expected) = *data;
                    async move {
                        assert_eq!(a + b, expected);
                    }
                });
        });

        // =================================================================
        // Mixed sync and async in same describe
        // =================================================================
        ctx.describe("Mixed sync and async", |ctx| {
            ctx.it("sync test", || {
                assert!(true);
            });

            ctx.async_it("async test", || async {
                assert!(true);
            });
        });

        // =================================================================
        // async_test() wrapper used directly
        // =================================================================
        ctx.describe("async_test wrapper", |ctx| {
            ctx.it(
                "used directly with it()",
                rsspec::async_test(|| async {
                    assert_eq!(1 + 1, 2);
                }),
            );
        });

        // =================================================================
        // Pending async tests
        // =================================================================
        ctx.describe("Pending async", |ctx| {
            ctx.async_xit("not yet implemented", || async {
                panic!("should never run");
            });
        });
    });
}

async fn async_add(a: i32, b: i32) -> i32 {
    a + b
}
