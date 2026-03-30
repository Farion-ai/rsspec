#![forbid(unsafe_code)]
//! # rsspec — A Ginkgo/RSpec-inspired BDD testing framework for Rust
//!
//! Write expressive, structured tests using a closure-based API with
//! `describe`, `context`, `it`, lifecycle hooks, table-driven tests, and more.
//!
//! ## Quick example
//!
//! ```rust,no_run
//! rsspec::run(|ctx| {
//!     ctx.describe("Calculator", |ctx| {
//!         ctx.it("adds two numbers", || {
//!             assert_eq!(2 + 3, 5);
//!         });
//!
//!         ctx.context("with negative numbers", |ctx| {
//!             ctx.it("handles negatives", || {
//!                 assert_eq!(-1 + 1, 0);
//!             });
//!         });
//!     });
//! });
//! ```
//!
//! ## Features
//!
//! - `googletest` — re-exports `googletest` matchers via `rsspec::matchers`
//! - `tokio` — async test support via `async_it`, `async_before_each`, etc.

pub(crate) mod runner;
mod context;
pub(crate) mod ordered;
pub(crate) mod table;

pub use context::{Context, ItBuilder, run, run_inline};

/// Re-export of the [`googletest`] crate. Available with the `googletest` feature.
#[cfg(feature = "googletest")]
pub use googletest;

/// Composable matchers re-exported from [`googletest::prelude`].
#[cfg(feature = "googletest")]
pub mod matchers {
    pub use googletest::prelude::*;
}

// ============================================================================
// Async test support (requires `tokio` feature)
// ============================================================================

/// Wrap an async closure into a synchronous `Fn()` for use with rsspec.
///
/// Creates a fresh single-threaded Tokio runtime per invocation, preventing
/// cross-test state leakage and working correctly with retries.
///
/// # Example
///
/// ```rust,ignore
/// ctx.it("async test", rsspec::async_test(|| async {
///     let value = fetch().await;
///     assert_eq!(value, 42);
/// }));
/// ```
#[cfg(feature = "tokio")]
pub fn async_test<F, Fut>(f: F) -> impl Fn() + 'static
where
    F: Fn() -> Fut + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rsspec: failed to build Tokio runtime");
        rt.block_on(f());
    }
}

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::cell::RefCell;

thread_local! {
    /// Per-thread flag to suppress panic output during retries.
    /// Checked by the custom panic hook installed at init time.
    static SUPPRESS_PANIC_OUTPUT: RefCell<bool> = const { RefCell::new(false) };
}

/// Install a panic hook that respects the per-thread suppression flag.
/// Called once; wraps the default hook so normal panics still print.
fn install_panic_hook() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let suppress = SUPPRESS_PANIC_OUTPUT.with(|cell| *cell.borrow());
            if !suppress {
                prev(info);
            }
        }));
    });
}

/// A drop guard that runs cleanup code even if the test panics.
pub struct Guard<F: FnOnce()> {
    f: Option<F>,
}

impl<F: FnOnce()> Guard<F> {
    /// Create a new guard that runs `f` when dropped.
    pub fn new(f: F) -> Self {
        Guard { f: Some(f) }
    }
}

impl<F: FnOnce()> Drop for Guard<F> {
    fn drop(&mut self) {
        if let Some(f) = self.f.take() {
            f();
        }
    }
}

/// Check if the current test's labels match the `RSSPEC_LABEL_FILTER` env var.
///
/// Returns `true` (run the test) if no filter is set.
pub(crate) fn check_labels(labels: &[&str]) -> bool {
    let filter = match std::env::var("RSSPEC_LABEL_FILTER") {
        Ok(f) if !f.is_empty() => f,
        _ => return true,
    };
    labels_match_filter(labels, &filter)
}

