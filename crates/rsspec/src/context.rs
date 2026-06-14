//! Closure-based BDD API — Context, ItBuilder, SuiteBuilder, and `run()`.

use crate::runner::{self, RunConfig, Suite, TestNode};
use std::cell::RefCell;

// ============================================================================
// IntoTestBody — marker-type dispatch for it() accepting Fn() or Fn(&T)
// ============================================================================

/// Marker: test body takes no parameters.
#[doc(hidden)]
pub struct Plain(());
/// Marker: test body receives a `&T` from a returning `before_each`.
#[doc(hidden)]
pub struct WithSetup<T>(std::marker::PhantomData<T>);

/// Sealing supertrait — keeps `IntoTestBody` and `IntoBeforeHook` non-implementable
/// downstream so their signatures can evolve. Marker-generic so the two blanket
/// impls below cannot overlap.
mod private {
    pub trait Sealed<Marker> {}
}

/// Trait that converts a closure into a boxed `Fn()` test body.
/// Uses marker types so `it()` can accept both `|| { ... }` and `|val: &T| { ... }`.
///
/// This trait is an implementation detail of rsspec's marker-type dispatch.
/// It is not a user extension point — it is sealed.
#[doc(hidden)]
pub trait IntoTestBody<Marker>: private::Sealed<Marker> {
    fn into_test_fn(self) -> crate::TestBody;
}

// Sealing impls shared by `IntoTestBody` and `IntoBeforeHook`. The return type is
// left free (`-> T`) so a single seal covers both the value-discarding closures
// (`Fn()` / `Fn(&T)`, used by `it` / `after_*`) and the value-returning ones
// (`Fn() -> T` / `Fn(&U) -> T`, used by `before_*`). The markers keep the two
// blanket impls from overlapping; `mod private` keeps the trait unimplementable
// downstream.
impl<T, F: Fn() -> T + crate::MaybeSend + 'static> private::Sealed<Plain> for F {}
impl<U, T, F: Fn(&U) -> T + crate::MaybeSend + 'static> private::Sealed<WithSetup<U>> for F {}

impl<F: Fn() + crate::MaybeSend + 'static> IntoTestBody<Plain> for F {
    fn into_test_fn(self) -> crate::TestBody {
        Box::new(self)
    }
}

impl<T: 'static, F: Fn(&T) + crate::MaybeSend + 'static> IntoTestBody<WithSetup<T>> for F {
    fn into_test_fn(self) -> crate::TestBody {
        Box::new(move || {
            crate::with_setup_value::<T, _>(|val| self(val));
        })
    }
}

// ============================================================================
// IntoBeforeHook — marker-type dispatch for before_each/before_all
// ============================================================================
//
// before_* hooks differ from `it`/after_* in that they may also *return* a new
// fixture (stored by type when `T != ()`). The two markers mirror `IntoTestBody`:
// `Plain` is `Fn() -> T` (no read), `WithSetup<U>` is `Fn(&U) -> T` (reads a
// fixture from an enclosing scope, exactly like `it(|v: &U|)`).

/// Selects which fixture store a returning `before_*` hook writes into.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum FixtureScope {
    /// Per-test store, cleared between tests (`before_each`).
    PerTest,
    /// Per-describe-scope store, persists across the scope's tests (`before_all`).
    PerScope,
}

/// Store a returning hook's value, unless it is the unit type (side-effect-only
/// hooks return `()` and store nothing).
fn store_before_value<T: 'static>(scope: FixtureScope, val: T) {
    if std::any::TypeId::of::<T>() != std::any::TypeId::of::<()>() {
        match scope {
            FixtureScope::PerScope => crate::store_scope_setup_value(val),
            FixtureScope::PerTest => crate::store_setup_value(val),
        }
    }
}

/// Trait that converts a `before_each`/`before_all` closure into a boxed hook
/// body. Marker-generic (`Plain` / `WithSetup<U>`) so the no-read and fixture-
/// reading forms dispatch without overlap. Sealed — not a user extension point.
#[doc(hidden)]
pub trait IntoBeforeHook<Marker>: private::Sealed<Marker> {
    fn into_before_fn(self, scope: FixtureScope) -> crate::TestBody;
}

