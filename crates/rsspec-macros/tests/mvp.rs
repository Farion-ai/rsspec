//! Phase-0 MVP: implicit fixture resolution + act-once, on a universal fixture.
//!
//! `before_all!(stats: Stats = summarize(&data))` builds the summary **once**;
//! each `it!` reads `stats` bare (no `|stats: &Stats|`) and asserts one property.
//! `summarize` runs exactly once for the whole `describe!` — "arrange & act once,
//! assert often."

use rsspec_macros::describe;
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};

struct Stats {
    sum: i32,
    count: usize,
    max: i32,
}

static BUILDS: AtomicU32 = AtomicU32::new(0);

/// The "act": fold a slice into a summary, counting its own invocations so the
/// test can prove it runs once for the whole scenario.
fn summarize(values: &[i32]) -> Stats {
    BUILDS.fetch_add(1, SeqCst);
    Stats {
        sum: values.iter().sum(),
        count: values.len(),
        max: *values.iter().max().expect("non-empty input"),
    }
}

#[test]
fn implicit_fixture_resolves_and_acts_once() {
    BUILDS.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("summary of a dataset", {
            before_all!(stats: Stats = summarize(&[3, 1, 4, 1, 5, 9, 2, 6]));
            it!("sums every value", {
                assert_eq!(stats.sum, 31);
            });
            it!("counts the values", {
                assert_eq!(stats.count, 8);
            });
            it!("finds the maximum", {
                assert_eq!(stats.max, 9);
            });
        });
    });

    assert_eq!(
        BUILDS.load(SeqCst),
        1,
        "summarize ran once for the whole describe — arrange & act once, assert often"
    );
}
