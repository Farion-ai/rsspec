// End-to-end check for the `parallel` feature.
//
// Proves that the public API's `Send` bounds compile for ordinary (Send)
// closures and that the parallel run path produces correct results. Run with
// `RSSPEC_PARALLEL=4` to exercise the worker pool; with default env it runs
// sequentially. Either way the assertions below must hold, so this binary is
// safe under both `cargo test` (sequential) and an explicit parallel run.
//
// The rigorous "actual concurrency happens" proof lives in the unit tests in
// src/runner.rs, where the worker count is controlled directly.
#![allow(clippy::assertions_on_constants)]

use std::sync::atomic::{AtomicU32, Ordering};

fn main() {
    rsspec::run(|ctx| {
        // Each top-level describe is an independent unit of parallelism. They
        // must not share mutable state that depends on execution order.

        ctx.describe("Alpha — fixtures via before_all", |ctx| {
            ctx.before_all(|| -> String { "alpha-fixture".to_string() });

            ctx.it("receives the before_all value", |fixture: &String| {
                assert_eq!(fixture, "alpha-fixture");
            });

            ctx.it("still sees the same value", |fixture: &String| {
                assert_eq!(fixture, "alpha-fixture");
            });
        });

        ctx.describe("Beta — before_each fixture + after_each", |ctx| {
            static AFTER_EACH_RUNS: AtomicU32 = AtomicU32::new(0);

            ctx.before_each(|| -> i32 { 21 * 2 });
            ctx.after_each(|| {
                AFTER_EACH_RUNS.fetch_add(1, Ordering::SeqCst);
            });

            ctx.it("computes the fixture", |answer: &i32| {
                assert_eq!(*answer, 42);
            });

            ctx.it("gets a fresh fixture each time", |answer: &i32| {
                assert_eq!(*answer, 42);
            });
        });

        ctx.describe("Gamma — nested scopes", |ctx| {
            ctx.context("with a sub-context", |ctx| {
                ctx.it("runs nested tests", || {
                    let xs = [1, 2, 3];
                    assert_eq!(xs.iter().sum::<i32>(), 6);
                });
            });
        });

        ctx.describe("Delta — ordered steps", |ctx| {
            static STEP_COUNT: AtomicU32 = AtomicU32::new(0);
            ctx.ordered("a workflow", |oct| {
                oct.step("step one", || {
                    STEP_COUNT.fetch_add(1, Ordering::SeqCst);
                });
                oct.step("step two", || {
                    assert_eq!(STEP_COUNT.load(Ordering::SeqCst), 1);
                });
            });
        });

        ctx.describe("Epsilon — table-driven", |ctx| {
            ctx.describe_table("arithmetic")
                .case("addition", (2i32, 3i32, 5i32))
                .case("negatives", (-2i32, -3i32, -5i32))
                .run(|(a, b, expected): &(i32, i32, i32)| {
                    assert_eq!(a + b, *expected);
                });
        });
    });
}
