//! Behaviour tests for the optional macro layer.
//!
//! Each macro lowers to the same closure-runtime calls as the hand-written API,
//! so these assert the OBSERVABLE behaviour: specs run, `before_all` fixtures
//! flow into `it` bodies, hooks fire, `xit` skips, `fit` focuses, decorators
//! apply, and the bool-assertion sugar actually asserts. `run_inline` panics on
//! any spec failure, so a mis-lowered spec fails the enclosing `#[test]`.

use rsspec::{after_all, before_all, before_each, context, describe, fit, it, xit};
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
use std::sync::{Arc, Mutex};

#[test]
fn describe_context_it_run_nested_specs() {
    static RAN: AtomicU32 = AtomicU32::new(0);
    RAN.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("calculator", {
            it!("adds", {
                assert_eq!(2 + 3, 5);
                RAN.fetch_add(1, SeqCst);
            });
            context!("with negatives", {
                it!("handles them", {
                    assert_eq!(-1 + 1, 0);
                    RAN.fetch_add(1, SeqCst);
                });
            });
        });
    });

    assert_eq!(RAN.load(SeqCst), 2, "both nested specs ran");
}

#[test]
fn before_all_fixture_flows_into_it() {
    rsspec::run_inline(|_| {
        describe!("api", {
            before_all!(cfg: String = format!("shared-{}", 42));
            it!("receives the fixture by &T", |cfg: &String| assert_eq!(
                cfg,
                "shared-42"
            ));
        });
    });
}

#[test]
fn before_all_macro_reads_parent_fixture() {
    struct Env {
        base: u32,
    }
    struct Results {
        v: u32,
    }

    rsspec::run_inline(|_| {
        describe!("outer", {
            before_all!(env: Env = Env { base: 10 });
            describe!("inner", {
                before_all!(|env: &Env| -> Results { Results { v: env.base + 5 } });
                it!("derives from the parent fixture", |r: &Results| assert_eq!(r.v, 15));
            });
        });
    });
}

#[test]
fn after_all_macro_reads_fixture() {
    struct Env {
        id: u32,
    }

    let seen = Arc::new(Mutex::new(Vec::<u32>::new()));
    let seen_hook = Arc::clone(&seen);

    rsspec::run_inline(move |_| {
        describe!("x", {
            before_all!(env: Env = Env { id: 9 });
            it!("uses env", |_e: &Env| assert!(true));
            after_all!(|e: &Env| {
                seen_hook.lock().unwrap().push(e.id);
            });
        });
    });

    assert_eq!(*seen.lock().unwrap(), vec![9]);
}

#[test]
fn before_each_runs_once_per_spec() {
    static HOOK: AtomicU32 = AtomicU32::new(0);
    HOOK.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("hooked", {
            before_each!({
                HOOK.fetch_add(1, SeqCst);
            });
            it!("one", { assert!(1 == 1) });
            it!("two", { assert!(2 == 2) });
        });
    });

    assert_eq!(HOOK.load(SeqCst), 2, "before_each fired once per spec");
}

#[test]
fn xit_body_never_runs() {
    static RAN: AtomicU32 = AtomicU32::new(0);
    RAN.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("pending", {
            xit!("not yet", {
                RAN.fetch_add(1, SeqCst);
                panic!("xit body must not run");
            });
        });
    });

    assert_eq!(RAN.load(SeqCst), 0, "xit body skipped");
}

#[test]
fn fit_focuses_only_marked_spec() {
    static FOCUSED: AtomicU32 = AtomicU32::new(0);
    static OTHER: AtomicU32 = AtomicU32::new(0);
    FOCUSED.store(0, SeqCst);
    OTHER.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("focus", {
            fit!("runs", {
                FOCUSED.fetch_add(1, SeqCst);
            });
            it!("skipped under focus", {
                OTHER.fetch_add(1, SeqCst);
            });
        });
    });

    assert_eq!(FOCUSED.load(SeqCst), 1, "focused spec ran");
    assert_eq!(OTHER.load(SeqCst), 0, "non-focused spec skipped");
}

#[test]
fn bool_sugar_passes_on_true_expression() {
    static RAN: AtomicU32 = AtomicU32::new(0);
    RAN.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("math", {
            it!("two plus two is four", 2 + 2 == 4);
            it!("ran marker", {
                RAN.fetch_add(1, SeqCst);
            });
        });
    });

    assert_eq!(RAN.load(SeqCst), 1);
}

#[test]
fn bool_sugar_fails_on_false_expression() {
    let outcome = std::panic::catch_unwind(|| {
        rsspec::run_inline(|_| {
            describe!("math", {
                it!("one is not two", 1 == 2);
            });
        });
    });

    assert!(
        outcome.is_err(),
        "a false bool-sugar expression must fail the spec (the macro must really assert)"
    );
}

#[test]
fn retries_decorator_recovers_a_flaky_spec() {
    static ATTEMPTS: AtomicU32 = AtomicU32::new(0);
    ATTEMPTS.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("flaky", {
            it!(
                "passes on the third attempt",
                {
                    let n = ATTEMPTS.fetch_add(1, SeqCst);
                    assert!(n >= 2, "fail the first two attempts");
                },
                retries = 3
            );
        });
    });

    assert_eq!(ATTEMPTS.load(SeqCst), 3, "retried until it passed");
}

#[test]
fn tags_decorator_lowers_to_labels() {
    // No filter set, so the spec runs; this asserts the `tags = [...]` arm lowers
    // to `.labels(...)` and compiles/registers without error.
    static RAN: AtomicU32 = AtomicU32::new(0);
    RAN.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("tagged", {
            it!(
                "carries labels",
                {
                    RAN.fetch_add(1, SeqCst);
                },
                tags = ["integration", "slow"]
            );
        });
    });

    assert_eq!(RAN.load(SeqCst), 1);
}

#[test]
fn it_arm_dispatch_is_pinned() {
    // Pins `__it_impl!` arm order. The block body's value is `()`; if the greedy
    // `$cond:expr` arm ever shadowed the block arm, it would expand to
    // `assert!(())` — a compile error. So this test is the canary for a silent
    // dispatch regression (block vs. fixture vs. bool).
    static BLOCK_RAN: AtomicU32 = AtomicU32::new(0);
    BLOCK_RAN.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("arm dispatch", {
            before_all!(n: u32 = 7);
            it!("block arm runs statements", {
                let collected: Vec<u32> = (0..3).collect();
                assert_eq!(collected.len(), 3);
                BLOCK_RAN.fetch_add(1, SeqCst);
            });
            it!("fixture arm reads &T", |n: &u32| assert_eq!(*n, 7));
            it!("cond arm asserts a bool", 1 + 1 == 2);
        });
    });

    assert_eq!(
        BLOCK_RAN.load(SeqCst),
        1,
        "the block arm executed its statements"
    );
}

#[cfg(feature = "tokio")]
#[test]
fn async_it_arm_runs_on_a_runtime() {
    static RAN: AtomicU32 = AtomicU32::new(0);
    RAN.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("async", {
            it!("awaits a future", async {
                let v = async_add(2, 3).await;
                assert_eq!(v, 5);
                RAN.fetch_add(1, SeqCst);
            });
        });
    });

    assert_eq!(RAN.load(SeqCst), 1);
}

#[cfg(feature = "tokio")]
async fn async_add(a: i32, b: i32) -> i32 {
    a + b
}
