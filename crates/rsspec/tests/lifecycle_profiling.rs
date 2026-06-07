//! Profiling / diagnostic tests for lifecycle-hook invocation counts.
//!
//! These pin down *how many times* `before_all` and `before_each` actually run
//! across nested `context` blocks — the question behind "why are the seam tests
//! slow?". The invariant under test:
//!
//! - `before_all` runs **exactly once** per `describe`, no matter how many
//!   nested contexts or specs sit beneath it.
//! - `before_each` runs **once per spec** (the common cause of an expensive act
//!   executing N times instead of once).
//! - Sibling `describe`s each run their **own** `before_all` — so N specs
//!   registered as N describes means N acts, even against the same fixture.

use std::sync::atomic::{AtomicU32, Ordering::SeqCst};

#[test]
fn before_all_runs_once_across_nested_contexts() {
    static BEFORE_ALL: AtomicU32 = AtomicU32::new(0);
    static SPECS: AtomicU32 = AtomicU32::new(0);
    BEFORE_ALL.store(0, SeqCst);
    SPECS.store(0, SeqCst);

    rsspec::run_inline(|ctx| {
        ctx.describe("expensive parent", |ctx| {
            // The "act": must run exactly once for the whole subtree.
            ctx.before_all(|| {
                BEFORE_ALL.fetch_add(1, SeqCst);
            });

            ctx.context("context a", |ctx| {
                ctx.it("a1", || {
                    SPECS.fetch_add(1, SeqCst);
                });
                ctx.it("a2", || {
                    SPECS.fetch_add(1, SeqCst);
                });
            });

            ctx.context("context b", |ctx| {
                ctx.it("b1", || {
                    SPECS.fetch_add(1, SeqCst);
                });
                ctx.context("deeply nested c", |ctx| {
                    ctx.it("c1", || {
                        SPECS.fetch_add(1, SeqCst);
                    });
                    ctx.it("c2", || {
                        SPECS.fetch_add(1, SeqCst);
                    });
                });
            });
        });
    });

    assert_eq!(SPECS.load(SeqCst), 5, "all five specs ran");
    assert_eq!(
        BEFORE_ALL.load(SeqCst),
        1,
        "before_all in the parent describe must run exactly once, \
         regardless of nested contexts/specs"
    );
}

#[test]
fn before_each_runs_once_per_spec_not_once_per_scope() {
    static BEFORE_EACH: AtomicU32 = AtomicU32::new(0);
    BEFORE_EACH.store(0, SeqCst);

    rsspec::run_inline(|ctx| {
        ctx.describe("parent", |ctx| {
            // A `before_each` here runs for EVERY spec below — if the expensive
            // act lands here instead of in `before_all`, it executes N times.
            ctx.before_each(|| {
                BEFORE_EACH.fetch_add(1, SeqCst);
            });
            ctx.context("a", |ctx| {
                ctx.it("a1", || {});
                ctx.it("a2", || {});
            });
            ctx.context("b", |ctx| {
                ctx.it("b1", || {});
            });
        });
    });

    assert_eq!(
        BEFORE_EACH.load(SeqCst),
        3,
        "before_each runs once per spec (3), not once per scope — \
         use before_all for an expensive one-time act"
    );
}

#[test]
fn sibling_describes_each_run_their_own_before_all() {
    // Mirrors the manifest pattern: many specs registered as separate describes,
    // each with its own before_all. The act runs once PER describe — so N specs
    // registered as N describes = N acts, even against the same fixture.
    static ACTS: AtomicU32 = AtomicU32::new(0);
    ACTS.store(0, SeqCst);

    rsspec::run_inline(|ctx| {
        for name in ["spec one", "spec two", "spec three"] {
            ctx.describe(name, |ctx| {
                ctx.before_all(|| {
                    ACTS.fetch_add(1, SeqCst);
                });
                ctx.it("asserts", || {});
            });
        }
    });

    assert_eq!(
        ACTS.load(SeqCst),
        3,
        "each sibling describe runs its own before_all — N describes = N acts; \
         hoist shared setup to a common parent describe to act once per fixture"
    );
}
