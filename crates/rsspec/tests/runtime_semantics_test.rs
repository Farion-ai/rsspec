//! Runtime-semantics regression tests — the behaviors the suite documents but
//! never proved with a failing-capable assertion.
//!
//! All are exercised through the **closure API** (no `macros`/`tokio`/`parallel`
//! features) via `run_inline`, which PANICS on any spec failure — so a failing
//! spec is observable here with `catch_unwind`, and focus/skip stay contained to
//! one suite per `#[test]`. Expected values are derived from the documented
//! contract in `Context`/runner docs, not from runner output.
//!
//! (`pending`-as-reported-status and label filtering are intentionally NOT here:
//! `run_inline` fixes `include_ignored:false`/`label_filter:None` and exposes no
//! `RunResult`, so those are only observable in the in-crate runner unit tests.)

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};

/// Build a hook/spec body that records `label` into the shared log when it runs.
fn rec(log: &Arc<Mutex<Vec<&'static str>>>, label: &'static str) -> impl Fn() + 'static {
    let log = Arc::clone(log);
    move || log.lock().expect("log mutex poisoned").push(label)
}

// ---- Hook execution ordering -------------------------------------------------

// Contract (Context docs): before_each runs outer→inner in registration order;
// just_before_each runs after ALL before_each, still outer→inner; the body runs;
// after_each runs inner→outer. This exact cross-kind sequence was never asserted
// (the old just_before_each test only checked a fixture value, hooks_test.rs:386).
#[test]
fn hooks_fire_in_documented_order_across_nested_scopes() {
    let log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let outer_log = Arc::clone(&log);

    rsspec::run_inline(move |ctx| {
        ctx.describe("outer", {
            let log = Arc::clone(&outer_log);
            move |ctx| {
                ctx.before_each(rec(&log, "be:outer"));
                ctx.just_before_each(rec(&log, "jbe:outer"));
                ctx.after_each(rec(&log, "ae:outer"));
                ctx.describe("inner", {
                    let log = Arc::clone(&log);
                    move |ctx| {
                        ctx.before_each(rec(&log, "be:inner"));
                        ctx.just_before_each(rec(&log, "jbe:inner"));
                        ctx.after_each(rec(&log, "ae:inner"));
                        ctx.it("spec", rec(&log, "body"));
                    }
                });
            }
        });
    });

    let seq = log.lock().expect("log mutex poisoned").clone();
    assert_eq!(
        seq,
        vec![
            "be:outer", "be:inner", "jbe:outer", "jbe:inner", "body", "ae:inner", "ae:outer",
        ],
        "hook order must be: before_each outer→inner, just_before_each outer→inner, \
         body, after_each inner→outer"
    );
}

// ---- Panic isolation & after_each on failure --------------------------------

// Contract: a panicking spec is reported failed but must NOT abort sibling specs,
// and after_each must still run for the panicking spec. No prior test proved the
// suite continues past a panic, nor that after_each fires on a failing body.
#[test]
fn spec_panic_runs_after_each_and_does_not_abort_siblings() {
    let after_each_runs = Arc::new(AtomicUsize::new(0));
    let sibling_ran = Arc::new(AtomicBool::new(false));
    let ae = Arc::clone(&after_each_runs);
    let sr = Arc::clone(&sibling_ran);

    let outcome = catch_unwind(AssertUnwindSafe(move || {
        rsspec::run_inline(move |ctx| {
            ctx.describe("group", {
                let (ae, sr) = (Arc::clone(&ae), Arc::clone(&sr));
                move |ctx| {
                    {
                        let ae = Arc::clone(&ae);
                        ctx.after_each(move || {
                            ae.fetch_add(1, SeqCst);
                        });
                    }
                    ctx.it("panics", || panic!("boom"));
                    {
                        let sr = Arc::clone(&sr);
                        ctx.it("still runs", move || sr.store(true, SeqCst));
                    }
                }
            });
        });
    }));

    assert!(outcome.is_err(), "a panicking spec must fail the suite");
    assert!(
        sibling_ran.load(SeqCst),
        "a sibling spec must still run after an earlier spec panicked"
    );
    assert_eq!(
        after_each_runs.load(SeqCst),
        2,
        "after_each must run for BOTH the panicking and the passing spec"
    );
}

// ---- Decorator failure branches (closure API) -------------------------------

// Contract: retries(n) makes n additional attempts; a spec failing every attempt
// fails the suite. Prior tests only covered retries that eventually pass.
#[test]
fn retries_exhausted_fails_the_spec() {
    static ATTEMPTS: AtomicU32 = AtomicU32::new(0);
    ATTEMPTS.store(0, SeqCst);

    let outcome = catch_unwind(AssertUnwindSafe(|| {
        rsspec::run_inline(|ctx| {
            ctx.it("always fails", || {
                ATTEMPTS.fetch_add(1, SeqCst);
                panic!("never passes");
            })
            .retries(2);
        });
    }));

    assert!(outcome.is_err(), "a spec failing every attempt must fail the suite");
    assert_eq!(
        ATTEMPTS.load(SeqCst),
        3,
        "retries(2) means 1 initial attempt + 2 retries = 3 runs"
    );
}

// Contract: timeout(ms) fails a spec whose body exceeds the deadline. Prior
// closure-API tests registered timeout on instant bodies (never tripped).
#[test]
fn timeout_fails_a_slow_spec() {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        rsspec::run_inline(|ctx| {
            ctx.it("too slow", || {
                std::thread::sleep(std::time::Duration::from_millis(50));
            })
            .timeout(1);
        });
    }));

    assert!(
        outcome.is_err(),
        "a body exceeding its timeout must fail the suite"
    );
}

