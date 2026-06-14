//! Optional macro layer — opt-in sugar over the closure API.
//!
//! Every macro lowers to a `$crate::__rt` free function that delegates to the
//! same thread-local builder the closure API uses, so macro and closure styles
//! interoperate in one suite and "drop to closures" is mechanical. Use the
//! macros inside a `rsspec::run` / `rsspec::run_inline` body; import them with
//! `use rsspec::*;` or per-name.
//!
//! ```rust,no_run
//! use rsspec::{before_all, describe, it};
//! rsspec::run(|_| {
//!     describe!("Calculator", {
//!         before_all!(base: i32 = 10);
//!         it!("adds",            2 + 3 == 5);          // bool-assertion sugar
//!         it!("uses fixture",    |base: &i32| assert_eq!(*base, 10));
//!         it!("does work", {     assert!(true); });    // block body
//!     });
//! });
//! ```

// ---- containers -----------------------------------------------------------

/// Define a group of specs. Aliases: [`context!`](crate::context),
/// [`when!`](crate::when). Focus/pending: [`fdescribe!`], [`xdescribe!`].
#[macro_export]
macro_rules! describe {
    ($name:expr, $body:block) => {
        $crate::__rt::describe($name, false, false, || $body)
    };
}

/// Focused group — only focused groups/specs run.
#[macro_export]
macro_rules! fdescribe {
    ($name:expr, $body:block) => {
        $crate::__rt::describe($name, true, false, || $body)
    };
}

/// Pending group — its specs are registered but never executed.
#[macro_export]
macro_rules! xdescribe {
    ($name:expr, $body:block) => {
        $crate::__rt::describe($name, false, true, || $body)
    };
}

/// Alias for [`describe!`].
#[macro_export]
macro_rules! context {
    ($name:expr, $body:block) => {
        $crate::describe!($name, $body)
    };
}
/// Alias for [`describe!`].
#[macro_export]
macro_rules! when {
    ($name:expr, $body:block) => {
        $crate::describe!($name, $body)
    };
}
/// Focused alias for [`describe!`].
#[macro_export]
macro_rules! fcontext {
    ($name:expr, $body:block) => {
        $crate::fdescribe!($name, $body)
    };
}
/// Focused alias for [`describe!`].
#[macro_export]
macro_rules! fwhen {
    ($name:expr, $body:block) => {
        $crate::fdescribe!($name, $body)
    };
}
/// Pending alias for [`describe!`].
#[macro_export]
macro_rules! xcontext {
    ($name:expr, $body:block) => {
        $crate::xdescribe!($name, $body)
    };
}
/// Pending alias for [`describe!`].
#[macro_export]
macro_rules! xwhen {
    ($name:expr, $body:block) => {
        $crate::xdescribe!($name, $body)
    };
}

// ---- specs ----------------------------------------------------------------

/// Define a spec. Body forms:
/// - `it!("d", { .. })` — block body
/// - `it!("d", expr)` — asserts `expr` (with the source shown on failure)
/// - `it!("d", |v: &T| ..)` — reads a `before_all`/`before_each` fixture (the
///   parameter must be `&T`; an owned `|v: T|` is rejected)
/// - `it!("d", async { .. })` — async body (feature `tokio`; the body must not
///   itself build or `block_on` a runtime — it already runs on one)
///
/// Optional trailing decorators (any order): `tags=[..]` (lowers to `.labels(..)`,
/// the same labels `RSSPEC_LABEL_FILTER` / `--label-filter` match on), `retries=N,
/// timeout=MS, must_pass_repeatedly=N`. Alias: [`specify!`]. Focus/pending:
/// [`fit!`], [`xit!`].
///
/// The body becomes a re-callable `Fn()` (so `retries` / `must_pass_repeatedly`
/// can re-run it), so it must not move captured state. `it!(..)` expands to a
/// statement and is not usable in expression position.
#[macro_export]
macro_rules! it {
    ($($t:tt)*) => { $crate::__it_impl!($crate::__rt::it, $($t)*) };
}
/// Alias for [`it!`].
#[macro_export]
macro_rules! specify {
    ($($t:tt)*) => { $crate::__it_impl!($crate::__rt::it, $($t)*) };
}
/// Focused spec.
#[macro_export]
macro_rules! fit {
    ($($t:tt)*) => { $crate::__it_impl!($crate::__rt::fit, $($t)*) };
}
/// Focused spec.
#[macro_export]
macro_rules! fspecify {
    ($($t:tt)*) => { $crate::__it_impl!($crate::__rt::fit, $($t)*) };
}
/// Pending spec — registered but never executed.
#[macro_export]
macro_rules! xit {
    ($($t:tt)*) => { $crate::__it_impl!($crate::__rt::xit, $($t)*) };
}
/// Pending spec — registered but never executed.
#[macro_export]
macro_rules! xspecify {
    ($($t:tt)*) => { $crate::__it_impl!($crate::__rt::xit, $($t)*) };
}

