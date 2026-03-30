use std::sync::atomic::{AtomicU32, Ordering};

fn main() {
    rsspec::run(|ctx| {
        // =================================================================
        // defer_cleanup — LIFO cleanup after test
        // =================================================================
        ctx.describe("defer_cleanup", |ctx| {
            // Static needed: verifies cleanup ran across tests
            static CLEANUP_RAN: AtomicU32 = AtomicU32::new(0);

            ctx.it("registers a cleanup function", || {
                rsspec::defer_cleanup(|| {
                    CLEANUP_RAN.fetch_add(1, Ordering::SeqCst);
                });
            });

            ctx.it("runs cleanup after the previous test", || {
                assert!(CLEANUP_RAN.load(Ordering::SeqCst) >= 1);
            });
        });

        // =================================================================
        // by() — step documentation within a test
        // =================================================================
        ctx.describe("by()", |ctx| {
            ctx.it("documents steps within a test", || {
                rsspec::by("setting up");
                let x = 42;
                rsspec::by("verifying");
                assert_eq!(x, 42);
            });
        });
    });
}
