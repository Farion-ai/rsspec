# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.7.0] — 2026-06-19

### Added

- **Hooks read enclosing-scope fixtures via `&T` params.** `before_all`,
  `before_each`, `after_all`, `after_each`, and `just_before_each` now accept a
  closure that takes a `&T` reference to a fixture from an outer scope — the same
  way `it(|v: &T|)` already did. A returning `before_all` can thus *read* an outer
  fixture and *write* its own, enabling the "act once, assert often" seam where an
  inner `before_all` derives per-context results from one expensive outer fixture,
  and `after_all` can use the fixture for teardown. Works in both the closure API
  and the macro layer (`before_all!(|env: &T| -> U { .. })`, `after_all!(|env: &T|
  { .. })`). One fixture per hook. The no-parameter forms are unchanged. (Async
  hooks and `it!` bodies read fixtures too — by name, cloned in; see below.)
- Re-exported (doc-hidden) and **sealed** `IntoBeforeHook`, the marker-dispatch
  trait backing the `before_*` hooks' read/return forms.

- **Optional macro layer with implicit fixture parameters.** A proc-macro
  (`rsspec_macros`, on by default behind the `macros` feature) provides the
  `describe!`/`context!`/`when!` (+ `f`/`x` variants), `it!`/`specify!` (+ `fit!`/
  `xit!`), and `before_all!`/`before_each!`/`after_each!`/`after_all!`/
  `just_before_each!` surface. A fixture declared `before_all!(name: T = …)` is
  then available by its bare `name` in later `it!`/hook bodies — no per-`it`
  `|v: &T|` parameter — so "arrange & act once, assert often" carries no
  per-assertion ceremony. An `it!` body is a block (in-scope fixtures read
  implicitly), an explicit `|v: &T|` closure (the runtime hands the reference in),
  or `async { … }` (feature `tokio`); trailing decorators `tags=[…], retries=N,
  timeout=MS, must_pass_repeatedly=N` apply. Two in-scope fixtures of the same
  type are a compile error — implicit reads can't disambiguate them. The macros
  lower to the same builder calls as the closure API and interoperate with it, so
  dropping back to closures is mechanical. Opt out with `default-features = false`
  to skip the proc-macro build cost.
- Re-exported (doc-hidden) and **sealed** `IntoTestBody`, the marker-dispatch
  trait the `it!` macros name in their bounds — sealing keeps it
  non-implementable downstream so its signature can evolve.

- **Async value-returning lifecycle hooks on a suite-scoped runtime.**
  `async_before_all` and `async_before_each` now resolve to a value `T` (was
  `()` only), stored exactly like their sync counterparts — so an async
  `before_all` can build a fixture (`before_all!(pool: Pool = async { … })`) and
  later sync `it!` bodies read it implicitly. rsspec drives every async hook and
  test in a subtree on **one** lazily-built `current_thread` Tokio runtime that
  lives for the whole subtree (per worker thread under `parallel`) instead of a
  throwaway runtime per call. A connection pool or IO handle created in an async
  hook therefore stays usable across later hooks and tests — no more "IO driver
  has terminated" — with no `block_on` in user code. A suite that runs no async
  work builds no runtime. The `tokio` feature now enables the `net`, `time`, and
  `sync` drivers so the runtime can do real IO under `enable_all`.
- **`rsspec::fixture_cloned::<T>()` and implicit fixture reads in `async`
  bodies.** Clone an in-scope fixture by type (requires `T: Clone`); inside an
  `async` hook or `it!` body a `&T` can't be held across `.await`, so the fixture
  is cloned out first and owned. The macro layer injects this automatically: an
  `async` hook body (`before_all!(core: T = async { … env … })`) **or an `async`
  `it!` body** (`it!("…", async { … env … })`) that names an enclosing fixture
  gets an owned clone bound before the `async` block — so an async spec can
  `.await` against the fixture and assert inline (no pre-computed transfer
  struct), and implicit reads work in async bodies just as they do in sync ones.

### Changed

- `async_before_all` / `async_before_each` gain a fixture type parameter
  (inferred from the future's output); existing `Output = ()` hooks are
  unaffected. A `&T` fixture still can't be held across `.await` — name the
  fixture in an `async` body and the macro clones it in, or call
  `rsspec::fixture_cloned::<T>()` directly.
- MSRV raised to **1.85**, required by the `trybuild`-based test suite's
  toolchain (its `toml` dependency moved to the 2024 edition). The library
  itself uses no 2024-edition features.

## [0.6.0] — 2026-06-03

### Added

- **Opt-in parallel execution (`parallel` feature).** Distinct top-level
  `describe` / `it` / `ordered` subtrees run on a worker-thread pool via
  `--parallel[=N]` (or `RSSPEC_PARALLEL`). `before_all` still runs once per
  subtree and fixtures stay isolated; output is buffered per subtree and flushed
  in tree order, so it stays deterministic. The feature adds a `Send` bound to
  every test/hook closure; with it off the API is byte-for-byte unchanged.

- **Boolean label-filter grammar with globs.** `RSSPEC_LABEL_FILTER` and the new
  `--label-filter` CLI flag accept `&&`, `||`, `!`, and parentheses, plus glob
  atoms (`lang:*`). Example: `--label-filter "lang:* && !pg:slow"`. The legacy
  `,` (OR) / `+` (AND) / `!` (exclude) syntax still works; the CLI flag overrides
  the env var.

## [0.5.0] — 2026-03-30

### Added

- **Fixture passing via `before_each` / `before_all` return values.** Hooks can
  now return a typed value `T` that is passed to `it` blocks as `&T`, eliminating
  the need for `OnceLock<Mutex<Option<T>>>` boilerplate:

  ```rust
  ctx.describe("API client", |ctx| {
      ctx.before_each(|| -> Client {
          Client::new("http://localhost:8080")
      });

      ctx.it("fetches users", |client: &Client| {
          let users = client.get("/users").unwrap();
          assert!(!users.is_empty());
      });
  });
  ```

  - `before_each` values are fresh per test (cleared after each `it`).
  - `before_all` values persist for the scope (cleared when the `describe` exits).
  - Nested scopes shadow outer values of the same type; the outer value is
    restored after the inner scope closes.
  - Plain `|| { }` hooks (returning `()`) continue to work unchanged.

- Async test support (`tokio` feature): `async_it`, `async_before_each`,
  `async_before_all`, `async_after_each`, `async_after_all`,
  `async_just_before_each`, `async_step`, `async_run` (table-driven async).

### Changed

- MSRV raised to **1.82** (required for `const {}` in `thread_local!`).

## [0.4.0] — earlier

See git history.
