//! Phase-1: `it!` decorators (tags/retries/...) and the `after_*`/
//! `just_before_each` hooks reading fixtures implicitly.

use rsspec_macros::describe;
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};

#[test]
fn it_decorators_apply() {
    static ATTEMPTS: AtomicU32 = AtomicU32::new(0);
    ATTEMPTS.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("decorated", {
            it!(
                "retries until it passes",
                {
                    let n = ATTEMPTS.fetch_add(1, SeqCst);
                    assert!(n >= 2, "fail the first two attempts");
                },
                retries = 3
            );
            it!(
                "carries labels and a timeout",
                {
                    assert!(2 + 2 == 4);
                },
                tags = ["integration", "slow"],
                timeout = 1000
            );
        });
    });

    assert_eq!(ATTEMPTS.load(SeqCst), 3, "retried until it passed");
}

#[test]
fn after_all_reads_the_scope_fixture() {
    static AFTER_SAW: AtomicU32 = AtomicU32::new(0);
    AFTER_SAW.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("teardown", {
            before_all!(id: u32 = 7);
            it!("uses the fixture", {
                assert_eq!(*id, 7);
            });
            after_all!({
                AFTER_SAW.store(*id, SeqCst); // reads `id` implicitly
            });
        });
    });

    assert_eq!(AFTER_SAW.load(SeqCst), 7, "after_all read the fixture");
}

#[test]
fn just_before_each_reads_fixture_once_per_spec() {
    static JBE_SUM: AtomicU32 = AtomicU32::new(0);
    JBE_SUM.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("just-before-each", {
            before_all!(base: u32 = 5);
            just_before_each!({
                JBE_SUM.fetch_add(*base, SeqCst); // reads `base` implicitly
            });
            it!("a", {
                assert!(true);
            });
            it!("b", {
                assert!(true);
            });
        });
    });

    assert_eq!(JBE_SUM.load(SeqCst), 10, "ran once per spec (2 x base=5)");
}
