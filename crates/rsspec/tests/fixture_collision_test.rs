//! Runtime backstop for the same-type fixture collision.
//!
//! Two `before_all` values of the same type in ONE scope can't be disambiguated
//! by a type-keyed read (both resolve to the same `TypeId`), so the runtime must
//! reject the second registration rather than silently last-wins. The macro layer
//! catches the syntactic case at compile time; this `TypeId` guard catches what it
//! cannot see — the same type written two ways. The guard must NOT fire across
//! nested scopes, where inner-shadows-outer is intentional.
//!
//! Exercised through the closure API (no macros feature needed) so it pins the
//! runtime behavior the macro lowering relies on.

use std::panic::catch_unwind;

#[test]
fn two_same_type_before_all_in_one_scope_is_rejected() {
    let outcome = catch_unwind(|| {
        rsspec::run_inline(|ctx| {
            ctx.describe("collision", |ctx| {
                ctx.before_all(|| -> u32 { 1 });
                ctx.before_all(|| -> u32 { 2 }); // same TypeId, same scope
                ctx.it("never runs", |_n: &u32| {});
            });
        });
    });

    assert!(
        outcome.is_err(),
        "two same-type before_all in one scope must fail the run, not silently \
         keep the last value"
    );
}

#[test]
fn same_type_before_all_shadowing_across_scopes_is_allowed() {
    // The guard keys per scope layer, so nested re-declaration is legitimate
    // shadowing — this must run clean and resolve innermost-first.
    rsspec::run_inline(|ctx| {
        ctx.describe("outer", |ctx| {
            ctx.before_all(|| -> u32 { 1 });
            ctx.describe("inner", |ctx| {
                ctx.before_all(|| -> u32 { 2 }); // different scope layer — OK
                ctx.it("reads the inner value", |n: &u32| assert_eq!(*n, 2));
            });
            ctx.it("reads the outer value", |n: &u32| assert_eq!(*n, 1));
        });
    });
}
