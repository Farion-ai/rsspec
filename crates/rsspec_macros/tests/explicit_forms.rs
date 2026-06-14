//! Explicit closure-read forms — the escape hatch kept for parity with the
//! closure API. A `before_all!(|outer: &U| -> T { … })` reads an enclosing
//! fixture and returns a derived one; `after_all!(|outer: &T| { … })` reads a
//! fixture for teardown. Both pass straight through to the runtime's hook
//! dispatch (no implicit injection — the reference is named explicitly).

use rsspec_macros::describe;
use std::sync::{Arc, Mutex};

struct Account {
    balance: u32,
}
struct Statement {
    available: u32,
}

#[test]
fn inner_before_all_chains_from_outer_via_explicit_closure() {
    rsspec::run_inline(|_| {
        describe!("account", {
            before_all!(account: Account = Account { balance: 100 });
            describe!("statement", {
                // reads the outer `account` explicitly, returns a derived fixture
                before_all!(|account: &Account| -> Statement {
                    Statement {
                        available: account.balance - 25,
                    }
                });
                it!("derives availability", |s: &Statement| assert_eq!(s.available, 75));
            });
        });
    });
}

#[test]
fn after_all_reads_fixture_via_explicit_closure() {
    let seen = Arc::new(Mutex::new(Vec::<u32>::new()));
    let hook = Arc::clone(&seen);

    rsspec::run_inline(move |_| {
        describe!("account", {
            before_all!(account: Account = Account { balance: 100 });
            it!("uses the account", |a: &Account| assert_eq!(a.balance, 100));
            after_all!(|a: &Account| {
                hook.lock().unwrap().push(a.balance);
            });
        });
    });

    assert_eq!(*seen.lock().unwrap(), vec![100]);
}
