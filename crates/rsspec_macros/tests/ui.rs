//! Compile-fail (UI) tests: diagnostics the proc-macro must emit. Each
//! `tests/ui/*.rs` case is compiled and its stderr pinned against `*.stderr`.
//! Run `TRYBUILD=overwrite cargo test -p rsspec_macros --test ui` to refresh.

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
