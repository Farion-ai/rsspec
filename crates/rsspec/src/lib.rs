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
//! - `macros` *(default)* — `describe!`/`it!`/`before_all!` (and the rest) as a
//!   proc-macro, with implicit fixture params; opt out with
//!   `default-features = false`
//! - `googletest` — re-exports `googletest` matchers via `rsspec::matchers`
//! - `tokio` — async test support via `async_it`, `async_before_each`, etc.
//! - `parallel` — run distinct top-level subtrees on worker threads (adds a
//!   `Send` bound to test/hook closures; see `--parallel` / `RSSPEC_PARALLEL`)

mod context;
pub(crate) mod ordered;
pub(crate) mod runner;
pub(crate) mod table;

pub use context::{run, run_inline, Context, IntoBeforeHook, IntoTestBody, ItBuilder};

/// The optional macro layer — `describe!`/`context!`/`when!` (and their `f`/`x`
/// variants) plus the `it!`/`before_all!`/`before_each!`/`after_*!` forms they
/// parse. A proc-macro gives fixtures declared with `before_all!(name: T = …)` an
/// implicit `name` binding in later bodies. On by default; opt out with
/// `default-features = false`.
#[cfg(feature = "macros")]
pub use rsspec_macros::{
    context, describe, fcontext, fdescribe, fwhen, when, xcontext, xdescribe, xwhen,
};

/// Free-function backing for the optional macro layer (`describe!`, `it!`, …).
///
/// Not a stable API: these mirror [`Context`] methods but take no `ctx`, so the
/// macros never thread a context token — nesting is handled by the same
/// thread-local builder the closure API uses. Hidden from docs.
#[doc(hidden)]
pub mod __rt {
    use crate::context::{self, Context};

    /// Backing for `describe!` / `context!` / `fdescribe!` / `xdescribe!`.
    pub fn describe(name: &str, focused: bool, pending: bool, body: impl FnOnce()) {
        context::push_group(name, focused, pending);
        body();
        context::pop_group();
    }

    pub fn it<M>(name: &str, body: impl crate::IntoTestBody<M> + 'static) -> crate::ItBuilder {
        Context.it(name, body)
    }
    pub fn fit<M>(name: &str, body: impl crate::IntoTestBody<M> + 'static) -> crate::ItBuilder {
        Context.fit(name, body)
    }
    pub fn xit<M>(name: &str, body: impl crate::IntoTestBody<M> + 'static) -> crate::ItBuilder {
        Context.xit(name, body)
    }

    pub fn before_each<M>(hook: impl crate::IntoBeforeHook<M> + 'static) {
        Context.before_each(hook);
    }
    pub fn before_all<M>(hook: impl crate::IntoBeforeHook<M> + 'static) {
        Context.before_all(hook);
    }
    pub fn after_each<M>(hook: impl crate::IntoTestBody<M> + 'static) {
        Context.after_each(hook);
    }
    pub fn after_all<M>(hook: impl crate::IntoTestBody<M> + 'static) {
        Context.after_all(hook);
    }
    pub fn just_before_each<M>(hook: impl crate::IntoTestBody<M> + 'static) {
        Context.just_before_each(hook);
    }

    /// Read an in-scope fixture by type — backs the macro layer's implicit
    /// parameters. A thin typed alias over `with_setup_value`.
    pub fn with_fixture<T: 'static, R>(f: impl FnOnce(&T) -> R) -> R {
        crate::with_setup_value::<T, R>(f)
    }

    /// Wrap an `async { … }` spec/hook body into a `Fn()`. Backs the `it!` async arm.
    #[cfg(feature = "tokio")]
    pub fn async_test<F, Fut>(f: F) -> impl Fn() + 'static
    where
        F: Fn() -> Fut + crate::MaybeSend + 'static,
        Fut: std::future::Future<Output = ()> + 'static,
    {
        crate::async_test(f)
    }
}

/// Re-export of the [`googletest`] crate. Available with the `googletest` feature.
#[cfg(feature = "googletest")]
pub use googletest;

/// Composable matchers re-exported from [`googletest::prelude`].
#[cfg(feature = "googletest")]
pub mod matchers {
    pub use googletest::prelude::*;
}

