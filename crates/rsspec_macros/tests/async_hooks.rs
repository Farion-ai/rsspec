//! Async bodies in the macro hook layer: `after_all!`, `after_each!`, and
//! `just_before_each!` accept an `async { … }` body, mirroring `before_all!`'s
//! async form. An async hook can read an enclosing fixture — it is cloned out
//! before the `await` (a `&T` from the store can't be held across `.await`), so
//! the owned value lives across the await.

use rsspec_macros::describe;
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
use std::sync::Arc;

#[derive(Clone)]
struct Probe {
    teardowns: Arc<AtomicU32>,
}

async fn open_probe() -> Probe {
    Probe {
        teardowns: Arc::new(AtomicU32::new(0)),
    }
}

async fn record(p: &Probe) {
    p.teardowns.fetch_add(1, SeqCst);
}

async fn tick() {}

#[test]
fn after_all_async_body_reads_an_enclosing_fixture() {
    // Observed via a static, not a captured variable: an async hook lowers to a
    // `Fn() -> Fut`, so it can't move a captured non-Clone outer value into the
    // coroutine. Owned state crosses the await via the cloned fixture instead.
    static OBSERVED: AtomicU32 = AtomicU32::new(99);
    OBSERVED.store(99, SeqCst);

    rsspec::run_inline(|_| {
        describe!("async teardown", {
            before_all!(res: Probe = async { open_probe().await });
            it!("the resource starts open", {
                assert_eq!(res.teardowns.load(SeqCst), 0);
            });
            after_all!(async {
                record(&res).await; // reads the cloned `res`, across an await
                OBSERVED.store(res.teardowns.load(SeqCst), SeqCst);
            });
        });
    });

    assert_eq!(OBSERVED.load(SeqCst), 1, "async after_all read the fixture");
}

#[test]
fn after_each_and_just_before_each_accept_async_bodies() {
    static JBE: AtomicU32 = AtomicU32::new(0);
    static AE: AtomicU32 = AtomicU32::new(0);
    JBE.store(0, SeqCst);
    AE.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("async per-spec hooks", {
            just_before_each!(async {
                tick().await;
                JBE.fetch_add(1, SeqCst);
            });
            after_each!(async {
                tick().await;
                AE.fetch_add(1, SeqCst);
            });
            it!("a", {
                assert!(1 + 1 == 2);
            });
            it!("b", {
                assert!(2 + 2 == 4);
            });
        });
    });

    assert_eq!(JBE.load(SeqCst), 2, "async just_before_each ran per spec");
    assert_eq!(AE.load(SeqCst), 2, "async after_each ran per spec");
}
