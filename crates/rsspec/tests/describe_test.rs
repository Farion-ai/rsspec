// These tests are intentionally simple arithmetic examples demonstrating the framework.
// The constant-folding and eq_op lints fire on pedagogical assertions like `assert_eq!(2+3, 5)`.
#![allow(clippy::assertions_on_constants, clippy::eq_op)]

fn main() {
    rsspec::run(|ctx| {
        // =================================================================
        // describe / context / when — grouping tests
        // =================================================================
        ctx.describe("describe, context, when", |ctx| {
            ctx.it("groups tests under describe", || {
                assert_eq!(2 + 3, 5);
            });

            ctx.context("nested with context", |ctx| {
                ctx.it("inherits the outer scope", || {
                    assert_eq!(-1 + 3, 2);
                });
            });

            ctx.when("using when as alias", |ctx| {
                ctx.it("behaves like context", || {
                    assert_eq!(10 / 2, 5);
                });
            });
        });

        // =================================================================
        // it / specify — test definitions
        // =================================================================
        ctx.describe("it and specify", |ctx| {
            ctx.it("defines a test case", || {
                assert_eq!(3 * 4, 12);
            });

            ctx.specify("is an alias for it", || {
                assert!(true);
            });
        });

        // =================================================================
        // pending — xit / xdescribe skip test execution
        // =================================================================
        ctx.describe("pending tests", |ctx| {
            ctx.xit("skips individual tests with xit", || {
                panic!("should never run");
            });

            ctx.xdescribe("skips entire groups with xdescribe", |ctx| {
                ctx.it("is also pending", || {
                    panic!("should never run");
                });
            });
        });

        // =================================================================
        // labels on describe — inherited by children
        // =================================================================
        ctx.describe("describe-level labels", |ctx| {
            ctx.labels(&["integration"]);

            ctx.it("are inherited by child tests", || {
                assert!(true);
            });
        });
    });
}