impl<T: 'static, F: Fn() -> T + crate::MaybeSend + 'static> IntoBeforeHook<Plain> for F {
    fn into_before_fn(self, scope: FixtureScope) -> crate::TestBody {
        Box::new(move || store_before_value(scope, self()))
    }
}

impl<U: 'static, T: 'static, F: Fn(&U) -> T + crate::MaybeSend + 'static>
    IntoBeforeHook<WithSetup<U>> for F
{
    fn into_before_fn(self, scope: FixtureScope) -> crate::TestBody {
        Box::new(move || {
            // Safe re-entrancy: `self(u)` runs under a shared borrow of the
            // fixture store, and the only writers (`store_*_setup_value`) are
            // `pub(crate)` and never reachable from a hook body — so no
            // `borrow_mut` can fire mid-read. The read borrow is then released
            // when `with_setup_value` returns `val`, before the store below.
            let val = crate::with_setup_value::<U, _>(|u| self(u));
            store_before_value(scope, val);
        })
    }
}

// ============================================================================
// Thread-local suite builder
// ============================================================================

thread_local! {
    static BUILDER: RefCell<Option<SuiteBuilder>> = const { RefCell::new(None) };
}

pub(crate) struct SuiteBuilder {
    stack: Vec<GroupFrame>,
}

struct GroupFrame {
    name: String,
    focused: bool,
    pending: bool,
    labels: Vec<String>,
    before_each: Vec<crate::TestBody>,
    after_each: Vec<crate::TestBody>,
    before_all: Vec<crate::TestBody>,
    after_all: Vec<crate::TestBody>,
    just_before_each: Vec<crate::TestBody>,
    children: Vec<TestNode>,
}

impl GroupFrame {
    fn root() -> Self {
        GroupFrame {
            name: String::new(),
            focused: false,
            pending: false,
            labels: Vec::new(),
            before_each: Vec::new(),
            after_each: Vec::new(),
            before_all: Vec::new(),
            after_all: Vec::new(),
            just_before_each: Vec::new(),
            children: Vec::new(),
        }
    }
}

impl SuiteBuilder {
    fn new() -> Self {
        SuiteBuilder {
            stack: vec![GroupFrame::root()],
        }
    }

    pub(crate) fn push_group(&mut self, name: String, focused: bool, pending: bool) {
        self.stack.push(GroupFrame {
            name,
            focused,
            pending,
            labels: Vec::new(),
            before_each: Vec::new(),
            after_each: Vec::new(),
            before_all: Vec::new(),
            after_all: Vec::new(),
            just_before_each: Vec::new(),
            children: Vec::new(),
        });
    }

    pub(crate) fn pop_group(&mut self) {
        let frame = self.stack.pop().expect("rsspec: unbalanced group push/pop");
        let node = TestNode::Describe {
            name: frame.name,
            focused: frame.focused,
            pending: frame.pending,
            labels: frame.labels,
            before_each: frame.before_each,
            after_each: frame.after_each,
            before_all: frame.before_all,
            after_all: frame.after_all,
            just_before_each: frame.just_before_each,
            children: frame.children,
        };
        self.current_frame_mut().children.push(node);
    }

    pub(crate) fn add_node(&mut self, node: TestNode) {
        self.current_frame_mut().children.push(node);
    }

    fn add_before_each(&mut self, hook: crate::TestBody) {
        self.current_frame_mut().before_each.push(hook);
    }

    fn add_after_each(&mut self, hook: crate::TestBody) {
        self.current_frame_mut().after_each.push(hook);
    }

    fn add_before_all(&mut self, hook: crate::TestBody) {
        self.current_frame_mut().before_all.push(hook);
    }

    fn add_after_all(&mut self, hook: crate::TestBody) {
        self.current_frame_mut().after_all.push(hook);
    }

    fn add_just_before_each(&mut self, hook: crate::TestBody) {
        self.current_frame_mut().just_before_each.push(hook);
    }

    fn add_labels(&mut self, labels: Vec<String>) {
        self.current_frame_mut().labels.extend(labels);
    }

    fn current_frame_mut(&mut self) -> &mut GroupFrame {
        self.stack.last_mut().expect("rsspec: empty builder stack")
    }

