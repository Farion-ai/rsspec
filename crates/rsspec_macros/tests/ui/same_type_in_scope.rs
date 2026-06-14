//! Two in-scope fixtures of the same type — implicit reads can't disambiguate
//! them (both would resolve to the innermost value), so the macro must reject it
//! at expansion rather than silently last-wins.

use rsspec_macros::describe;

fn main() {
    rsspec::run_inline(|_| {
        describe!("two fixtures of the same type", {
            before_all!(first: String = String::from("x"));
            before_all!(second: String = String::from("y"));
            it!("reads first", {
                assert_eq!(first.len(), 1);
            });
        });
    });
}
