# rsspec

A Ginkgo/RSpec-inspired BDD testing framework for Rust with a closure-based API.

Write expressive, structured tests using `describe`, `context`, `it`, lifecycle hooks, table-driven tests, and more — all in idiomatic Rust.

## Quick Start

Add to your `Cargo.toml`:

```toml
[dev-dependencies]
rsspec = "0.6"

[[test]]
name = "my_tests"
harness = false
```

Write your first spec in `tests/my_tests.rs`:

```rust
fn main() {
    rsspec::run(|ctx| {
        ctx.describe("Calculator", |ctx| {
            ctx.it("adds two numbers", || {
                assert_eq!(2 + 3, 5);
            });

            ctx.context("with negative numbers", |ctx| {
                ctx.it("handles negatives", || {
                    assert_eq!(-1 + 1, 0);
                });
            });
        });
    });
}
```

Run with `cargo test`:

```
Calculator
  ✓ adds two numbers
  with negative numbers
    ✓ handles negatives

PASS
2 passed (0.001s)
```

### Using with `#[test]` functions

`rsspec::run()` auto-detects when it's running inside cargo test's harness and adapts: it skips CLI arg parsing and panics on failure instead of calling `process::exit`.

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn calculator_spec() {
        rsspec::run(|ctx| {
            ctx.describe("Calculator", |ctx| {
                ctx.it("adds", || { assert_eq!(2 + 3, 5); });
            });
        });
    }
}
```

`run_inline()` is also available as an explicit alternative that never parses CLI args.

> **Note:** When using `#[test]` mode, the BDD tree output goes to stderr (which cargo test captures by default). Add `--show-output` or `--nocapture` to see it: `cargo test -- --show-output`

## API Reference

### Containers

Nest your specs with `describe`, `context`, or `when` — they are aliases:

```rust
ctx.describe("outer", |ctx| {
    ctx.context("inner", |ctx| {
        ctx.when("something happens", |ctx| {
            ctx.it("works", || { assert!(true); });
        });
    });
});
```

**Focus** — only run focused containers and their children:

```rust
ctx.fdescribe("only this runs", |ctx| {
    ctx.it("focused by inheritance", || { /* runs */ });
});
```

Variants: `fdescribe`, `fcontext`, `fwhen`

**Pending** — skip entire containers:

```rust
ctx.xdescribe("not yet implemented", |ctx| {
    ctx.it("skipped", || { /* never runs */ });
});
```

Variants: `xdescribe`, `xcontext`, `xwhen`

### Specs

Individual test cases use `it` or `specify`:

```rust
ctx.it("does something", || {
    assert_eq!(1 + 1, 2);
});

ctx.specify("also works", || {
    assert!(true);
});
```

**Focus**: `fit`, `fspecify` — **Pending**: `xit`, `xspecify`

> **Note:** Test closures must be `Fn()` (not `FnOnce`) to support retries and `must_pass_repeatedly`. If you need to move a non-Copy value into a test closure, wrap it in an `Rc` or use `clone()`.

### Lifecycle Hooks

| Hook | Runs | Scope |
| --- | --- | --- |
| `before_each` | Before every `it` | Inherited by nested scopes |
| `just_before_each` | After all `before_each`, right before the body | Inherited |
| `after_each` | After every `it` (even on panic) | Inherited |
| `before_all` | Once before all tests in scope | Per describe (not inherited) |
| `after_all` | Once after all tests in scope | Per describe (not inherited) |

```rust
ctx.describe("database tests", |ctx| {
    ctx.before_all(|| {
        // expensive setup — runs once
    });

    ctx.before_each(|| {
        // runs before every test
    });

    ctx.after_each(|| {
        // runs after every test (even on panic)
    });

    ctx.after_all(|| {
        // cleanup — runs once after all tests
    });

    ctx.it("uses the database", || {
        assert!(true);
    });
});
```

Execution order per test:

```
before_all (once) -> before_each -> just_before_each -> body -> after_each -> after_all (once)
```

**Hook details:**

- **Inheritance:** `before_each`, `after_each`, and `just_before_each` are inherited by nested `describe`/`context` blocks. `before_all` and `after_all` only run in the scope where they are defined.
- **Ordering:** `before_each` hooks run outer-to-inner. `after_each` hooks run inner-to-outer. Both are guaranteed to run even if a prior hook or the test body panics.
- **Multiple hooks:** You can register multiple hooks of the same type in the same scope. They all run in registration order.
- **Ordered tests:** `before_each` and `after_each` from parent describes wrap the *entire* ordered sequence, not each individual step.
- **Filtering optimization:** `before_all`/`after_all` are skipped when all children in a scope are filtered out (by labels or focus mode), avoiding unnecessary setup.