    fn into_nodes(mut self) -> Vec<TestNode> {
        assert_eq!(
            self.stack.len(),
            1,
            "rsspec: unbalanced group push/pop at finalization"
        );
        self.stack
            .pop()
            .expect("rsspec: root frame missing — internal error")
            .children
    }
}

/// Access the thread-local builder.
pub(crate) fn with_builder<R>(f: impl FnOnce(&mut SuiteBuilder) -> R) -> R {
    BUILDER.with(|cell| {
        let mut opt = cell.borrow_mut();
        let builder = opt
            .as_mut()
            .expect("rsspec: Context used outside of rsspec::run()");
        f(builder)
    })
}

/// Push a named group onto the builder. Macro-layer backing for `describe!`,
/// kept private so `SuiteBuilder` stays internal.
pub(crate) fn push_group(name: &str, focused: bool, pending: bool) {
    with_builder(|b| b.push_group(name.to_string(), focused, pending));
}

/// Pop the current group off the builder. Macro-layer backing for `describe!`.
pub(crate) fn pop_group() {
    with_builder(|b| b.pop_group());
}

// ============================================================================
// Context — the user-facing handle
// ============================================================================

/// A lightweight handle for defining BDD test structure.
///
/// All methods delegate to a thread-local builder. `Context` is `Copy` so it
/// can be passed into nested closures without ceremony.
///
/// # Example
/// ```rust,no_run
/// rsspec::run(|ctx| {
///     ctx.describe("Calculator", |ctx| {
///         ctx.it("adds", || { assert_eq!(2 + 3, 5); });
///     });
/// });
/// ```
#[derive(Copy, Clone)]
pub struct Context;

impl Context {
    // ---- Describe / Context / When -------------------------------------------

    /// Define a named group of tests. Alias: [`context`](Self::context), [`when`](Self::when).
    pub fn describe(&self, name: &str, body: impl FnOnce(Context)) {
        self.describe_impl(name, false, false, body);
    }

    /// Focused variant of [`describe`](Self::describe). Only focused groups and their
    /// children run; all other tests are skipped.
    pub fn fdescribe(&self, name: &str, body: impl FnOnce(Context)) {
        self.describe_impl(name, true, false, body);
    }

    /// Pending variant of [`describe`](Self::describe). All children are marked pending
    /// and their bodies never execute.
    pub fn xdescribe(&self, name: &str, body: impl FnOnce(Context)) {
        self.describe_impl(name, false, true, body);
    }

    /// Alias for [`describe`](Self::describe).
    pub fn context(&self, name: &str, body: impl FnOnce(Context)) {
        self.describe(name, body);
    }

    /// Alias for [`fdescribe`](Self::fdescribe).
    pub fn fcontext(&self, name: &str, body: impl FnOnce(Context)) {
        self.fdescribe(name, body);
    }

    /// Alias for [`xdescribe`](Self::xdescribe).
    pub fn xcontext(&self, name: &str, body: impl FnOnce(Context)) {
        self.xdescribe(name, body);
    }

    /// Alias for [`describe`](Self::describe).
    pub fn when(&self, name: &str, body: impl FnOnce(Context)) {
        self.describe(name, body);
    }

    /// Alias for [`fdescribe`](Self::fdescribe).
    pub fn fwhen(&self, name: &str, body: impl FnOnce(Context)) {
        self.fdescribe(name, body);
    }

    /// Alias for [`xdescribe`](Self::xdescribe).
    pub fn xwhen(&self, name: &str, body: impl FnOnce(Context)) {
        self.xdescribe(name, body);
    }

    fn describe_impl(&self, name: &str, focused: bool, pending: bool, body: impl FnOnce(Context)) {
        with_builder(|b| b.push_group(name.to_string(), focused, pending));
        body(Context);
        with_builder(|b| b.pop_group());
    }

    // ---- It / Specify --------------------------------------------------------