/// Check if labels match a filter string.
///
/// Filter syntax:
/// - `integration` — matches if any label equals "integration"
/// - `!slow` — excludes if any label equals "slow"
/// - `integration,smoke` — OR: matches if any positive term matches
/// - `integration+fast` — AND: all terms must match (negation supported: `integration+!slow`)
pub(crate) fn labels_match_filter(labels: &[&str], filter: &str) -> bool {
    // Reject ambiguous filters mixing AND (+) and OR (,) syntax
    if filter.contains('+') && filter.contains(',') {
        eprintln!(
            "rsspec: invalid label filter '{filter}' — cannot mix '+' (AND) and ',' (OR). \
             Use one or the other."
        );
        return false;
    }

    // AND filter: "a+b+!c" means all terms must match
    if filter.contains('+') {
        return filter.split('+').all(|term| {
            let term = term.trim();
            if let Some(negated) = term.strip_prefix('!') {
                !labels.contains(&negated)
            } else {
                labels.contains(&term)
            }
        });
    }

    // OR filter: "a,b" means any must match
    // Separate positive and negative terms: positive terms use OR, negative terms use AND
    let mut has_positive = false;
    let mut positive_match = false;

    for term in filter.split(',') {
        let term = term.trim();
        if let Some(negated) = term.strip_prefix('!') {
            // Negative terms are exclusions: if any matches, exclude the test
            if labels.contains(&negated) {
                return false;
            }
        } else {
            has_positive = true;
            if labels.contains(&term) {
                positive_match = true;
            }
        }
    }

    // If there were positive terms, at least one must have matched.
    // If only negative terms, they all passed (none excluded).
    !has_positive || positive_match
}

/// Retry a test function up to `retries` additional times on failure.
pub(crate) fn with_retries(retries: u32, f: impl Fn()) {
    install_panic_hook();

    let max_attempts = retries + 1;
    let mut last_panic = None;

    // Suppress panic output during retries — expected failures are noisy otherwise.
    // Uses a thread-local flag so parallel tests don't interfere with each other.
    SUPPRESS_PANIC_OUTPUT.with(|cell| *cell.borrow_mut() = true);

    for attempt in 1..=max_attempts {
        match catch_unwind(AssertUnwindSafe(&f)) {
            Ok(()) => {
                SUPPRESS_PANIC_OUTPUT.with(|cell| *cell.borrow_mut() = false);
                return;
            }
            Err(e) => {
                if attempt < max_attempts {
                    eprintln!("  attempt {attempt}/{max_attempts} failed, retrying...");
                }
                last_panic = Some(e);
            }
        }
    }

    SUPPRESS_PANIC_OUTPUT.with(|cell| *cell.borrow_mut() = false);

    if let Some(e) = last_panic {
        resume_unwind(e);
    }
}

/// Require a test to pass `n` consecutive times.
///
/// Panics if `n` is 0 (would be a no-op that always passes).
pub(crate) fn must_pass_repeatedly(n: u32, f: impl Fn()) {
    assert!(n > 0, "rsspec: must_pass_repeatedly requires n >= 1");
    for attempt in 1..=n {
        if let Err(e) = catch_unwind(AssertUnwindSafe(&f)) {
            eprintln!("  must_pass_repeatedly: failed on attempt {attempt}/{n}");
            resume_unwind(e);
        }
    }
}

/// Panics if `RSSPEC_FAIL_ON_FOCUS` is set and focus mode is active.
pub(crate) fn check_fail_on_focus() {
    if let Ok(val) = std::env::var("RSSPEC_FAIL_ON_FOCUS") {
        if val == "1" || val.eq_ignore_ascii_case("true") {
            panic!(
                "rsspec: focused tests detected but RSSPEC_FAIL_ON_FOCUS is set. \
                 Remove fit/fdescribe/fcontext before pushing."
            );
        }
    }
}

// ============================================================================
// Setup value store — typed return values from before_each/before_all, accessed by it
// ============================================================================

use std::any::{Any, TypeId};
use std::collections::HashMap;

thread_local! {
    /// Per-test values from returning `before_each`. Cleared between tests.
    static SETUP_STORE: RefCell<HashMap<TypeId, Box<dyn Any>>> = RefCell::new(HashMap::new());
    /// Stack of per-scope value maps from returning `before_all`.
    /// `push_scope_setup_layer()` adds a new layer on scope entry;
    /// `pop_scope_setup_layer()` removes it on scope exit.
    /// Lookup searches from top (innermost scope) to bottom (outermost scope),
    /// so inner `before_all` values shadow outer ones for the same type.
    /// Tests run single-threaded; these thread-locals are never shared across threads.
    static SCOPE_SETUP_STACK: RefCell<Vec<HashMap<TypeId, Box<dyn Any>>>> = RefCell::new(Vec::new());
}

