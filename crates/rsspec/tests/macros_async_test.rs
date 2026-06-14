//! Macro-form async fixtures: `before_all!(name: T = async { … })` drives the
//! future on the suite runtime, stores its `T`, and a later sync `it!` body reads
//! it implicitly by name — no `block_on`, no `|v: &T|` ceremony.

use rsspec::describe;

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
    });
}