    /// Define a test case. Returns an [`ItBuilder`] for optional decorators.
    ///
    /// The body can be either `Fn()` or `Fn(&T)` (receives the value
    /// returned by `before_each`).
    ///
    /// ```rust,no_run
    /// # fn main() { rsspec::run(|ctx| {
    /// ctx.it("works", || { assert!(true); });
    ///
    /// ctx.it("slow test", || { /* ... */ })
    ///     .labels(&["slow"])
    ///     .retries(3)
    ///     .timeout(5000);
    /// # }); }
    /// ```
    pub fn it<M>(&self, name: &str, body: impl IntoTestBody<M> + 'static) -> ItBuilder {
        ItBuilder::new(name.to_string(), body.into_test_fn(), false, false)
    }

    /// Focused variant of [`it`](Self::it). Only focused tests run; others are skipped.
    pub fn fit<M>(&self, name: &str, body: impl IntoTestBody<M> + 'static) -> ItBuilder {
        ItBuilder::new(name.to_string(), body.into_test_fn(), true, false)
    }

    /// Pending variant of [`it`](Self::it). The body is registered but never executed.
    pub fn xit<M>(&self, name: &str, body: impl IntoTestBody<M> + 'static) -> ItBuilder {
        ItBuilder::new(name.to_string(), body.into_test_fn(), false, true)
    }

    /// Alias for [`it`](Self::it).
    pub fn specify<M>(&self, name: &str, body: impl IntoTestBody<M> + 'static) -> ItBuilder {
        self.it(name, body)
    }

    /// Alias for [`fit`](Self::fit).
    pub fn fspecify<M>(&self, name: &str, body: impl IntoTestBody<M> + 'static) -> ItBuilder {
        self.fit(name, body)
    }

    /// Alias for [`xit`](Self::xit).
    pub fn xspecify<M>(&self, name: &str, body: impl IntoTestBody<M> + 'static) -> ItBuilder {
        self.xit(name, body)
    }

    // ---- Hooks ---------------------------------------------------------------

    /// Register a hook that runs before every test in this scope and nested scopes.
    /// Multiple `before_each` hooks in the same scope run in registration order.
    ///
    /// If the closure returns a value (`T ≠ ()`), it is stored and can be received
    /// by `it` blocks via a `|val: &T|` parameter. Closures that return `()` are
    /// treated as side-effect-only hooks (backward-compatible with the original API).
    ///
    /// The closure may also *read* a fixture from an enclosing scope via a `&U`
    /// parameter (one per hook), exactly like `it(|v: &U|)`, and derive its own
    /// value from it. Async hooks cannot read a fixture (the borrow can't be held
    /// across `.await`).
    ///
    /// ```rust,no_run
    /// # fn main() { rsspec::run(|ctx| {
    /// // Returning a fixture — received as &T by it blocks
    /// ctx.before_each(|| -> String { "hello".to_string() });
    /// ctx.it("receives the value", |s: &String| { assert_eq!(s, "hello"); });
    ///
    /// // Reading an enclosing-scope fixture and deriving a per-test one
    /// ctx.before_all(|| -> String { "base".to_string() });
    /// ctx.before_each(|base: &String| -> usize { base.len() });
    /// ctx.it("derived from the scope fixture", |n: &usize| { assert_eq!(*n, 4); });
    ///
    /// // Side-effect only — returns () silently, no fixture stored
    /// ctx.before_each(|| { /* setup */ });
    /// ctx.it("no fixture needed", || { assert!(true); });
    /// # }); }
    /// ```
    pub fn before_each<M>(&self, hook: impl IntoBeforeHook<M> + 'static) {
        with_builder(|b| b.add_before_each(hook.into_before_fn(FixtureScope::PerTest)));
    }

    /// Register a hook that runs after every test in this scope and nested scopes,
    /// even if the test panics. Multiple `after_each` hooks run inner-to-outer.
    /// May read an enclosing-scope fixture via a `&T` parameter (e.g. for teardown).
    ///
    /// An `after_*` hook cannot *return* a fixture — there is no later consumer —
    /// so a returning closure is rejected at compile time:
    ///
    /// ```compile_fail
    /// # fn main() { rsspec::run(|ctx| {
    /// ctx.after_each(|| -> String { "nowhere to go".to_string() });
    /// # }); }
    /// ```
    pub fn after_each<M>(&self, hook: impl IntoTestBody<M> + 'static) {
        with_builder(|b| b.add_after_each(hook.into_test_fn()));
    }

