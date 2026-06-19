//! An async `it!` naming an enclosing fixture whose type is not `Clone`.
//! The macro injects `fixture_cloned::<T>()` before the future (so a borrow is
//! never held across `.await`), which requires `T: Clone`. A non-`Clone` type
//! must fail at the clone-in with a `Clone` bound error, not a confusing one.

use rsspec_macros::describe;

struct NotClone {
    base: u32,
}

fn main() {
    rsspec::run_inline(|_| {
        describe!("async it names a non-Clone fixture", {
            before_all!(nc: NotClone = NotClone { base: 1 });
            it!("tries to read it across an await", async {
                let _ = nc.base;
            });
        });
    });
}