// ============================================================================
// Send gating for parallel execution
// ============================================================================
//
// A `dyn Fn` trait object bakes its auto-traits into its type, so the `Send`
// requirement cannot depend on a runtime `parallelism` value — only on a
// compile-time `cfg`. We therefore gate it behind the `parallel` feature.
//
// With the feature **off**, `MaybeSend`/`MaybeSendSync` are blanket no-ops and
// `TestBody` is `Box<dyn Fn()>` — the API is byte-for-byte what it was before,
// and `!Send` test bodies (capturing `Rc`/`RefCell`) still compile.
//
// With the feature **on**, the markers force `Send`/`Send + Sync` and test/hook
// closures must be `Send` so they can be moved onto worker threads.

// These markers are `pub` only because they appear in the bounds of public
// methods — users never name them. They are `#[doc(hidden)]` (and not an
// extension point) precisely because their definition flips with the `parallel`
// cfg; writing `where F: rsspec::MaybeSend` would couple user code to that flip.

/// Marker requiring `Send` when the `parallel` feature is enabled, otherwise a no-op.
#[doc(hidden)]
#[cfg(feature = "parallel")]
pub trait MaybeSend: Send {}
#[cfg(feature = "parallel")]
impl<T: Send> MaybeSend for T {}
/// Marker requiring `Send` when the `parallel` feature is enabled, otherwise a no-op.
#[doc(hidden)]
#[cfg(not(feature = "parallel"))]
pub trait MaybeSend {}
#[cfg(not(feature = "parallel"))]
impl<T> MaybeSend for T {}

/// Marker requiring `Send + Sync` when the `parallel` feature is enabled, otherwise a no-op.
#[doc(hidden)]
#[cfg(feature = "parallel")]
pub trait MaybeSendSync: Send + Sync {}
#[cfg(feature = "parallel")]
impl<T: Send + Sync> MaybeSendSync for T {}
/// Marker requiring `Send + Sync` when the `parallel` feature is enabled, otherwise a no-op.
#[doc(hidden)]
#[cfg(not(feature = "parallel"))]
pub trait MaybeSendSync {}
#[cfg(not(feature = "parallel"))]
impl<T> MaybeSendSync for T {}

/// Boxed test/hook body. Gains a `+ Send` bound under the `parallel` feature so
/// that whole top-level subtrees can be moved onto worker threads.
#[cfg(feature = "parallel")]
pub(crate) type TestBody = Box<dyn Fn() + Send>;
/// Boxed test/hook body (sequential build — no `Send` bound).
#[cfg(not(feature = "parallel"))]
pub(crate) type TestBody = Box<dyn Fn()>;

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
    F: Fn() -> Fut + MaybeSend + 'static,
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

use std::cell::RefCell;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

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
                !atom_present(negated, labels)
            } else {
                atom_present(term, labels)
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
            if atom_present(negated, labels) {
                return false;
            }
        } else {
            has_positive = true;
            if atom_present(term, labels) {
                positive_match = true;
            }
        }
    }

    // If there were positive terms, at least one must have matched.
    // If only negative terms, they all passed (none excluded).
    !has_positive || positive_match
}

/// Glob-match a label `pattern` (supporting `*` wildcards, no `?`) against `text`.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while ti < t.len() {
        if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Does any of `labels` match the (possibly glob) `pattern`?
fn atom_present(pattern: &str, labels: &[&str]) -> bool {
    labels.iter().any(|l| glob_match(pattern, l))
}

/// A token in the boolean label-filter grammar.
#[derive(Debug)]
enum FilterTok {
    And,
    Or,
    Not,
    LParen,
    RParen,
    Atom(String),
}

/// Tokenize a boolean label filter. Atoms are maximal runs of non-operator,
/// non-whitespace characters, so `pg:edge-*` is a single atom.
fn tokenize_filter(s: &str) -> Result<Vec<FilterTok>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            ' ' | '\t' => i += 1,
            '(' => {
                toks.push(FilterTok::LParen);
                i += 1;
            }
            ')' => {
                toks.push(FilterTok::RParen);
                i += 1;
            }
            '!' => {
                toks.push(FilterTok::Not);
                i += 1;
            }
            '&' => {
                if chars.get(i + 1) == Some(&'&') {
                    toks.push(FilterTok::And);
                    i += 2;
                } else {
                    return Err("expected `&&`".to_string());
                }
            }
            '|' => {
                if chars.get(i + 1) == Some(&'|') {
                    toks.push(FilterTok::Or);
                    i += 2;
                } else {
                    return Err("expected `||`".to_string());
                }
            }
            _ => {
                let start = i;
                while i < chars.len()
                    && !matches!(chars[i], ' ' | '\t' | '(' | ')' | '!' | '&' | '|')
                {
                    i += 1;
                }
                toks.push(FilterTok::Atom(chars[start..i].iter().collect()));
            }
        }
    }
    Ok(toks)
}