    /// Register a hook that runs once before all tests in this describe scope.
    /// Not inherited by nested scopes. Skipped if all children are filtered out.
    ///
    /// If the closure returns a value, it is stored and can be received by
    /// `it` blocks via a `|val: &T|` parameter. The value persists across
    /// all tests in the scope (not cleared between tests).
    ///
    /// The closure may also read a fixture from an enclosing scope via a `&U`
    /// parameter — the canonical "act once" seam, where an inner `before_all`
    /// derives per-context results from an expensive outer fixture:
    ///
    /// ```rust,no_run
    /// # fn main() { rsspec::run(|ctx| {
    /// ctx.before_all(|| -> String { "expensive".to_string() });
    /// ctx.describe("derived", |ctx| {
    ///     ctx.before_all(|env: &String| -> usize { env.len() });
    ///     ctx.it("uses the derived value", |n: &usize| { assert_eq!(*n, 9); });
    /// });
    /// # }); }
    /// ```
    pub fn before_all<M>(&self, hook: impl IntoBeforeHook<M> + 'static) {
        with_builder(|b| b.add_before_all(hook.into_before_fn(FixtureScope::PerScope)));
    }

    /// Register a hook that runs once after all tests in this describe scope.
    /// Not inherited by nested scopes. Runs even if `before_all` panicked.
    /// May read an enclosing-scope fixture via a `&T` parameter (e.g. for teardown).
    ///
    /// Like `after_each`, it cannot return a fixture — a returning closure is a
    /// compile error:
    ///
    /// ```compile_fail
    /// # fn main() { rsspec::run(|ctx| {
    /// ctx.after_all(|| -> String { "nowhere to go".to_string() });
    /// # }); }
    /// ```
    pub fn after_all<M>(&self, hook: impl IntoTestBody<M> + 'static) {
        with_builder(|b| b.add_after_all(hook.into_test_fn()));
    }

    /// Register a hook that runs after all `before_each` hooks but immediately
    /// before the test body. Useful for final setup that must run last.
    /// May read an enclosing-scope fixture via a `&T` parameter.
    pub fn just_before_each<M>(&self, hook: impl IntoTestBody<M> + 'static) {
        with_builder(|b| b.add_just_before_each(hook.into_test_fn()));
    }

    // ---- Labels on current describe ------------------------------------------

    /// Add labels to the current describe scope. Labels accumulate across
    /// multiple calls.
    ///
    /// ```rust,no_run
    /// # fn main() { rsspec::run(|ctx| {
    /// ctx.describe("integration tests", |ctx| {
    ///     ctx.labels(&["integration", "slow"]);
    ///     ctx.it("test", || { /* ... */ });
    /// });
    /// # }); }
    /// ```
    pub fn labels(&self, labels: &[&str]) {
        let labels: Vec<String> = labels.iter().map(|s| s.to_string()).collect();
        with_builder(|b| b.add_labels(labels));
    }

    // ---- Table-driven --------------------------------------------------------

    /// Start building a table-driven test.
    ///
    /// ```rust,no_run
    /// # fn main() { rsspec::run(|ctx| {
    /// ctx.describe_table("arithmetic")
    ///     .case("addition", (2i32, 3i32, 5i32))
    ///     .case("subtraction", (5, 3, 2))
    ///     .run(|(a, b, expected): &(i32, i32, i32)| {
    ///         assert_eq!(a + b, *expected);
    ///     });
    /// # }); }
    /// ```
    pub fn describe_table(&self, name: &str) -> crate::table::TableBuilder {
        crate::table::TableBuilder::new(name.to_string())
    }

    // ---- Ordered -------------------------------------------------------------

    /// Define an ordered sequence of steps that run as a single test.
    ///
    /// If any step fails, subsequent steps are skipped (fail-fast).
    ///
    /// ```rust,no_run
    /// # fn main() { rsspec::run(|ctx| {
    /// ctx.ordered("workflow", |oct| {
    ///     oct.step("step 1", || { /* ... */ });
    ///     oct.step("step 2", || { /* ... */ });
    /// });
    /// # }); }
    /// ```
    pub fn ordered(&self, name: &str, body: impl FnOnce(&mut crate::ordered::OrderedContext)) {
        let mut oct = crate::ordered::OrderedContext::new(name.to_string(), false);
        body(&mut oct);
        with_builder(|b| b.add_node(oct.into_node()));
    }