**Reading fixtures in hooks.** A returning `before_all`/`before_each` defines a fixture (keyed by type); any hook can *read* a fixture from an enclosing scope via a `&T` parameter, exactly like `it(|v: &T|)`. This is the "act once, assert often" seam — build one expensive fixture, then let nested setup derive per-context results from it:

```rust
ctx.describe("PURL lookup", |ctx| {
    ctx.before_all(|| -> TestEnv { TestEnv::seed() });        // expensive, once

    ctx.describe("core lookup", |ctx| {
        ctx.before_all(|env: &TestEnv| -> CoreResults {       // reads env, stores results
            CoreResults { affected: env.lookup(purl), fixed: env.fixed(purl) }
        });
        ctx.it("finds affected", |r: &CoreResults| { assert_eq!(r.affected.len(), 1); });
        ctx.it("returns empty for fixed", |r: &CoreResults| { assert!(r.fixed.is_empty()); });
    });

    ctx.after_all(|env: &TestEnv| { env.teardown(); });        // reads env for cleanup
});
```

One fixture per hook. Async hooks cannot read a fixture (the borrow can't be held across `.await`).

### Decorators

Attach metadata to `it` blocks using the fluent builder API:

```rust
ctx.it("tagged test", || { /* ... */ })
    .labels(&["integration", "slow"]);

ctx.it("flaky test", || { /* ... */ })
    .retries(3);

ctx.it("must be stable", || { /* ... */ })
    .must_pass_repeatedly(5);

ctx.it("fast test", || { /* ... */ })
    .timeout(1000);
```

Decorators can be combined:

```rust
ctx.it("everything", || { /* ... */ })
    .labels(&["smoke"])
    .retries(2)
    .timeout(5000);
```

**Decorator details:**

- **`.labels()`** accumulates across multiple calls: `.labels(&["a"]).labels(&["b"])` results in labels `["a", "b"]`.
- **`.retries(n)`** retries the test up to `n` additional times on failure. `retries(0)` means no retries (same as default).
- **`.must_pass_repeatedly(n)`** requires the test to pass `n` consecutive times. `n` must be >= 1.
- **`.timeout(ms)`** fails the test if it exceeds `ms` milliseconds. **Important:** The timeout is checked *after* the closure returns — it cannot abort a running test. If your test deadlocks or enters an infinite loop, the timeout will not fire. Use OS-level timeouts (e.g. CI job timeouts) as a safety net.
- **Composition order:** When combined, decorators apply as `timeout(must_pass_repeatedly(retries(body)))`. The timeout wraps the entire retry+must_pass cycle, not individual attempts.

### Describe-Level Labels

Add labels to a describe scope — they propagate to all child tests:

```rust
ctx.describe("integration tests", |ctx| {
    ctx.labels(&["integration"]);

    ctx.it("inherits labels", || { /* ... */ });
});
```

Labels accumulate: calling `ctx.labels()` multiple times adds to the existing set.

### Table-Driven Tests

Parameterized specs with `describe_table`:

```rust
ctx.describe_table("arithmetic")
    .case("addition", (2i32, 3i32, 5i32))
    .case("large numbers", (100, 200, 300))
    .case("negative", (-1, 1, 0))
    .run(|(a, b, expected): &(i32, i32, i32)| {
        assert_eq!(a + b, *expected);
    });
```

Each case becomes a separate test.

Use `case_unnamed` for auto-named cases (`case_1`, `case_2`, ...):

```rust
ctx.describe_table("squares")
    .case_unnamed((2i32, 4i32))
    .case_unnamed((3, 9))
    .case_unnamed((4, 16))
    .run(|(input, expected): &(i32, i32)| {
        assert_eq!(input * input, *expected);
    });
```

> **Type safety:** The first `.case()` call fixes the data type `T` for all subsequent cases. Mixing types is a compile-time error. Always annotate the first case's type explicitly (e.g. `2i32` not `2`) to avoid Rust's default integer inference.

### Ordered Tests

Sequential, fail-fast test workflows:

```rust
ctx.ordered("user registration", |oct| {
    oct.step("create account", || {
        // ...
    });
    oct.step("verify email", || {
        // ...
    });
});
```

All steps run in sequence. If any step fails, subsequent steps are skipped. Steps are numbered in the output (e.g. `[1/2] create account`).

Use `ordered_continue_on_failure` to run all steps regardless:

```rust
ctx.ordered_continue_on_failure("resilient workflow", |oct| {
    oct.step("step 1", || { /* ... */ });
    oct.step("step 2", || { /* runs even if step 1 fails */ });
});
```

## Macro layer (optional)

A thin, opt-in `macro_rules!` layer trims the closure ceremony. The macros lower to the
same builder calls as the closure API and interoperate with it inside the same `run` /
`run_inline` body, so you can adopt them gradually and drop back to closures any time.
Import with `use rsspec::*;` (or per name).

```rust
use rsspec::*;

fn main() {
    rsspec::run(|_| {
        describe!("Calculator", {
            before_all!(base: i32 = 10);              // fixture, stored once

            it!("adds", 2 + 3 == 5);                  // bare boolean — asserted
            it!("uses the fixture", |base: &i32| assert_eq!(*base, 10));
            it!("does setup work", {                  // block body
                let v = vec![1, 2, 3];
                assert_eq!(v.len(), 3);
            });

            context!("when slow", {
                it!("is tagged and retried", { /* … */ }, tags = ["slow"], retries = 2);
            });
        });
    });
}
```

**Specs** — `it!` / `specify!`, focused `fit!` / `fspecify!`, pending `xit!` /
`xspecify!`. Body forms: `{ block }`, a bare `expr` (asserted, with the expression text
shown on failure), `|v: &T|` (reads a `before_all` / `before_each` fixture), or
`async { … }` (requires the `tokio` feature — one `it!` replaces the `async_*` methods).
Trailing decorators in any order: `tags=[..]`, `retries=N`, `timeout=MS`,
`must_pass_repeatedly=N`.

**Containers** — `describe!` / `context!` / `when!`, focused `fdescribe!` / `fcontext!` /
`fwhen!`, pending `xdescribe!` / `xcontext!` / `xwhen!`.

**Hooks** — `before_all!` / `before_each!` take a fixture form `before_all!(name: T =
expr)` (stored for `|name: &T|` readers), a read-and-return form `before_all!(|env: &U|
-> T { .. })` that reads an enclosing-scope fixture and stores its result, or a `{ block }`
side-effect form. `after_each!` / `after_all!` / `just_before_each!` take a `{ block }` or a
read form `after_all!(|env: &T| { .. })`.

> **Note:** Reading a fixture still names its type at each `it` (`|resp: &T|`) — the
> macros remove the structural scaffolding, not that annotation. Everything lowers to the
> [closure API](#api-reference); the two styles are equivalent and mixable.

## Async Tests

Enable the `tokio` feature for async test support:

```toml
[dev-dependencies]
rsspec = { version = "0.6", features = ["tokio"] }
tokio = { version = "1", features = ["full"] }
```

Write async tests with `async_it`:

```rust
ctx.describe("API client", |ctx| {
    ctx.async_it("fetches data", || async {
        let data = fetch_data().await;
        assert!(!data.is_empty());
    });
});
```

All decorators work with async tests:

```rust
ctx.async_it("flaky network call", || async {
    let resp = call_api().await;
    assert!(resp.is_ok());
})
.retries(3)
.timeout(5000)
.labels(&["integration"]);
```

Async hooks:

```rust
ctx.describe("with async setup", |ctx| {
    ctx.async_before_each(|| async {
        setup_database().await;
    });

    ctx.async_after_each(|| async {
        cleanup_database().await;
    });

    ctx.async_it("uses the database", || async {
        let rows = query("SELECT 1").await;
        assert_eq!(rows.len(), 1);
    });
});
```

Async ordered steps and table-driven tests:

```rust
ctx.ordered("async workflow", |oct| {
    oct.async_step("create resource", || async { create().await; });
    oct.async_step("verify resource", || async { verify().await; });
});

ctx.describe_table("async endpoints")
    .case("users", "/api/users".to_string())
    .case("posts", "/api/posts".to_string())
    .async_run(|endpoint: &String| {
        let url = endpoint.clone();
        async move {
            let resp = reqwest::get(&url).await.unwrap();
            assert!(resp.status().is_success());
        }
    });
```

You can also use `rsspec::async_test()` directly to wrap any async closure:

```rust
ctx.it("manual async", rsspec::async_test(|| async {
    assert!(true);
}));
```

| Sync | Async |
| --- | --- |
| `it` / `fit` / `xit` | `async_it` / `async_fit` / `async_xit` |
| `specify` / `fspecify` / `xspecify` | `async_specify` / `async_fspecify` / `async_xspecify` |
| `before_each` / `after_each` | `async_before_each` / `async_after_each` |
| `before_all` / `after_all` | `async_before_all` / `async_after_all` |
| `just_before_each` | `async_just_before_each` |
| `step` (ordered) | `async_step` |
| `run` (table) | `async_run` |

**Async runtime details:**

- Each async test/hook gets a fresh **single-threaded** Tokio runtime (`new_current_thread`). This prevents cross-test state leakage and works correctly with retries.
- `tokio::spawn()` works but runs on the same thread — there is no multi-threaded parallelism within a single test.
- **Do not create a nested Tokio runtime** inside an async test. Calling `Runtime::new()` inside an `async_it` block will panic with "Cannot start a runtime from within a runtime."
- The `|| async { ... }` pattern (closure returning a future) is required because Rust's `async Fn()` trait is not yet stable.

## Parallel Execution

By default rsspec runs every spec sequentially on one thread. Enable the
`parallel` feature to run **distinct top-level subtrees** on a pool of worker
threads:

```toml
[dev-dependencies]
rsspec = { version = "0.6", features = ["parallel"] }
```

```bash
# Pick a worker count at runtime (CLI flag, env var, or `auto` for core count):
cargo test --test my_spec --features parallel -- --parallel=4
RSSPEC_PARALLEL=auto cargo test --test my_spec --features parallel
```

The unit of parallelism is one **top-level** `describe` / `it` / `ordered`
node. Each runs entirely on a single worker, so `before_all` still executes once
per subtree and fixtures stay isolated — exactly like Jest runs files in parallel
but specs within a file in order. Output is buffered per subtree and flushed in
tree order, so it stays deterministic regardless of which worker finishes first.

**The `parallel` feature adds a `Send` bound to every test and hook closure** so
they can move onto worker threads. This is gated behind the feature precisely
because a `dyn Fn` trait object bakes its auto-traits into its type — the
requirement cannot depend on the runtime worker count. With the feature off, the
API is unchanged and `!Send` bodies (capturing `Rc`/`RefCell`) still compile.

**Caveats:**

- Only distinct top-level subtrees run concurrently; specs within a subtree stay serial.
- Tests sharing **process-global mutable state** (`static mut`, shared atomics across subtrees, env vars, the current directory, shared files/ports) can race. Group them under one top-level `describe`, or run sequentially.
- Tests that install their own panic hook via `std::panic::set_hook` / `take_hook` are **unsupported** under `--parallel`: the panic hook is process-global and shared across all workers, so it races with rsspec's own hook and with other subtrees' panics.
- `--filter`, focus mode (`fit`/`fdescribe`), labels, and `xit`/pending apply identically.
- Async is unaffected — each async test still builds its own current-thread runtime inside the test body.
- The buffered tree output is deterministic; **stderr** (ordered-step echoes, retry notices) still interleaves.
- Without the `parallel` feature, `--parallel`/`RSSPEC_PARALLEL` are accepted but warn and run sequentially.

## Runtime Helpers

### defer_cleanup

Register LIFO cleanup functions (like Go's `defer`):

```rust
ctx.it("creates temp resources", || {
    rsspec::defer_cleanup(|| {
        // cleanup runs after this test, even on panic
    });
});
```

> **Note:** `defer_cleanup` uses a thread-local stack. Calling it from a `std::thread::spawn`ed thread inside a test will register the cleanup on the wrong thread. Keep cleanup registrations on the test thread.

### by

Document steps within a test:

```rust
ctx.it("complex workflow", || {
    rsspec::by("setting up prerequisites");
    // ...
    rsspec::by("performing the action");
    // ...
    rsspec::by("verifying the result");
    assert!(true);
});
```

Each step prints `STEP: description` to stderr.

### skip!

Skip a test at runtime:

```rust
ctx.it("requires a database", || {
    if !db_available() {
        rsspec::skip!("database not available");
    }
    // ... test body ...
});
```

## Environment Variables

| Variable | Description |
| --- | --- |
| `RSSPEC_LABEL_FILTER` | Filter tests by labels — boolean grammar + globs (see [Label filtering](#label-filtering)). Overridden by `--label-filter`. |
| `RSSPEC_FAIL_ON_FOCUS` | Set to `1` or `true` to fail when focused tests exist (CI safety) |
| `RSSPEC_PARALLEL` | Worker count for the `parallel` feature: a positive integer or `auto`. Overridden by `--parallel`. Ignored (warns) without the feature |
| `NO_COLOR` | Disable colored output |

## Label filtering

Attach labels with `.labels(&[...])` on a spec, or `ctx.labels(&[...])` on a
`describe` (inherited by all child specs). Filter at run time with the
`--label-filter` CLI flag or the `RSSPEC_LABEL_FILTER` env var (the flag wins):

```bash
cargo test --test seam -- --label-filter "lang:* && !pg:slow"
RSSPEC_LABEL_FILTER="integration || smoke" cargo test
```

The filter is a boolean expression:

- `&&`, `||`, `!`, and parentheses — `(lang:async || lang:plain) && !pg:slow`
- glob atoms with `*` — `lang:*` matches `lang:async` and `lang:plain-call`
- precedence: `!` > `&&` > `||`

The legacy syntax still works: `a,b` (OR), `a+b` (AND), `!x` (exclude). A spec's
effective labels are its own plus every ancestor `describe`'s. An invalid
expression matches nothing and prints a warning.

## Shared State Patterns

Since hooks and tests use `Fn() + 'static` closures, sharing mutable state requires thread-safe types. Here are the recommended patterns:

### Static atomics (simple counters/flags)

```rust
use std::sync::atomic::{AtomicU32, Ordering};

ctx.describe("with counter", |ctx| {
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    ctx.before_each(|| {
        COUNTER.fetch_add(1, Ordering::SeqCst);
    });

    ctx.it("counter incremented", || {
        assert!(COUNTER.load(Ordering::SeqCst) >= 1);
    });
});
```

### OnceLock (expensive one-time setup)

```rust
use std::sync::OnceLock;

ctx.describe("with shared resource", |ctx| {
    static POOL: OnceLock<DbPool> = OnceLock::new();

    ctx.before_all(|| {
        POOL.set(create_pool()).unwrap();
    });

    ctx.it("uses the pool", || {
        let pool = POOL.get().unwrap();
        // ... use pool ...
    });
});
```

## CI Integration

### GitHub Actions example

```yaml
- name: Run tests
  env:
    RSSPEC_FAIL_ON_FOCUS: "1"  # Fail if any fit/fdescribe slipped through
    NO_COLOR: "1"               # Clean CI logs
  run: cargo test

- name: Run integration tests only
  env:
    RSSPEC_LABEL_FILTER: "integration"
  run: cargo test
```

### Splitting test stages with labels

```rust
ctx.describe("API", |ctx| {
    ctx.labels(&["integration"]);
    // These only run when RSSPEC_LABEL_FILTER includes "integration"
    ctx.it("creates users", || { /* ... */ });
});

ctx.describe("Utils", |ctx| {
    ctx.labels(&["unit"]);
    // These only run when RSSPEC_LABEL_FILTER includes "unit"
    ctx.it("parses input", || { /* ... */ });
});
```

Then in CI:

```bash
RSSPEC_LABEL_FILTER=unit cargo test          # Fast PR checks
RSSPEC_LABEL_FILTER=integration cargo test   # Nightly / staging
```

## Migrating from `#[test]`

rsspec can coexist with standard `#[test]` functions. Migrate incrementally:

1. **Start with `#[test]` + `rsspec::run()`** — no `Cargo.toml` changes needed:

   ```rust
   #[test]
   fn user_api_spec() {
       rsspec::run(|ctx| {
           ctx.describe("User API", |ctx| {
               ctx.it("creates users", || { /* ... */ });
           });
       });
   }
   ```

2. **When you want full BDD output**, add a `[[test]]` entry with `harness = false`:

   ```toml
   [[test]]
   name = "user_api"
   harness = false
   ```

   Then change the test file to use `fn main()` instead of `#[test]`.

3. **Keep unit tests as `#[test]`** — rsspec is most valuable for integration and acceptance tests where nesting, hooks, and lifecycle management matter.

## googletest Integration

Enable the `googletest` feature for composable matchers:

```toml
[dev-dependencies]
rsspec = { version = "0.6", features = ["googletest"] }
```

```rust
use rsspec::matchers::*;

fn main() {
    rsspec::run(|ctx| {
        ctx.describe("with matchers", |ctx| {
            ctx.it("has elements", || {
                let v = vec![1, 2, 3];
                assert_that!(v, not(empty()));
            });
        });
    });
}
```

## License

Licensed under either of [Apache License, Version 2.0](http://www.apache.org/licenses/LICENSE-2.0)
or [MIT license](http://opensource.org/licenses/MIT) at your option.
