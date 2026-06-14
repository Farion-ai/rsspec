//! `it!` body forms beyond the implicit block: the explicit `|v: &T|` passthrough
//! (the runtime reads the fixture and hands it in — no implicit name in that body)
//! and the `async { … }` arm (lowered to `__rt::async_test`). The implicit-block
//! form is covered by `mvp.rs`.

use rsspec_macros::describe;

struct Stats {
    sum: i32,
    count: usize,
}

fn summarize(values: &[i32]) -> Stats {
    Stats {
        sum: values.iter().sum(),
        count: values.len(),
    }
}

async fn doubled(n: i32) -> i32 {
    n * 2
}

#[test]
fn explicit_closure_passthrough_reads_the_fixture() {
    rsspec::run_inline(|_| {
        describe!("explicit reads still work", {
            before_all!(stats: Stats = summarize(&[3, 1, 4, 1, 5, 9, 2, 6]));
            // Declared implicitly above; here read explicitly via `|s: &Stats|`.
            // The runtime hands the reference in — this body sees only `s`.
            it!("reads the sum via closure", |s: &Stats| {
                assert_eq!(s.sum, 31);
            });
            it!("reads the count via closure", |s: &Stats| {
                assert_eq!(s.count, 8);
            });
        });
    });
}

#[test]
fn async_body_runs_on_the_runtime() {
    rsspec::run_inline(|_| {
        describe!("async specs", {
            it!("awaits and asserts the result", async {
                assert_eq!(doubled(21).await, 42);
            });
        });
    });
}