/// Recursive-descent evaluator. Precedence: `!` (tightest) > `&&` > `||`.
struct FilterParser<'a> {
    toks: &'a [FilterTok],
    pos: usize,
    labels: &'a [&'a str],
}

impl FilterParser<'_> {
    fn eval_or(&mut self) -> Result<bool, String> {
        let mut v = self.eval_and()?;
        while matches!(self.toks.get(self.pos), Some(FilterTok::Or)) {
            self.pos += 1;
            let rhs = self.eval_and()?;
            v = v || rhs;
        }
        Ok(v)
    }

    fn eval_and(&mut self) -> Result<bool, String> {
        let mut v = self.eval_unary()?;
        while matches!(self.toks.get(self.pos), Some(FilterTok::And)) {
            self.pos += 1;
            let rhs = self.eval_unary()?;
            v = v && rhs;
        }
        Ok(v)
    }

    fn eval_unary(&mut self) -> Result<bool, String> {
        if matches!(self.toks.get(self.pos), Some(FilterTok::Not)) {
            self.pos += 1;
            Ok(!self.eval_unary()?)
        } else {
            self.eval_primary()
        }
    }

    fn eval_primary(&mut self) -> Result<bool, String> {
        let toks = self.toks;
        match toks.get(self.pos) {
            Some(FilterTok::Atom(a)) => {
                let present = atom_present(a, self.labels);
                self.pos += 1;
                Ok(present)
            }
            Some(FilterTok::LParen) => {
                self.pos += 1;
                let v = self.eval_or()?;
                if matches!(toks.get(self.pos), Some(FilterTok::RParen)) {
                    self.pos += 1;
                    Ok(v)
                } else {
                    Err("expected `)`".to_string())
                }
            }
            Some(t) => Err(format!("unexpected token `{t:?}`")),
            None => Err("unexpected end of filter".to_string()),
        }
    }
}

/// Evaluate the boolean grammar over `labels`.
fn eval_bool_filter(filter: &str, labels: &[&str]) -> Result<bool, String> {
    let toks = tokenize_filter(filter)?;
    if toks.is_empty() {
        return Ok(true);
    }
    let mut parser = FilterParser {
        toks: &toks,
        pos: 0,
        labels,
    };
    let v = parser.eval_or()?;
    if parser.pos != toks.len() {
        return Err("trailing tokens after expression".to_string());
    }
    Ok(v)
}