/// Internal: spec body-form dispatch shared by `it!`/`fit!`/`xit!`. `$ctor` is
/// the `__rt` constructor. Arm order matters — the greedy `$cond:expr` arm is
/// last so it does not shadow the block/async/closure forms.
#[doc(hidden)]
#[macro_export]
macro_rules! __it_impl {
    // block body
    ($ctor:path, $name:expr, { $($body:tt)* } $(, $($dec:tt)*)?) => {
        $crate::__it_decorate!( $ctor($name, move || { $($body)* }) $(, $($dec)*)? )
    };
    // async body — `__rt::async_test` only exists under the `tokio` feature
    ($ctor:path, $name:expr, async $blk:block $(, $($dec:tt)*)?) => {
        $crate::__it_decorate!(
            $ctor($name, $crate::__rt::async_test(move || async move $blk)) $(, $($dec)*)?
        )
    };
    // fixture-reading closure body: |v: &T| expr. The `&` is required — fixtures
    // are read by reference; an owned `|v: T|` misses this arm and falls through
    // to the bool arm (a local `bool` type error) instead of an opaque trait error.
    ($ctor:path, $name:expr, | $p:ident : & $ty:ty | $body:expr $(, $($dec:tt)*)?) => {
        $crate::__it_decorate!( $ctor($name, move |$p: &$ty| { $body }) $(, $($dec)*)? )
    };
    // bool-assertion sugar (greedy — MUST be the final arm)
    ($ctor:path, $name:expr, $cond:expr $(, $($dec:tt)*)?) => {
        $crate::__it_decorate!(
            $ctor($name, move || {
                assert!($cond, "expected `{}` to hold", ::core::stringify!($cond));
            }) $(, $($dec)*)?
        )
    };
}

/// Internal: fold optional, order-independent decorators onto the `ItBuilder`,
/// then drop it (the `Drop` impl registers the spec).
#[doc(hidden)]
#[macro_export]
macro_rules! __it_decorate {
    ($b:expr $(,)?) => { let _ = $b; };
    ($b:expr, tags = [ $($t:expr),* $(,)? ] $(, $($r:tt)*)?) => {
        $crate::__it_decorate!( $b.labels(&[$($t),*]) $(, $($r)*)? )
    };
    ($b:expr, retries = $n:expr $(, $($r:tt)*)?) => {
        $crate::__it_decorate!( $b.retries($n) $(, $($r)*)? )
    };
    ($b:expr, timeout = $ms:expr $(, $($r:tt)*)?) => {
        $crate::__it_decorate!( $b.timeout($ms) $(, $($r)*)? )
    };
    ($b:expr, must_pass_repeatedly = $n:expr $(, $($r:tt)*)?) => {
        $crate::__it_decorate!( $b.must_pass_repeatedly($n) $(, $($r)*)? )
    };
}

// ---- hooks ----------------------------------------------------------------

/// Run once before all specs in scope. Forms:
/// - `before_all!(name: T = expr)` — stores `T` for `it!(.., |v: &T| ..)`.
/// - `before_all!(|env: &U| -> T { .. })` — reads an enclosing-scope fixture `&U`
///   and stores the returned `T` (the read form drops the documentary name).
/// - `before_all!(|env: &U| { .. })` — reads `&U` for side effects only.
/// - `before_all!({ .. })` — side effects only.
///
/// The fixture is keyed by **type**, not by `name` (which is documentary), so two
/// same-type fixtures in one scope collide — last one wins. The read parameter
/// must be `&U`; an owned `|env: U|` is rejected. Arity is one fixture per hook.
#[macro_export]
macro_rules! before_all {
    (| $p:ident : & $in:ty | -> $out:ty $body:block) => {
        $crate::__rt::before_all(move |$p: &$in| -> $out { $body })
    };
    (| $p:ident : & $in:ty | $body:block) => {
        $crate::__rt::before_all(move |$p: &$in| $body)
    };
    ($name:ident : $ty:ty = $init:expr) => {
        $crate::__rt::before_all(move || -> $ty { $init })
    };
    ($body:block) => {
        $crate::__rt::before_all(move || $body)
    };
}

/// Run before every spec in scope. Fixture, read, or block form (see [`before_all!`]).
#[macro_export]
macro_rules! before_each {
    (| $p:ident : & $in:ty | -> $out:ty $body:block) => {
        $crate::__rt::before_each(move |$p: &$in| -> $out { $body })
    };
    (| $p:ident : & $in:ty | $body:block) => {
        $crate::__rt::before_each(move |$p: &$in| $body)
    };
    ($name:ident : $ty:ty = $init:expr) => {
        $crate::__rt::before_each(move || -> $ty { $init })
    };
    ($body:block) => {
        $crate::__rt::before_each(move || $body)
    };
}

/// Run after every spec in scope (even on panic). `after_each!(|env: &T| { .. })`
/// reads an enclosing-scope fixture; `after_each!({ .. })` is side-effect only.
#[macro_export]
macro_rules! after_each {
    (| $p:ident : & $ty:ty | $body:block) => {
        $crate::__rt::after_each(move |$p: &$ty| $body)
    };
    ($body:block) => {
        $crate::__rt::after_each(move || $body)
    };
}
/// Run once after all specs in scope. Read form `after_all!(|env: &T| { .. })`.
#[macro_export]
macro_rules! after_all {
    (| $p:ident : & $ty:ty | $body:block) => {
        $crate::__rt::after_all(move |$p: &$ty| $body)
    };
    ($body:block) => {
        $crate::__rt::after_all(move || $body)
    };
}
/// Run after all `before_each`, immediately before each spec body. Read form
/// `just_before_each!(|env: &T| { .. })`.
#[macro_export]
macro_rules! just_before_each {
    (| $p:ident : & $ty:ty | $body:block) => {
        $crate::__rt::just_before_each(move |$p: &$ty| $body)
    };
    ($body:block) => {
        $crate::__rt::just_before_each(move || $body)
    };
}