    /// Like [`ordered`](Self::ordered), but continues running steps even if one fails.
    pub fn ordered_continue_on_failure(
        &self,
        name: &str,
        body: impl FnOnce(&mut crate::ordered::OrderedContext),
    ) {
        let mut oct = crate::ordered::OrderedContext::new(name.to_string(), true);
        body(&mut oct);
        with_builder(|b| b.add_node(oct.into_node()));
    }
}

// ============================================================================
// Async methods (requires `tokio` feature)
// ============================================================================

#[cfg(feature = "tokio")]
impl Context {
    // ---- Async It / Specify ---------------------------------------------------

    /// Define an async test case. Returns an [`ItBuilder`] for optional decorators.
    ///
    /// ```rust,ignore
    /// ctx.async_it("fetches data", || async {
    ///     let data = fetch().await;
    ///     assert!(!data.is_empty());
    /// })
    /// .retries(3)
    /// .timeout(5000);
    /// ```
    pub fn async_it<F, Fut>(&self, name: &str, body: F) -> ItBuilder
    where
        F: Fn() -> Fut + crate::MaybeSend + 'static,
        Fut: std::future::Future<Output = ()> + 'static,
    {
        self.it(name, crate::async_test(body))
    }

    /// Focused variant of [`async_it`](Self::async_it).
    pub fn async_fit<F, Fut>(&self, name: &str, body: F) -> ItBuilder
    where
        F: Fn() -> Fut + crate::MaybeSend + 'static,
        Fut: std::future::Future<Output = ()> + 'static,
    {
        self.fit(name, crate::async_test(body))
    }

    /// Pending variant of [`async_it`](Self::async_it).
    pub fn async_xit<F, Fut>(&self, name: &str, body: F) -> ItBuilder
    where
        F: Fn() -> Fut + crate::MaybeSend + 'static,
        Fut: std::future::Future<Output = ()> + 'static,
    {
        self.xit(name, crate::async_test(body))
    }

    /// Alias for [`async_it`](Self::async_it).
    pub fn async_specify<F, Fut>(&self, name: &str, body: F) -> ItBuilder
    where
        F: Fn() -> Fut + crate::MaybeSend + 'static,
        Fut: std::future::Future<Output = ()> + 'static,
    {
        self.async_it(name, body)
    }

    /// Alias for [`async_fit`](Self::async_fit).
    pub fn async_fspecify<F, Fut>(&self, name: &str, body: F) -> ItBuilder
    where
        F: Fn() -> Fut + crate::MaybeSend + 'static,
        Fut: std::future::Future<Output = ()> + 'static,
    {
        self.async_fit(name, body)
    }

    /// Alias for [`async_xit`](Self::async_xit).
    pub fn async_xspecify<F, Fut>(&self, name: &str, body: F) -> ItBuilder
    where
        F: Fn() -> Fut + crate::MaybeSend + 'static,
        Fut: std::future::Future<Output = ()> + 'static,
    {
        self.async_xit(name, body)
    }

    // ---- Async Hooks ----------------------------------------------------------

    /// Async variant of [`before_each`](Context::before_each), driven on the
    /// suite-scoped runtime (see [`async_before_all`](Self::async_before_all)).
    ///
    /// The future may resolve to a value `T`, which is stored per-test exactly
    /// like a returning sync `before_each` — receive it in `it` blocks via a
    /// `|v: &T|` parameter. A future resolving to `()` is a side-effect hook.
    pub fn async_before_each<F, Fut, T>(&self, hook: F)
    where
        F: Fn() -> Fut + crate::MaybeSend + 'static,
        Fut: std::future::Future<Output = T> + 'static,
        T: 'static,
    {
        self.before_each(move || -> T { crate::block_on_suite(hook()) });
    }