/// Evaluate a label filter against a spec's labels. `None`/empty → matches all.
///
/// Routes to the boolean grammar (`&&`, `||`, `!`, parentheses, glob atoms) when
/// any boolean operator or paren is present, otherwise to the legacy `,` (OR) /
/// `+` (AND) syntax (also glob-aware). Invalid boolean expressions match nothing
/// and print a warning.
pub(crate) fn labels_match(labels: &[&str], filter: Option<&str>) -> bool {
    let filter = match filter {
        Some(f) if !f.trim().is_empty() => f.trim(),
        _ => return true,
    };

    let is_bool = filter.contains("&&")
        || filter.contains("||")
        || filter.contains('(')
        || filter.contains(')');

    if is_bool {
        match eval_bool_filter(filter, labels) {
            Ok(matched) => matched,
            Err(e) => {
                eprintln!("rsspec: invalid label filter '{filter}': {e}");
                false
            }
        }
    } else {
        labels_match_filter(labels, filter)
    }
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

// INVARIANT (parallel feature): these fixture stores are `thread_local!`, which
// is exactly what gives per-subtree isolation under parallel execution — each
// worker thread has its own copy. This holds ONLY because the runner's unit of
// parallelism is one whole top-level subtree per worker (see
// `render_suites_parallel` in runner.rs), so a single subtree's
// before_all/before_each/it/after_each/after_all/cleanup all run on one thread.
// Do not move these to a `static`/`Arc`, and do not parallelize *within* a
// subtree, without redesigning fixture storage — either silently breaks
// isolation with no compile error.
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

    // Per-test store first, in a single borrow: on a hit `f` runs under the
    // borrow and we return `Ok`; on a miss `f` is handed back unused via `Err`
    // so the scope-stack fallthrough can still call it exactly once.
    let f = match SETUP_STORE.with(|cell| {
        let store = cell.borrow();
        match store.get(&tid) {
            Some(boxed) => Ok(f(boxed
                .downcast_ref::<T>()
                .expect("rsspec: TypeId/value type mismatch — internal invariant violated"))),
            None => Err(f),
        }
    }) {
        Ok(r) => return r,
        Err(f) => f,
    };

    SCOPE_SETUP_STACK.with(|cell| {
        let stack = cell.borrow();
        for layer in stack.iter().rev() {
            if let Some(boxed) = layer.get(&tid) {
                return f(boxed.downcast_ref::<T>().expect(
                    "rsspec: TypeId/value type mismatch — internal invariant violated",
                ));
            }
        }
        panic!(
            "rsspec: no value of type `{}` — add a returning before_each or before_all",
            std::any::type_name::<T>()
        )
    })
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
        assert!(labels_match_filter(
            &["integration", "fast"],
            "integration+!slow"
        ));
        // Has integration AND slow → should be excluded
        assert!(!labels_match_filter(
            &["integration", "slow"],
            "integration+!slow"
        ));
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
        assert!(!labels_match_filter(
            &["integration", "slow"],
            "integration,!slow"
        ));
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

    // ---- glob_match ----
    #[test]
    fn glob_exact_and_wildcard() {
        assert!(glob_match("integration", "integration"));
        assert!(!glob_match("integration", "unit"));
        assert!(glob_match("lang:*", "lang:plain-call"));
        assert!(!glob_match("lang:*", "pg:edge-calls"));
        assert!(glob_match("*:edge-calls", "pg:edge-calls"));
        assert!(glob_match("pg:edge-*", "pg:edge-calls"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("a*b*c", "axxbyyc"));
        assert!(glob_match("a*b", "ab")); // star matches empty
        assert!(!glob_match("*foo", "barfoox"));
    }

    // ---- labels_match: boolean grammar ----
    #[test]
    fn labels_bool_and_or_not() {
        assert!(labels_match(&["a", "b"], Some("a && b")));
        assert!(!labels_match(&["a"], Some("a && b")));
        assert!(labels_match(&["a"], Some("a || b")));
        assert!(!labels_match(&["c"], Some("a || b")));
        assert!(labels_match(&["x"], Some("!a")));
        assert!(!labels_match(&["a"], Some("!a")));
    }

    #[test]
    fn labels_bool_precedence_and_parens() {
        // && binds tighter than ||: "a || b && c" == "a || (b && c)"
        assert!(labels_match(&["a"], Some("a || b && c")));
        assert!(!labels_match(&["b"], Some("a || b && c"))); // b set, c unset
                                                             // parens override precedence
        assert!(labels_match(&["b", "c"], Some("(a || b) && c")));
        assert!(!labels_match(&["b"], Some("(a || b) && c")));
        // exclusion
        assert!(!labels_match(&["a", "b"], Some("(a || x) && !b")));
        assert!(labels_match(&["a", "x"], Some("(a || z) && !b")));
    }

    #[test]
    fn labels_bool_with_glob() {
        assert!(labels_match(&["lang:async"], Some("lang:* && !pg:slow")));
        assert!(!labels_match(
            &["lang:async", "pg:slow"],
            Some("lang:* && !pg:slow")
        ));
        assert!(labels_match(
            &["pg:edge-calls"],
            Some("lang:* || pg:edge-*")
        ));
    }

    #[test]
    fn labels_none_or_empty_matches_all() {
        assert!(labels_match(&["a"], None));
        assert!(labels_match(&["a"], Some("")));
        assert!(labels_match(&["a"], Some("   ")));
    }

    #[test]
    fn labels_legacy_syntax_still_works_via_dispatcher() {
        assert!(labels_match(&["a", "b"], Some("a+b"))); // legacy AND
        assert!(!labels_match(&["a"], Some("a+b")));
        assert!(labels_match(&["a"], Some("a,b"))); // legacy OR
        assert!(!labels_match(&["slow"], Some("!slow"))); // legacy exclude
        assert!(labels_match(&["lang:async"], Some("lang:*"))); // glob atom in legacy path
    }

    #[test]
    fn labels_invalid_bool_filter_excludes() {
        // Malformed expressions must not match (a warning is printed).
        assert!(!labels_match(&["a"], Some("a &&")));
        assert!(!labels_match(&["a"], Some("(a")));
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
