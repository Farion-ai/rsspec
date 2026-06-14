//! Behaviour tests for the optional macro layer.
//!
//! Each macro lowers to the same closure-runtime calls as the hand-written API,
//! so these assert the OBSERVABLE behaviour: specs run, `before_all` fixtures
//! flow into `it` bodies, hooks fire, `xit` skips, `fit` focuses, decorators
//! apply, and a failing assertion really fails the spec. `run_inline` panics on
//! any spec failure, so a mis-lowered spec fails the enclosing `#[test]`.
//!
//! Only `describe!` is imported: `it!`/`before_all!`/`context!`/`fit!`/`xit!`/
//! `after_all!` are parsed as tokens by the `describe!` proc-macro, never invoked
//! standalone.

use rsspec::describe;
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
            it!("receives the fixture by &T", |cfg: &String| {
                assert_eq!(cfg, "shared-42");
            });
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
                it!("derives from the parent fixture", |r: &Results| {
                    assert_eq!(r.v, 15);
                });
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
fn passing_assertion_runs_the_spec() {
    static RAN: AtomicU32 = AtomicU32::new(0);
    RAN.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("math", {
            it!("two plus two is four", {
                assert_eq!(2 + 2, 4);
            });
            it!("ran marker", {
                RAN.fetch_add(1, SeqCst);
            });
        });
    });

    assert_eq!(RAN.load(SeqCst), 1);
}

#[test]
fn failing_assertion_fails_the_spec() {
    let outcome = std::panic::catch_unwind(|| {
        rsspec::run_inline(|_| {
            describe!("math", {
                it!("one is not two", {
                    assert_eq!(1, 2);
                });
            });
        });
    });

    assert!(
        outcome.is_err(),
        "a failing assertion must fail the spec (the macro must really run the body)"
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
    // The `tags = [...]` arm lowers to `.labels(...)`. Label *filtering* — a filter
    // expression including/excluding a spec — is covered end-to-end by the runner's
    // `label_filter_config_filters_specs_at_runtime`; `run_inline` applies no filter,
    // so this pins only that the arm registers a runnable, labelled spec.
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
fn fdescribe_focuses_its_whole_container() {
    // Focus must propagate to every spec in the focused container and exclude
    // sibling containers — not just a single fit! spec.
    static IN_FOCUS: AtomicU32 = AtomicU32::new(0);
    static SIBLING: AtomicU32 = AtomicU32::new(0);
    IN_FOCUS.store(0, SeqCst);
    SIBLING.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("suite", {
            fdescribe!("focused group", {
                it!("runs by container focus", {
                    IN_FOCUS.fetch_add(1, SeqCst);
                });
                context!("nested under focus", {
                    it!("also runs", {
                        IN_FOCUS.fetch_add(1, SeqCst);
                    });
                });
            });
            describe!("unfocused sibling", {
                it!("is excluded", {
                    SIBLING.fetch_add(1, SeqCst);
                });
            });
        });
    });

    assert_eq!(IN_FOCUS.load(SeqCst), 2, "all focused specs ran");
    assert_eq!(SIBLING.load(SeqCst), 0, "sibling excluded");
}

#[test]
fn xdescribe_skips_its_whole_container() {
    static RAN: AtomicU32 = AtomicU32::new(0);
    RAN.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("suite", {
            xdescribe!("pending group", {
                it!("never runs", {
                    RAN.fetch_add(1, SeqCst);
                    panic!("a spec inside xdescribe! must not run");
                });
                context!("nested deeper", {
                    it!("also never runs", {
                        RAN.fetch_add(1, SeqCst);
                        panic!("a nested spec inside xdescribe! must not run");
                    });
                });
            });
        });
    });

    assert_eq!(RAN.load(SeqCst), 0, "all specs skipped");
}

#[test]
fn timeout_decorator_fails_a_slow_spec() {
    // The timeout is checked after the body returns, so a body that sleeps past it
    // must fail the spec — proving the decorator does more than register.
    let outcome = std::panic::catch_unwind(|| {
        rsspec::run_inline(|_| {
            describe!("slow", {
                it!(
                    "exceeds its timeout",
                    {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    },
                    timeout = 1
                );
            });
        });
    });

    assert!(outcome.is_err(), "slow body fails via timeout");
}

#[test]
fn must_pass_repeatedly_fails_on_a_flaky_attempt() {
    // must_pass_repeatedly must actually re-run the body and fail the spec when a
    // later attempt fails — the branch a registration-only test never exercises.
    static ATTEMPTS: AtomicU32 = AtomicU32::new(0);
    ATTEMPTS.store(0, SeqCst);

    let outcome = std::panic::catch_unwind(|| {
        rsspec::run_inline(|_| {
            describe!("unstable", {
                it!(
                    "passes once then fails the second run",
                    {
                        let n = ATTEMPTS.fetch_add(1, SeqCst);
                        assert_eq!(n, 0, "only the first attempt passes");
                    },
                    must_pass_repeatedly = 3
                );
            });
        });
    });

    assert!(outcome.is_err(), "a failing repeat fails the spec");
    assert_eq!(ATTEMPTS.load(SeqCst), 2, "ran twice then stopped");
}

#[test]
fn it_body_forms_coexist() {
    // The three `it!` body forms must dispatch unambiguously side by side in one
    // describe: a block body that reads the fixture IMPLICITLY, an explicit
    // `|v: &T|` closure read, and a plain assertion block. A regression would
    // mis-parse one form as another.
    static BLOCK_RAN: AtomicU32 = AtomicU32::new(0);
    BLOCK_RAN.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("arm dispatch", {
            before_all!(n: u32 = 7);
            it!("block arm reads the fixture implicitly", {
                let collected: Vec<u32> = (0..3).collect();
                assert_eq!(collected.len(), 3);
                assert_eq!(*n, 7);
                BLOCK_RAN.fetch_add(1, SeqCst);
            });
            it!("closure arm reads &T", |n: &u32| assert_eq!(*n, 7));
            it!("assertion-block arm runs", {
                assert!(1 + 1 == 2);
            });
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
