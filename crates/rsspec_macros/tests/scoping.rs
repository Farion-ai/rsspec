//! Phase-1: nested `context!` reads enclosing fixtures, `it!` reads multiple
//! fixtures, and an inner `before_all!` derives from an outer one — all via the
//! same scan-free implicit injection. `run_inline` panics on any failure.

use rsspec_macros::describe;

#[test]
fn nested_context_reads_outer_fixture() {
    rsspec::run_inline(|_| {
        describe!("outer", {
            before_all!(base: i32 = 100);
            context!("inner", {
                it!("sees the outer fixture", {
                    assert_eq!(*base, 100);
                });
            });
        });
    });
}

#[test]
fn it_reads_multiple_fixtures() {
    rsspec::run_inline(|_| {
        describe!("two fixtures", {
            before_all!(name: String = String::from("alice"));
            before_all!(age: u32 = 30);
            it!("reads both", {
                assert_eq!(name.as_str(), "alice");
                assert_eq!(*age, 30);
            });
        });
    });
}

#[test]
fn inner_before_all_derives_from_outer() {
    rsspec::run_inline(|_| {
        describe!("derivation", {
            before_all!(base: i32 = 10);
            context!("derived", {
                // reads `base` implicitly, returns a distinct type so no collision
                before_all!(doubled: u64 = (base * 2) as u64);
                it!("is twice the base", {
                    assert_eq!(*doubled, 20);
                });
            });
        });
    });
}