/// Store a per-test value. Called by returning `before_each` hooks.
///
/// If two `before_each` hooks in the same scope return the same type `T`,
/// the later hook's value overwrites the earlier one (last-registered-wins).
pub(crate) fn store_setup_value<T: 'static>(val: T) {
    SETUP_STORE.with(|cell| {
        cell.borrow_mut().insert(TypeId::of::<T>(), Box::new(val));
    });
}

/// Push a new empty scope layer onto the stack.
/// Called by the runner when entering a `Describe` scope before `before_all` runs.
pub(crate) fn push_scope_setup_layer() {
    SCOPE_SETUP_STACK.with(|cell| {
        cell.borrow_mut().push(HashMap::new());
    });
}

/// Store a per-scope value into the current (innermost) scope layer.
/// Called by returning `before_all` hooks.
pub(crate) fn store_scope_setup_value<T: 'static>(val: T) {
    SCOPE_SETUP_STACK.with(|cell| {
        let mut stack = cell.borrow_mut();
        let top = stack
            .last_mut()
            .expect("rsspec: store_scope_setup_value called with no active scope — internal error");
        top.insert(TypeId::of::<T>(), Box::new(val));
    });
}

/// Borrow a setup value by type and pass it to `f`.
///
/// Lookup order:
/// 1. Per-test store (`before_each` values) — checked first
/// 2. Scope stack from innermost to outermost (`before_all` values)
///
/// Panics with a clear message if no value of type `T` is found.
pub(crate) fn with_setup_value<T: 'static, R>(f: impl FnOnce(&T) -> R) -> R {
    let tid = TypeId::of::<T>();

    let in_test_store = SETUP_STORE.with(|cell| cell.borrow().contains_key(&tid));

    if in_test_store {
        SETUP_STORE.with(|cell| {
            let store = cell.borrow();
            let val = store
                .get(&tid)
                .expect("rsspec: TypeId present but value missing — internal invariant violated")
                .downcast_ref::<T>()
                .expect("rsspec: TypeId/value type mismatch — internal invariant violated");
            f(val)
        })
    } else {
        SCOPE_SETUP_STACK.with(|cell| {
            let stack = cell.borrow();
            for layer in stack.iter().rev() {
                if let Some(boxed) = layer.get(&tid) {
                    return f(boxed
                        .downcast_ref::<T>()
                        .expect("rsspec: TypeId/value type mismatch — internal invariant violated"));
                }
            }
            panic!(
                "rsspec: no value of type `{}` — add a returning before_each or before_all",
                std::any::type_name::<T>()
            )
        })
    }
}

/// Clear per-test values. Called by the runner between tests.
pub(crate) fn clear_setup_values() {
    SETUP_STORE.with(|cell| cell.borrow_mut().clear());
}

/// Pop the current (innermost) scope layer.
/// Called by the runner when a `Describe` scope ends, after `after_all` has run.
pub(crate) fn pop_scope_setup_layer() {
    SCOPE_SETUP_STACK.with(|cell| {
        cell.borrow_mut().pop();
    });
}

// ============================================================================
// DeferCleanup — LIFO cleanup stack
// ============================================================================

thread_local! {
    static CLEANUP_STACK: RefCell<Vec<Box<dyn FnOnce()>>> = RefCell::new(Vec::new());
}

/// Register a cleanup function that will run after the current test completes.
///
/// Cleanup functions run in LIFO (last-registered-first) order.
pub fn defer_cleanup(f: impl FnOnce() + 'static) {
    CLEANUP_STACK.with(|stack| {
        stack.borrow_mut().push(Box::new(f));
    });
}

/// Run all deferred cleanup functions.
///
/// Each cleanup runs inside `catch_unwind` so that a panic in one cleanup
/// does not prevent the remaining cleanups from executing.
pub(crate) fn run_deferred_cleanups() {
    CLEANUP_STACK.with(|stack| {
        let mut cleanups: Vec<Box<dyn FnOnce()>> = stack.borrow_mut().drain(..).collect();
        cleanups.reverse();
        let mut first_panic = None;
        for cleanup in cleanups {
            if let Err(e) = catch_unwind(AssertUnwindSafe(cleanup)) {
                eprintln!("  warning: deferred cleanup panicked");
                if first_panic.is_none() {
                    first_panic = Some(e);
                }
            }
        }
        if let Some(e) = first_panic {
            resume_unwind(e);
        }
    });
}

// ============================================================================
// By — step documentation
// ============================================================================