    /// Async variant of [`after_each`](Context::after_each).
    /// Each invocation runs on a fresh single-threaded Tokio runtime.
    pub fn async_after_each<F, Fut>(&self, hook: F)
    where
        F: Fn() -> Fut + crate::MaybeSend + 'static,
        Fut: std::future::Future<Output = ()> + 'static,
    {
        self.after_each(crate::async_test(hook));
    }

    /// Async variant of [`before_all`](Context::before_all).
    ///
    /// rsspec drives this (and every async hook/test in the subtree) on one
    /// lazily-built `current_thread` Tokio runtime that lives for the whole
    /// subtree — so a connection pool or IO handle created here stays usable in
    /// later hooks and tests, with no `block_on` in your code.
    ///
    /// The future may resolve to a value `T`, which is stored per-scope exactly
    /// like a returning sync `before_all` (received via `|v: &T|`). Read an
    /// in-scope fixture from inside the body with the closure form (the borrow is
    /// released before any `.await`); a `&T` cannot be held across `.await`.
    pub fn async_before_all<F, Fut, T>(&self, hook: F)
    where
        F: Fn() -> Fut + crate::MaybeSend + 'static,
        Fut: std::future::Future<Output = T> + 'static,
        T: 'static,
    {
        self.before_all(move || -> T { crate::block_on_suite(hook()) });
    }

    /// Async variant of [`after_all`](Context::after_all).
    /// Runs on a fresh single-threaded Tokio runtime.
    pub fn async_after_all<F, Fut>(&self, hook: F)
    where
        F: Fn() -> Fut + crate::MaybeSend + 'static,
        Fut: std::future::Future<Output = ()> + 'static,
    {
        self.after_all(crate::async_test(hook));
    }

    /// Async variant of [`just_before_each`](Context::just_before_each).
    /// Each invocation runs on a fresh single-threaded Tokio runtime.
    pub fn async_just_before_each<F, Fut>(&self, hook: F)
    where
        F: Fn() -> Fut + crate::MaybeSend + 'static,
        Fut: std::future::Future<Output = ()> + 'static,
    {
        self.just_before_each(crate::async_test(hook));
    }
}

// ============================================================================
// ItBuilder — fluent decorator API, registers test on Drop
// ============================================================================

/// Builder returned by [`Context::it`]. Supports chaining decorators and
/// registers the test node when dropped.
///
/// ```rust,no_run
/// # fn main() { rsspec::run(|ctx| {
/// // Simple (drops immediately, registered at semicolon):
/// ctx.it("simple", || { assert!(true); });
///
/// // With decorators:
/// ctx.it("complex", || { /* ... */ })
///     .labels(&["integration"])
///     .retries(3)
///     .timeout(5000);
/// # }); }
/// ```
pub struct ItBuilder {
    name: String,
    body: Option<crate::TestBody>,
    focused: bool,
    pending: bool,
    labels: Vec<String>,
    retries: Option<u32>,
    timeout_ms: Option<u64>,
    must_pass_repeatedly: Option<u32>,
}

impl ItBuilder {
    fn new(name: String, body: crate::TestBody, focused: bool, pending: bool) -> Self {
        ItBuilder {
            name,
            body: Some(body),
            focused,
            pending,
            labels: Vec::new(),
            retries: None,
            timeout_ms: None,
            must_pass_repeatedly: None,
        }
    }

    /// Add labels for filtering via `RSSPEC_LABEL_FILTER`. Labels accumulate
    /// across multiple calls.
    pub fn labels(mut self, labels: &[&str]) -> Self {
        self.labels.extend(labels.iter().map(|s| s.to_string()));
        self
    }

    /// Retry the test up to `n` additional times on failure.
    pub fn retries(mut self, n: u32) -> Self {
        self.retries = Some(n);
        self
    }

    /// Fail the test if it exceeds `ms` milliseconds.
    ///
    /// **Note:** The timeout is checked *after* the closure returns — the
    /// closure cannot be forcibly aborted mid-execution. If your test blocks
    /// forever (e.g. an infinite loop or deadlock), the timeout will not fire.
    pub fn timeout(mut self, ms: u64) -> Self {
        self.timeout_ms = Some(ms);
        self
    }

