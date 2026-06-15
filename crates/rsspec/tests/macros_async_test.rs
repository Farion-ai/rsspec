//! Macro-form async fixtures: `before_all!(name: T = async { … })` drives the
//! future on the suite runtime, stores its `T`, and a later sync `it!` body reads
//! it implicitly by name — no `block_on`, no `|v: &T|` ceremony.

use rsspec::describe;
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};

#[derive(Clone)]
struct Env {
    base: u32,
}

static TORN_DOWN: AtomicU32 = AtomicU32::new(0);

fn main() {
    rsspec::run(|_| {
        describe!("async fixture via macro", {
            before_all!(port: u16 = async {
                tokio::task::yield_now().await;
                8080u16
            });

            it!("reads the async-built fixture implicitly", {
                assert_eq!(*port, 8080);
            });

            it!("still reads it on a later spec", {
                assert_eq!(*port, 8080);
            });
        });

        // FR5: an inner async before_all reads an ENCLOSING fixture by bare name.
        // The macro must inject an owned clone before the `async` block, since a
        // `&Env` borrow could not be held across the `.await`.
        describe!("async inner reads outer fixture", {
            before_all!(env: Env = async {
                tokio::task::yield_now().await;
                Env { base: 100 }
            });

            describe!("inner", {
                before_all!(core: u32 = async {
                    // hold a reference to the (owned, cloned) fixture across an await
                    let r = &env;
                    tokio::task::yield_now().await;
                    r.base + 1
                });

                it!("derives from the enclosing async fixture", {
                    assert_eq!(*core, 101);
                });
            });
        });

        // async teardown reading an enclosing fixture by bare name.
        describe!("async teardown", {
            before_all!(envt: Env = async {
                tokio::task::yield_now().await;
                Env { base: 5 }
            });

            after_all!(async {
                let r = &envt;
                tokio::task::yield_now().await;
                TORN_DOWN.fetch_add(r.base, SeqCst);
            });

            it!("runs the spec", {
                assert_eq!(envt.base, 5);
            });
        });
    });

    assert_eq!(
        TORN_DOWN.load(SeqCst),
        5,
        "async after_all ran once and read the enclosing fixture"
    );
}
