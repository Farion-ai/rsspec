//! Compile-fail (UI) tests: diagnostics the proc-macro must emit. Each
//! `tests/ui/*.rs` case is compiled and its stderr pinned against `*.stderr`.
//! Run `TRYBUILD=overwrite cargo test -p rsspec_macros --test ui` to refresh.

#[test]
fn ui() {
    // trybuild pins the compiler's rendering of our diagnostics, which can drift
    // across rustc versions. Run only where `RSSPEC_UI_TESTS` is set (the main CI
    // job and local refreshes), never on the MSRV toolchain.
    if std::env::var_os("RSSPEC_UI_TESTS").is_none() {
        eprintln!("skipping UI tests (set RSSPEC_UI_TESTS=1 to run)");
        return;
    }
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
