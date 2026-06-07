# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