// Contract: must_pass_repeatedly(n) requires n consecutive passes and stops at
// the first failure. Prior closure-API tests used always-true bodies that could
// not distinguish "ran n times" from "ran once".
#[test]
fn must_pass_repeatedly_fails_on_a_flaky_attempt() {
    static ATTEMPTS: AtomicU32 = AtomicU32::new(0);
    ATTEMPTS.store(0, SeqCst);

    let outcome = catch_unwind(AssertUnwindSafe(|| {
        rsspec::run_inline(|ctx| {
            ctx.it("flaky", || {
                let n = ATTEMPTS.fetch_add(1, SeqCst);
                assert_eq!(n, 0, "passes the first time, fails the second required pass");
            })
            .must_pass_repeatedly(3);
        });
    }));

    assert!(
        outcome.is_err(),
        "must_pass_repeatedly must fail when a later required pass fails"
    );
    assert_eq!(
        ATTEMPTS.load(SeqCst),
        2,
        "execution stops at the first failing repeat: 1 pass + 1 fail"
    );
}

// ---- Focus (closure API) ----------------------------------------------------

// Contract: a focused spec (`fit`) runs while non-focused siblings are skipped.
// Focus was tested only through the macro layer; the closure `ctx.fit` path had
// no coverage.
#[test]
fn fit_focuses_and_skips_non_focused_siblings() {
    let focused_ran = Arc::new(AtomicBool::new(false));
    let sibling_ran = Arc::new(AtomicBool::new(false));
    let fr = Arc::clone(&focused_ran);
    let sr = Arc::clone(&sibling_ran);

    rsspec::run_inline(move |ctx| {
        ctx.describe("group", {
            let (fr, sr) = (Arc::clone(&fr), Arc::clone(&sr));
            move |ctx| {
                {
                    let fr = Arc::clone(&fr);
                    ctx.fit("focused", move || fr.store(true, SeqCst));
                }
                {
                    let sr = Arc::clone(&sr);
                    ctx.it("not focused", move || sr.store(true, SeqCst));
                }
            }
        });
    });

    assert!(focused_ran.load(SeqCst), "the focused spec must run");
    assert!(
        !sibling_ran.load(SeqCst),
        "non-focused siblings must be skipped while focus is active"
    );
}

// Contract: `fdescribe` focuses its whole subtree; specs in sibling describes are
// skipped.
#[test]
fn fdescribe_focuses_its_subtree_and_skips_sibling_describes() {
    let inside_ran = Arc::new(AtomicBool::new(false));
    let outside_ran = Arc::new(AtomicBool::new(false));
    let ir = Arc::clone(&inside_ran);
    let or = Arc::clone(&outside_ran);

    rsspec::run_inline(move |ctx| {
        {
            let ir = Arc::clone(&ir);
            ctx.fdescribe("focused group", move |ctx| {
                let ir = Arc::clone(&ir);
                ctx.it("runs", move || ir.store(true, SeqCst));
            });
        }
        {
            let or = Arc::clone(&or);
            ctx.describe("other group", move |ctx| {
                let or = Arc::clone(&or);
                ctx.it("skipped", move || or.store(true, SeqCst));
            });
        }
    });

    assert!(inside_ran.load(SeqCst), "specs inside the focused describe must run");
    assert!(
        !outside_ran.load(SeqCst),
        "specs in sibling describes must be skipped"
    );
}

// ---- defer_cleanup ordering -------------------------------------------------

// Contract (defer_cleanup docs): cleanups run LIFO (last-registered-first), each
// once. The old test (utilities_test.rs) registered ONE cleanup and asserted
// `>= 1` — proving neither order nor count.
#[test]
fn defer_cleanup_runs_in_lifo_order() {
    let order = Arc::new(Mutex::new(Vec::<i32>::new()));
    let o = Arc::clone(&order);

    rsspec::run_inline(move |ctx| {
        let o = Arc::clone(&o);
        ctx.it("registers three cleanups", move || {
            for n in 1..=3 {
                let o = Arc::clone(&o);
                rsspec::defer_cleanup(move || o.lock().expect("order mutex poisoned").push(n));
            }
        });
    });

    let seq = order.lock().expect("order mutex poisoned").clone();
    assert_eq!(seq, vec![3, 2, 1], "deferred cleanups must run last-registered-first");
}

// ---- skip! macro ------------------------------------------------------------

// Contract: the `skip!` macro skips the current spec at runtime and returns early
// (so code after it does not run), WITHOUT failing the suite. The in-crate unit
// test could only call `skip()` directly ("can't use the macro in a Fn closure");
// the actual `skip!` macro (which resolves `rsspec::skip`) had zero coverage.
#[test]
fn skip_macro_returns_early_without_failing() {
    let reached_before = Arc::new(AtomicBool::new(false));
    let reached_after = Arc::new(AtomicBool::new(false));
    let before = Arc::clone(&reached_before);
    let after = Arc::clone(&reached_after);

    // run_inline panics on failure; reaching here without panicking proves a
    // skipped spec is not a failure.
    rsspec::run_inline(move |ctx| {
        let (before, after) = (Arc::clone(&before), Arc::clone(&after));
        ctx.it("skips midway", move || {
            before.store(true, SeqCst);
            // black_box keeps the early return out of the `unreachable_code` lint
            // while still always firing at runtime.
            if std::hint::black_box(true) {
                rsspec::skip!("not ready yet");
            }
            after.store(true, SeqCst);
        });
    });

    assert!(reached_before.load(SeqCst), "the spec body runs up to skip!");
    assert!(
        !reached_after.load(SeqCst),
        "skip! must return early — code after it must not run"
    );
}