    /// Require the test to pass `n` consecutive times.
    pub fn must_pass_repeatedly(mut self, n: u32) -> Self {
        self.must_pass_repeatedly = Some(n);
        self
    }
}

impl Drop for ItBuilder {
    fn drop(&mut self) {
        // If we're already panicking (e.g. a describe body panicked), don't
        // double-panic by trying to access the builder.
        if std::thread::panicking() {
            return;
        }
        let Some(body) = self.body.take() else {
            return;
        };
        let node = TestNode::It {
            name: std::mem::take(&mut self.name),
            focused: self.focused,
            pending: self.pending,
            labels: std::mem::take(&mut self.labels),
            retries: self.retries,
            timeout_ms: self.timeout_ms,
            must_pass_repeatedly: self.must_pass_repeatedly,
            test_fn: body,
        };
        with_builder(|b| b.add_node(node));
    }
}

// ============================================================================
// run() / run_inline() — entry points
// ============================================================================

/// Build the test tree from user closures.
fn build_tree(body: impl FnOnce(Context)) -> Vec<TestNode> {
    BUILDER.with(|cell| {
        *cell.borrow_mut() = Some(SuiteBuilder::new());
    });

    body(Context);

    BUILDER.with(|cell| {
        cell.borrow_mut()
            .take()
            .expect("rsspec: builder missing after run")
            .into_nodes()
    })
}

/// Build and run a BDD test suite.
///
/// Works in both contexts:
///
/// - **`harness = false`** — parses CLI args for filtering/listing, calls
///   [`std::process::exit`] on failure.
/// - **`#[test]` functions** — auto-detected via libtest-specific CLI args;
///   skips arg parsing and panics on failure so other tests can still run.
///
/// # Example
///
/// ```rust,no_run
/// rsspec::run(|ctx| {
///     ctx.describe("Calculator", |ctx| {
///         ctx.it("adds", || { assert_eq!(2 + 3, 5); });
///     });
/// });
/// ```
pub fn run(body: impl FnOnce(Context)) {
    let nodes = build_tree(body);

    // Auto-detect: are we inside cargo test's standard harness?
    let args: Vec<String> = std::env::args().collect();
    let inside_harness = runner::detect_libtest_args(&args[1..]).is_some();

    let config = if inside_harness {
        RunConfig {
            filter: None,
            list: false,
            include_ignored: false,
            parallelism: 1,
            label_filter: None,
        }
    } else {
        RunConfig::from_args()
    };

    let suite = Suite::new("", nodes);
    let result = runner::run_suites(vec![suite], &config);

    if result.failed > 0 {
        if inside_harness {
            // Inside #[test]: panic so other test functions still run
            let details = result
                .failures
                .iter()
                .enumerate()
                .map(|(i, f)| format!("  {}. {}", i + 1, f))
                .collect::<Vec<_>>()
                .join("\n");
            panic!("rsspec: {} test(s) failed\n{}", result.failed, details);
        } else {
            std::process::exit(1);
        }
    }
}

/// Build and run a BDD test suite inline, compatible with `#[test]` functions.
///
/// Unlike [`run`], this does **not** parse command-line args (avoiding
/// conflicts with `cargo test`'s own filter arguments) and **panics** on
/// failure instead of calling `process::exit`.
///
/// # Example
///
/// ```rust,no_run
/// #[test]
/// fn calculator_spec() {
///     rsspec::run_inline(|ctx| {
///         ctx.describe("Calculator", |ctx| {
///             ctx.it("adds", || { assert_eq!(2 + 3, 5); });
///         });
///     });
/// }
/// ```
pub fn run_inline(body: impl FnOnce(Context)) {
    let nodes = build_tree(body);
    let config = RunConfig {
        filter: None,
        list: false,
        include_ignored: false,
        parallelism: 1,
        label_filter: None,
    };
    let suite = Suite::new("", nodes);
    let result = runner::run_suites(vec![suite], &config);

    if result.failed > 0 {
        let details = result
            .failures
            .iter()
            .enumerate()
            .map(|(i, f)| format!("  {}. {}", i + 1, f))
            .collect::<Vec<_>>()
            .join("\n");
        panic!("rsspec: {} test(s) failed\n{}", result.failed, details);
    }
}
