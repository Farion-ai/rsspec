//! Regression: a resource bound to the Tokio runtime created in an async
//! `before_all` must stay usable from later async hooks/tests.
//!
//! Models the sqlx-pool case. An async `before_all` spawns a long-lived actor on
//! the suite runtime and returns a CLONE-able handle (an mpsc `Sender`) as its
//! fixture. Later async `it`s clone the handle out of the fixture synchronously
//! and round-trip on it. If every async hook/test built its own throwaway
//! runtime, the actor (spawned during `before_all`) would be dropped with that
//! runtime and the channel closed — the "IO driver has terminated" class of bug.
//! One suite-scoped runtime keeps it alive, and its state persists across hooks.

use tokio::sync::{mpsc, oneshot};

/// Clone-able handle to a counter actor — the analogue of a `sqlx::Pool`.
#[derive(Clone)]
struct Counter {
    tx: mpsc::Sender<oneshot::Sender<u32>>,
}

fn main() {
    rsspec::run(|ctx| {
        ctx.describe("suite-scoped async runtime", |ctx| {
            // Value-returning async before_all (FR2): spawns the actor on the
            // suite runtime (FR1) and stores the handle as a per-scope fixture.
            ctx.async_before_all(|| async {
                let (tx, mut rx) = mpsc::channel::<oneshot::Sender<u32>>(8);
                tokio::spawn(async move {
                    let mut n: u32 = 0;
                    while let Some(reply) = rx.recv().await {
                        n += 1;
                        let _ = reply.send(n);
                    }
                });
                Counter { tx }
            });

            ctx.async_it("first round-trip sees count 1", || async {
                let tx = rsspec::fixture_cloned::<Counter>().tx;
                let (reply, recv) = oneshot::channel();
                tx.send(reply)
                    .await
                    .expect("actor must still be alive on the shared runtime");
                assert_eq!(recv.await.unwrap(), 1, "first call increments to 1");
            });

            ctx.async_it("second round-trip sees count 2", || async {
                let tx = rsspec::fixture_cloned::<Counter>().tx;
                let (reply, recv) = oneshot::channel();
                tx.send(reply)
                    .await
                    .expect("actor still alive across the second hook");
                assert_eq!(recv.await.unwrap(), 2, "actor state persisted across hooks");
            });
        });
    });
}
