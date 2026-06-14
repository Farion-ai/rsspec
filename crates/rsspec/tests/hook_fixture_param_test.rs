//! Hooks read parent fixtures via `&T` params — the same way `it(|v: &T|)` does.
//!
//! A returning `before_all`/`before_each` writes a fixture (keyed by type); these
//! tests assert that hooks can also *read* a fixture declared in an enclosing
//! scope, so an inner `before_all` can derive its own value from an outer one and
//! `after_all`/`just_before_each` can use it. `run_inline` panics on any spec
//! failure, so a mis-wired fixture fails the enclosing `#[test]`.

use std::sync::{Arc, Mutex};

// The canonical "act once, assert often" seam: an expensive fixture is built once
// by the outer `before_all`, then a nested `before_all` reads it to precompute
// per-context results that the `it` blocks assert on.
#[test]
fn inner_before_all_reads_outer_fixture_and_derives_results() {
    struct TestEnv {
        base: u32,
    }
    impl TestEnv {
        fn lookup(&self, n: u32) -> u32 {
            self.base + n
        }
    }
    struct CoreResults {
        affected: u32,
        fixed: u32,
    }

    rsspec::run_inline(|ctx| {
        ctx.describe("PURL Lookup", |ctx| {
            ctx.before_all(|| -> TestEnv { TestEnv { base: 100 } });

            ctx.describe("core lookup", |ctx| {
                ctx.before_all(|env: &TestEnv| -> CoreResults {
                    CoreResults {
                        affected: env.lookup(1),
                        fixed: env.lookup(0),
                    }
                });

                ctx.it("derives affected from the env", |r: &CoreResults| {
                    assert_eq!(r.affected, 101);
                });
                ctx.it("derives fixed from the env", |r: &CoreResults| {
                    assert_eq!(r.fixed, 100);
                });
            });
        });
    });
}

// `after_all` reads the scope's `before_all` fixture for teardown. It has no
// return, so we observe the read through a side channel.
#[test]
fn after_all_reads_scope_fixture() {
    struct TestEnv {
        id: u32,
    }

    let seen = Arc::new(Mutex::new(Vec::<u32>::new()));
    let seen_hook = Arc::clone(&seen);

    rsspec::run_inline(move |ctx| {
        ctx.describe("env", |ctx| {
            ctx.before_all(|| -> TestEnv { TestEnv { id: 7 } });
            ctx.it("uses the env", |_e: &TestEnv| {});
            ctx.after_all(move |e: &TestEnv| {
                seen_hook.lock().unwrap().push(e.id);
            });
        });
    });

    assert_eq!(*seen.lock().unwrap(), vec![7], "after_all saw the fixture");
}

// `before_each` reads a `before_all` value (cross-store: per-test SETUP_STORE
// misses, scope stack hits) and derives a fresh per-test fixture from it.
#[test]
fn before_each_reads_before_all_fixture() {
    struct Env {
        base: u32,
    }
    struct Conn {
        port: u32,
    }

    rsspec::run_inline(|ctx| {
        ctx.describe("db", |ctx| {
            ctx.before_all(|| -> Env { Env { base: 5432 } });
            ctx.before_each(|env: &Env| -> Conn { Conn { port: env.base + 1 } });
            ctx.it("conn is derived from env", |c: &Conn| {
                assert_eq!(c.port, 5433);
            });
        });
    });
}

// `just_before_each` reads the fixture and runs once per spec.
#[test]
fn just_before_each_reads_fixture_once_per_spec() {
    struct Env {
        id: u32,
    }

    let seen = Arc::new(Mutex::new(Vec::<u32>::new()));
    let seen_hook = Arc::clone(&seen);

    rsspec::run_inline(move |ctx| {
        ctx.describe("x", |ctx| {
            ctx.before_all(|| -> Env { Env { id: 3 } });
            ctx.just_before_each(move |e: &Env| {
                seen_hook.lock().unwrap().push(e.id);
            });
            ctx.it("a", || {});
            ctx.it("b", || {});
        });
    });

    assert_eq!(*seen.lock().unwrap(), vec![3, 3], "ran once per spec");
}

// Backward-compat: the no-parameter forms (returning and side-effect-only) keep
// compiling and behaving exactly as before.
#[test]
fn no_param_hooks_still_work() {
    let after = Arc::new(Mutex::new(false));
    let after_hook = Arc::clone(&after);

    rsspec::run_inline(move |ctx| {
        ctx.describe("compat", |ctx| {
            ctx.before_all(|| -> String { "cfg".to_string() });
            ctx.before_each(|| { /* side effect only */ });
            ctx.after_all(move || {
                *after_hook.lock().unwrap() = true;
            });
            ctx.it("reads the returning fixture", |c: &String| {
                assert_eq!(c, "cfg");
            });
        });
    });

    assert!(*after.lock().unwrap(), "no-arg after_all ran");
}

// Declaring `&T` for a fixture that no scope provides is an error — the existing
// lookup panic ("no value of type ...") fires, surfaced by run_inline.
#[test]
#[should_panic(expected = "no value of type")]
fn reading_absent_fixture_panics() {
    struct Absent;

    rsspec::run_inline(|ctx| {
        ctx.describe("x", |ctx| {
            ctx.before_all(|_a: &Absent| -> u32 { 0 });
            ctx.it("never reached", |_n: &u32| {});
        });
    });
}