/// Document a step within a test. Prints the step description to stderr.
pub fn by(description: &str) {
    eprintln!("  STEP: {description}");
}

// ============================================================================
// Skip — runtime test skipping
// ============================================================================

thread_local! {
    static SKIP_REASON: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Skip the current test at runtime with a reason.
///
/// Sets a thread-local flag so the runner can report the test as skipped
/// rather than passed. Use via the [`skip!`] macro, which also returns
/// from the test closure.
pub fn skip(reason: &str) {
    SKIP_REASON.with(|cell| {
        *cell.borrow_mut() = Some(reason.to_string());
    });
}

/// Check and clear the skip flag. Returns `Some(reason)` if the test was skipped.
pub(crate) fn take_skip_reason() -> Option<String> {
    SKIP_REASON.with(|cell| cell.borrow_mut().take())
}

/// Skip the current test at runtime. Prints the reason and returns from the test.
#[macro_export]
macro_rules! skip {
    ($reason:expr) => {{
        rsspec::skip($reason);
        return;
    }};
}

/// Document a step within a test (macro form).
#[macro_export]
macro_rules! by {
    ($description:expr) => {
        rsspec::by($description);
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guard_runs_on_success() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static RAN: AtomicBool = AtomicBool::new(false);

        {
            let _g = Guard::new(|| RAN.store(true, Ordering::SeqCst));
        }
        assert!(RAN.load(Ordering::SeqCst));
    }

    #[test]
    fn test_guard_runs_on_panic() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static RAN: AtomicBool = AtomicBool::new(false);

        let result = catch_unwind(AssertUnwindSafe(|| {
            let _g = Guard::new(|| RAN.store(true, Ordering::SeqCst));
            panic!("boom");
        }));
        assert!(result.is_err());
        assert!(RAN.load(Ordering::SeqCst));
    }

    // C1 regression: negation in AND filter (integration+!slow)
    #[test]
    fn test_labels_and_filter_with_negation() {
        // Has integration, not slow → should run
        assert!(labels_match_filter(&["integration", "fast"], "integration+!slow"));
        // Has integration AND slow → should be excluded
        assert!(!labels_match_filter(&["integration", "slow"], "integration+!slow"));
        // Missing integration → should be excluded
        assert!(!labels_match_filter(&["fast"], "integration+!slow"));
    }

    // C1 regression: negation in OR filter (!slow excludes)
    #[test]
    fn test_labels_or_filter_with_negation() {
        // Has integration → matches positive term
        assert!(labels_match_filter(&["integration"], "integration,!slow"));
        // Has slow → excluded by negation
        assert!(!labels_match_filter(&["slow"], "integration,!slow"));
        // Has integration + slow → excluded despite matching positive
        assert!(!labels_match_filter(&["integration", "slow"], "integration,!slow"));
        // Has only "fast" → positive "integration" not matched → excluded
        assert!(!labels_match_filter(&["fast"], "integration,!slow"));
    }

    // Pure negation filter: "!slow" — no positive terms
    #[test]
    fn test_labels_pure_negation() {
        assert!(labels_match_filter(&["fast"], "!slow"));
        assert!(!labels_match_filter(&["slow"], "!slow"));
        assert!(labels_match_filter(&[], "!slow"));
    }

    // I7 regression: mixed AND+OR filter syntax is rejected
    #[test]
    fn test_labels_mixed_and_or_rejected() {
        assert!(!labels_match_filter(&["a", "b"], "a+b,c"));
        assert!(!labels_match_filter(&["a"], "a,b+c"));
    }

    // Basic positive OR filter
    #[test]
    fn test_labels_positive_or() {
        assert!(labels_match_filter(&["integration"], "integration,smoke"));
        assert!(labels_match_filter(&["smoke"], "integration,smoke"));
        assert!(!labels_match_filter(&["fast"], "integration,smoke"));
    }

    #[test]
    fn test_with_retries_success_first_try() {
        with_retries(3, || {
            assert_eq!(1, 1);
        });
    }

    #[test]
    fn test_with_retries_eventual_success() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static ATTEMPTS: AtomicU32 = AtomicU32::new(0);
        ATTEMPTS.store(0, Ordering::SeqCst);

        with_retries(3, || {
            let n = ATTEMPTS.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                panic!("not yet");
            }
        });

        assert_eq!(ATTEMPTS.load(Ordering::SeqCst), 3);
    }
}
