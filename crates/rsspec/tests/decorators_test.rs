// Placeholder test bodies use `assert!(true)` to show the hook / decorator wires up correctly.
#![allow(clippy::assertions_on_constants)]

use std::sync::atomic::{AtomicU32, Ordering};

fn main() {
    rsspec::run(|ctx| {
        // =================================================================
        // decorators — labels, retries, timeout, must_pass_repeatedly
        // =================================================================
        ctx.describe("decorators", |ctx| {
            ctx.it("supports labels for filtering", || {
                assert!(true);
            })
            .labels(&["smoke", "fast"]);

            // Static needed: counts across retry attempts within one test
            static RETRY_COUNT: AtomicU32 = AtomicU32::new(0);

            ctx.it("retries on failure up to N times", || {
                let n = RETRY_COUNT.fetch_add(1, Ordering::SeqCst);
                assert!(n >= 2, "should fail first 2 attempts");
            })
            .retries(3);

            ctx.it("requires N consecutive passes", || {
                assert!(true);
            })
            .must_pass_repeatedly(3);

            ctx.it("fails if execution exceeds timeout", || {
                assert!(true);
            })
            .timeout(5000);
        });

        // =================================================================
        // table-driven — parameterized tests
        // =================================================================
        ctx.describe("table-driven tests", |ctx| {
            ctx.describe_table("addition")
                .case("positive", (2i32, 3i32, 5i32))
                .case("large", (100i32, 200i32, 300i32))
                .case("negative", (-1i32, 1i32, 0i32))
                .run(|(a, b, expected): &(i32, i32, i32)| {
                    assert_eq!(a + b, *expected);
                });
        });

        // =================================================================
        // ordered — sequential steps that fail fast
        // =================================================================
        ctx.describe("ordered tests", |ctx| {
            // Static needed: counts across sequential steps
            static STEPS: AtomicU32 = AtomicU32::new(0);

            ctx.ordered("runs steps in sequence", |oct| {
                oct.step("step 1", || {
                    STEPS.fetch_add(1, Ordering::SeqCst);
                });
                oct.step("step 2", || {
                    STEPS.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(STEPS.load(Ordering::SeqCst), 2);
                });
            });
        });
    });
}
