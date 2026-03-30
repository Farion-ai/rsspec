//! BDD-style test runner with colored, indented tree output.
//!
//! Used with `harness = false` test targets to get Ginkgo-like output:
//!
//! ```text
//! Calculator
//!   ✓ adds two numbers
//!   when negative
//!     ✓ handles negatives
//!     ✗ fails on overflow
//! ```

use std::borrow::Cow;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Instant;

// ============================================================================
// Test tree types
// ============================================================================

/// A step in an ordered test sequence.
pub(crate) struct OrderedStep {
    pub name: String,
    pub body: Box<dyn Fn()>,
}

/// A node in the BDD test tree.
pub(crate) enum TestNode {
    /// A describe/context/when container.
    Describe {
        name: String,
        focused: bool,
        pending: bool,
        labels: Vec<String>,
        before_each: Vec<Box<dyn Fn()>>,
        after_each: Vec<Box<dyn Fn()>>,
        before_all: Vec<Box<dyn Fn()>>,
        after_all: Vec<Box<dyn Fn()>>,
        just_before_each: Vec<Box<dyn Fn()>>,
        children: Vec<TestNode>,
    },
    /// An individual test case.
    It {
        name: String,
        focused: bool,
        pending: bool,
        labels: Vec<String>,
        retries: Option<u32>,
        timeout_ms: Option<u64>,
        must_pass_repeatedly: Option<u32>,
        test_fn: Box<dyn Fn()>,
    },
    /// An ordered sequence of steps that run as a single test.
    Ordered {
        name: String,
        labels: Vec<String>,
        continue_on_failure: bool,
        steps: Vec<OrderedStep>,
    },
}

#[cfg(test)]
impl TestNode {
    fn describe(name: impl Into<String>, children: Vec<TestNode>) -> Self {
        TestNode::Describe {
            name: name.into(),
            focused: false,
            pending: false,
            labels: Vec::new(),
            before_each: Vec::new(),
            after_each: Vec::new(),
            before_all: Vec::new(),
            after_all: Vec::new(),
            just_before_each: Vec::new(),
            children,
        }
    }

    fn describe_with_hooks(
        name: impl Into<String>,
        before_all: Vec<Box<dyn Fn()>>,
        after_all: Vec<Box<dyn Fn()>>,
        children: Vec<TestNode>,
    ) -> Self {
        TestNode::Describe {
            name: name.into(),
            focused: false,
            pending: false,
            labels: Vec::new(),
            before_each: Vec::new(),
            after_each: Vec::new(),
            before_all,
            after_all,
            just_before_each: Vec::new(),
            children,
        }
    }

    fn describe_with_each_hooks(
        name: impl Into<String>,
        before_each: Vec<Box<dyn Fn()>>,
        after_each: Vec<Box<dyn Fn()>>,
        children: Vec<TestNode>,
    ) -> Self {
        TestNode::Describe {
            name: name.into(),
            focused: false,
            pending: false,
            labels: Vec::new(),
            before_each,
            after_each,
            before_all: Vec::new(),
            after_all: Vec::new(),
            just_before_each: Vec::new(),
            children,
        }
    }

    fn it(name: impl Into<String>, f: impl Fn() + 'static) -> Self {
        TestNode::It {
            name: name.into(),
            focused: false,
            pending: false,
            labels: Vec::new(),
            retries: None,
            timeout_ms: None,
            must_pass_repeatedly: None,
            test_fn: Box::new(f),
        }
    }

    fn fit(name: impl Into<String>, f: impl Fn() + 'static) -> Self {
        TestNode::It {
            name: name.into(),
            focused: true,
            pending: false,
            labels: Vec::new(),
            retries: None,
            timeout_ms: None,
            must_pass_repeatedly: None,
            test_fn: Box::new(f),
        }
    }
}

/// Extract a human-readable message from a panic payload.
///
/// Returns a `Cow::Borrowed` when the payload is `&str` or `String`,
/// avoiding an allocation in the common case.
///
/// Must be called with `&*e` (not `&e`) when `e: Box<dyn Any + Send>`,
/// because `&Box<dyn Any>` coerces to a trait object for the Box itself
/// rather than deref-ing through to the inner type.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> Cow<'_, str> {
    if let Some(s) = payload.downcast_ref::<&str>() {
        Cow::Borrowed(s)
    } else if let Some(s) = payload.downcast_ref::<String>() {
        Cow::Borrowed(s.as_str())
    } else {
        Cow::Borrowed("unknown panic")
    }
}

// ============================================================================
// Hook chain — accumulates hooks from ancestor Describe nodes
// ============================================================================

/// Accumulated hooks from ancestor Describe nodes, grown with push/pop
/// as the runner descends into nested scopes. Avoids O(depth²) cloning.
#[derive(Default)]
struct HookChain<'a> {
    before_each: Vec<&'a dyn Fn()>,
    after_each: Vec<&'a dyn Fn()>,
    just_before_each: Vec<&'a dyn Fn()>,
    labels: Vec<&'a str>,
}

impl<'a> HookChain<'a> {
    /// Push the hooks and labels from `node` onto the chain.
    ///
    /// Returns the lengths before pushing so that [`pop_describe`] can
    /// restore the chain to its previous state.
    fn push_describe(&mut self, node: &'a TestNode) -> [usize; 4] {
        let saved = [
            self.before_each.len(),
            self.after_each.len(),
            self.just_before_each.len(),
            self.labels.len(),
        ];
        if let TestNode::Describe {
            before_each,
            after_each,
            just_before_each,
            labels,
            ..
        } = node
        {
            self.before_each.extend(before_each.iter().map(Box::as_ref));
            self.after_each.extend(after_each.iter().map(Box::as_ref));
            self.just_before_each
                .extend(just_before_each.iter().map(Box::as_ref));
            self.labels.extend(labels.iter().map(String::as_str));
        }
        saved
    }

    /// Restore the chain to the state captured by a prior [`push_describe`] call.
    fn pop_describe(&mut self, saved: [usize; 4]) {
        self.before_each.truncate(saved[0]);
        self.after_each.truncate(saved[1]);
        self.just_before_each.truncate(saved[2]);
        self.labels.truncate(saved[3]);
    }
}

// ============================================================================
// ANSI color helpers
// ============================================================================

/// Returns whether colored output should be used.
///
/// Result is cached in a `OnceLock` so the env-var and isatty checks
/// run at most once per process.
fn use_color() -> bool {
    use std::sync::OnceLock;
    static COLOR: OnceLock<bool> = OnceLock::new();
    *COLOR.get_or_init(|| {
        if std::env::var("NO_COLOR").is_ok() {
            return false;
        }
        std::io::IsTerminal::is_terminal(&std::io::stdout())
    })
}

fn green(s: &str) -> String {
    if use_color() {
        format!("\x1b[32m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn red(s: &str) -> String {
    if use_color() {
        format!("\x1b[31m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn yellow(s: &str) -> String {
    if use_color() {
        format!("\x1b[33m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn bold(s: &str) -> String {
    if use_color() {
        format!("\x1b[1m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn dim(s: &str) -> String {
    if use_color() {
        format!("\x1b[2m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

// ============================================================================
// Runner
// ============================================================================

/// Results from running a test tree.
#[derive(Default)]
pub(crate) struct RunResult {
    pub passed: usize,
    pub failed: usize,
    pub pending: usize,
    pub skipped: usize,
    pub failures: Vec<String>,
}

/// Configuration parsed from command-line args.
pub(crate) struct RunConfig {
    /// Filter string — only run tests whose full path contains this.
    pub filter: Option<String>,
    /// Only list tests, don't run them.
    pub list: bool,
    /// Include ignored/pending tests in the run.
    pub include_ignored: bool,
}

/// Args that are exclusively used by libtest (cargo test's built-in harness).
/// If we see any of these, `rsspec::run()` is almost certainly being called
/// inside a `#[test]` function instead of a `harness = false` binary.
const LIBTEST_ONLY_ARGS: &[&str] = &[
    "--format",
    "--test-threads",
    "--logfile",
    "--report-time",
    "--ensure-time",
    "--shuffle-seed",
    "--show-output",
    "-Zunstable-options",
];

/// Check if a list of CLI args contains libtest-specific arguments.
///
/// Returns `Some(arg)` with the first offending arg if detected, `None` otherwise.
pub(crate) fn detect_libtest_args(args: &[String]) -> Option<String> {
    for arg in args {
        // split_once is infallible here — the fallback branch is unreachable
        let arg_name = arg.split_once('=').map_or(arg.as_str(), |(name, _)| name);
        if LIBTEST_ONLY_ARGS.contains(&arg_name) {
            return Some(arg.clone());
        }
    }
    None
}

impl RunConfig {
    /// Parse from the process args (compatible with `cargo test -- <args>`).
    ///
    /// Only use this for `harness = false` targets. For `#[test]` functions,
    /// `run()` auto-detects the context and skips arg parsing.
    pub(crate) fn from_args() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let mut filter = None;
        let mut list = false;
        let mut include_ignored = false;

        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--list" => list = true,
                "--include-ignored" | "--ignored" => include_ignored = true,
                arg if !arg.starts_with('-') => {
                    filter = Some(arg.to_string());
                }
                _ => {}
            }
            i += 1;
        }

        RunConfig {
            filter,
            list,
            include_ignored,
        }
    }
}

/// A named suite for multi-suite runs.
pub(crate) struct Suite {
    pub name: String,
    pub nodes: Vec<TestNode>,
}

impl Suite {
    pub fn new(name: impl Into<String>, nodes: Vec<TestNode>) -> Self {
        Suite {
            name: name.into(),
            nodes,
        }
    }
}

/// Run a single test tree and print BDD-formatted output.
#[cfg(test)]
fn run_tree(nodes: &[TestNode], config: &RunConfig) -> RunResult {
    let focus_mode = tree_has_focus(nodes);
    let mut result = RunResult::default();
    let start = Instant::now();

    if config.list {
        let mut path = Vec::new();
        list_tree(nodes, &mut path, config);
        return result;
    }

    println!();
    let mut hooks = HookChain::default();
    let mut path = Vec::new();
    run_nodes(nodes, 0, &mut path, &mut hooks, focus_mode, false, config, &mut result);
    print_summary(&result, start.elapsed());

    result
}

/// Run multiple named suites, printing a header per suite and a combined summary.
pub(crate) fn run_suites(suites: &[Suite], config: &RunConfig) -> RunResult {
    let focus_mode = suites.iter().any(|s| tree_has_focus(&s.nodes));
    let mut result = RunResult::default();
    let start = Instant::now();

    if config.list {
        let mut path = Vec::new();
        for suite in suites {
            list_tree(&suite.nodes, &mut path, config);
        }
        return result;
    }

    println!();

    for suite in suites {
        if !suite.name.is_empty() {
            println!("{}", dim(&format!("--- {} ---", suite.name)));
            println!();
        }

        let mut hooks = HookChain::default();
        let mut path = Vec::new();
        run_nodes(
            &suite.nodes,
            0,
            &mut path,
            &mut hooks,
            focus_mode,
            false,
            config,
            &mut result,
        );

        if suites.len() > 1 {
            println!();
        }
    }

    print_summary(&result, start.elapsed());

    result
}

/// Check if any tests in this subtree will actually execute, considering
/// focus mode, label filters, path filters, and pending status.
///
/// Used to skip `before_all`/`after_all` when all children are filtered out.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn has_runnable_tests<'a>(
    nodes: &'a [TestNode],
    path: &mut Vec<String>,
    hooks: &mut HookChain<'a>,
    focus_mode: bool,
    force_focused: bool,
    config: &RunConfig,
) -> bool {
    for node in nodes {
        match node {
            TestNode::Describe {
                name,
                focused,
                pending,
                children,
                ..
            } => {
                if *pending {
                    continue;
                }
                let child_force_focused = force_focused || *focused;
                path.push(name.clone());
                let saved = hooks.push_describe(node);
                let has_any = has_runnable_tests(
                    children,
                    path,
                    hooks,
                    focus_mode,
                    child_force_focused,
                    config,
                );
                hooks.pop_describe(saved);
                path.pop();
                if has_any {
                    return true;
                }
            }
            TestNode::It {
                name,
                focused,
                pending,
                labels,
                ..
            } => {
                if *pending {
                    continue;
                }
                path.push(name.clone());
                let full_path = path.join(" > ");
                path.pop();
                if let Some(ref f) = config.filter {
                    if !full_path.to_lowercase().contains(&f.to_lowercase()) {
                        continue;
                    }
                }
                let effectively_focused = *focused || force_focused;
                if focus_mode && !effectively_focused && !config.include_ignored {
                    continue;
                }
                let all_labels: Vec<&str> = hooks
                    .labels
                    .iter()
                    .copied()
                    .chain(labels.iter().map(|s| s.as_str()))
                    .collect();
                if !crate::check_labels(&all_labels) {
                    continue;
                }
                return true;
            }
            TestNode::Ordered { name, labels, .. } => {
                path.push(name.clone());
                let full_path = path.join(" > ");
                path.pop();
                if let Some(ref f) = config.filter {
                    if !full_path.to_lowercase().contains(&f.to_lowercase()) {
                        continue;
                    }
                }
                if focus_mode && !force_focused && !config.include_ignored {
                    continue;
                }
                let all_labels: Vec<&str> = hooks
                    .labels
                    .iter()
                    .copied()
                    .chain(labels.iter().map(|s| s.as_str()))
                    .collect();
                if !crate::check_labels(&all_labels) {
                    continue;
                }
                return true;
            }
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn run_nodes<'a>(
    nodes: &'a [TestNode],
    depth: usize,
    path: &mut Vec<String>,
    hooks: &mut HookChain<'a>,
    focus_mode: bool,
    force_focused: bool,
    config: &RunConfig,
    result: &mut RunResult,
) {
    for node in nodes {
        run_node(node, depth, path, hooks, focus_mode, force_focused, config, result);
    }
}

/// Dispatch a single test node to the appropriate handler.
#[allow(clippy::too_many_arguments)]
fn run_node<'a>(
    node: &'a TestNode,
    depth: usize,
    path: &mut Vec<String>,
    hooks: &mut HookChain<'a>,
    focus_mode: bool,
    force_focused: bool,
    config: &RunConfig,
    result: &mut RunResult,
) {
    match node {
        TestNode::Describe { .. } => {
            run_describe_node(node, depth, path, hooks, focus_mode, force_focused, config, result);
        }
        TestNode::It { .. } => {
            run_it_node(node, depth, path, hooks, focus_mode, force_focused, config, result);
        }
        TestNode::Ordered { .. } => {
            run_ordered_node(node, depth, path, hooks, focus_mode, force_focused, config, result);
        }
    }
}

/// Run a Describe node: print its name, run `before_all`/`after_all`, recurse.
///
/// Manages the scope-setup stack push/pop and hook-chain push/pop so that
/// `before_all` values and inherited hooks are correctly scoped and restored.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn run_describe_node<'a>(
    node: &'a TestNode,
    depth: usize,
    path: &mut Vec<String>,
    hooks: &mut HookChain<'a>,
    focus_mode: bool,
    force_focused: bool,
    config: &RunConfig,
    result: &mut RunResult,
) {
    let TestNode::Describe {
        name,
        focused,
        pending,
        children,
        before_all,
        after_all,
        ..
    } = node
    else {
        unreachable!()
    };

    let indent = "  ".repeat(depth);
    println!("{indent}{}", bold(name));

    // If this describe is pending, mark all children as pending
    if *pending {
        run_nodes_pending(children, depth + 1, result);
        return;
    }

    let child_force_focused = force_focused || *focused;

    // Push path segment and this node's hooks before checking runnability
    // so that has_runnable_tests sees the full accumulated context.
    path.push(name.clone());
    let saved_hooks = hooks.push_describe(node);

    let any_runnable =
        has_runnable_tests(children, path, hooks, focus_mode, child_force_focused, config);
    let has_hooks = !before_all.is_empty() || !after_all.is_empty();

    if !any_runnable && has_hooks {
        // Still recurse children so pending/skipped counts are correct,
        // but skip the before_all/after_all hooks.
        // Push a scope layer for symmetry with the normal path (B1 fix).
        crate::push_scope_setup_layer();
        run_nodes(
            children,
            depth + 1,
            path,
            hooks,
            focus_mode,
            child_force_focused,
            config,
            result,
        );
        crate::pop_scope_setup_layer();
        hooks.pop_describe(saved_hooks);
        path.pop();
        return;
    }

    // Push a new scope layer before before_all runs so that values stored by
    // returning before_all hooks are scoped to this describe block and do not
    // clobber outer-scope values when this block ends.
    crate::push_scope_setup_layer();

    // Run before_all once at scope entry.
    // If it panics, skip children but still run after_all.
    let before_all_ok = catch_unwind(AssertUnwindSafe(|| {
        for hook in before_all {
            hook();
        }
    }));

    if let Err(e) = &before_all_ok {
        let msg = panic_message(&**e);
        let full_path = path.join(" > ");
        println!("{indent}  {} before_all failed: {}", red("✗"), red(msg.as_ref()));
        result.failed += 1;
        result.failures.push(format!("{full_path} (before_all): {msg}"));
    } else {
        run_nodes(
            children,
            depth + 1,
            path,
            hooks,
            focus_mode,
            child_force_focused,
            config,
            result,
        );
    }

    // Run after_all once at scope exit — even if before_all failed
    if let Err(e) = catch_unwind(AssertUnwindSafe(|| {
        for hook in after_all {
            hook();
        }
    })) {
        let msg = panic_message(&*e);
        let full_path = path.join(" > ");
        println!("{indent}  {} after_all failed: {}", red("✗"), red(msg.as_ref()));
        result.failed += 1;
        result.failures.push(format!("{full_path} (after_all): {msg}"));
    }

    // Pop this scope's layer, restoring outer-scope before_all values for the same types
    crate::pop_scope_setup_layer();
    hooks.pop_describe(saved_hooks);
    path.pop();
}

/// Run an It node: apply filters, execute the test body with all decorators.
///
/// Handles focus mode, label filtering, path filtering, retries,
/// must_pass_repeatedly, and timeout composition.
#[allow(clippy::too_many_arguments, clippy::too_many_lines, clippy::cognitive_complexity)]
fn run_it_node<'a>(
    node: &'a TestNode,
    depth: usize,
    path: &mut Vec<String>,
    hooks: &mut HookChain<'a>,
    focus_mode: bool,
    force_focused: bool,
    config: &RunConfig,
    result: &mut RunResult,
) {
    let TestNode::It {
        name,
        focused,
        pending,
        labels,
        retries,
        timeout_ms,
        must_pass_repeatedly,
        test_fn,
    } = node
    else {
        unreachable!()
    };

    let indent = "  ".repeat(depth);

    path.push(name.clone());
    let full_path = path.join(" > ");
    path.pop();

    // Filter check
    if let Some(ref f) = config.filter {
        if !full_path.to_lowercase().contains(&f.to_lowercase()) {
            return;
        }
    }

    // Pending
    if *pending {
        println!("{indent}{} {}", yellow("-"), dim(name));
        result.pending += 1;
        return;
    }

    // Focus mode: skip non-focused
    let effectively_focused = *focused || force_focused;
    if focus_mode && !effectively_focused && !config.include_ignored {
        result.skipped += 1;
        return;
    }

    // Fail-on-focus CI check
    if effectively_focused && focus_mode {
        crate::check_fail_on_focus();
    }

    // Label check (merge accumulated + own)
    let all_labels: Vec<&str> = hooks
        .labels
        .iter()
        .copied()
        .chain(labels.iter().map(|s| s.as_str()))
        .collect();
    if !crate::check_labels(&all_labels) {
        return;
    }

    // Execute the test
    let start = Instant::now();

    let test_body = || {
        // Run before_each + just_before_each + test body, catching any panic
        // so that after_each and cleanups are guaranteed to run.
        let body_result = catch_unwind(AssertUnwindSafe(|| {
            for hook in &hooks.before_each {
                hook();
            }
            for hook in &hooks.just_before_each {
                hook();
            }
            test_fn();
        }));

        // after_each (innermost first) — each individually protected
        let mut after_each_panic = None;
        for hook in hooks.after_each.iter().rev() {
            if let Err(e) = catch_unwind(AssertUnwindSafe(hook)) {
                eprintln!("  warning: after_each hook panicked");
                if after_each_panic.is_none() {
                    after_each_panic = Some(e);
                }
            }
        }

        // Deferred cleanups
        crate::run_deferred_cleanups();

        // Clear per-test setup values between tests
        crate::clear_setup_values();

        // Propagate the first failure: body takes priority over after_each
        if let Err(e) = body_result {
            std::panic::resume_unwind(e);
        }
        if let Some(e) = after_each_panic {
            std::panic::resume_unwind(e);
        }
    };

    // Apply decorators compositionally so combinations behave as expected:
    // retries -> must_pass_repeatedly -> timeout (outermost)
    let with_retries = || {
        if let Some(n) = *retries {
            crate::with_retries(n, test_body);
        } else {
            test_body();
        }
    };

    let with_must_pass_repeatedly = || {
        if let Some(n) = *must_pass_repeatedly {
            crate::must_pass_repeatedly(n, with_retries);
        } else {
            with_retries();
        }
    };

    let outcome = if let Some(ms) = *timeout_ms {
        run_with_timeout(ms, &with_must_pass_repeatedly)
    } else {
        catch_unwind(AssertUnwindSafe(with_must_pass_repeatedly))
    };

    // Check if the test called skip!() — report as skipped, not passed
    if outcome.is_ok() {
        if let Some(reason) = crate::take_skip_reason() {
            println!(
                "{indent}{} {} {}",
                yellow("-"),
                dim(name),
                dim(&format!("({reason})"))
            );
            result.skipped += 1;
        } else {
            report_outcome(&indent, name, &full_path, outcome, start, result);
        }
    } else {
        // Clear any skip flag set before the panic
        let _ = crate::take_skip_reason();
        report_outcome(&indent, name, &full_path, outcome, start, result);
    }
}

/// Run an Ordered node: execute steps in sequence with fail-fast semantics.
///
/// Applies before_each/after_each hooks around the entire sequence.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn run_ordered_node<'a>(
    node: &'a TestNode,
    depth: usize,
    path: &mut Vec<String>,
    hooks: &mut HookChain<'a>,
    focus_mode: bool,
    force_focused: bool,
    config: &RunConfig,
    result: &mut RunResult,
) {
    let TestNode::Ordered {
        name,
        labels,
        continue_on_failure,
        steps,
    } = node
    else {
        unreachable!()
    };

    let indent = "  ".repeat(depth);

    path.push(name.clone());
    let full_path = path.join(" > ");
    path.pop();

    // Filter check
    if let Some(ref f) = config.filter {
        if !full_path.to_lowercase().contains(&f.to_lowercase()) {
            return;
        }
    }

    // Focus mode: skip non-focused ordered tests unless include_ignored is set.
    if focus_mode && !force_focused && !config.include_ignored {
        result.skipped += 1;
        return;
    }

    // Fail-on-focus CI check for ordered tests inside focused containers.
    if force_focused && focus_mode {
        crate::check_fail_on_focus();
    }

    // Label check
    let all_labels: Vec<&str> = hooks
        .labels
        .iter()
        .copied()
        .chain(labels.iter().map(|s| s.as_str()))
        .collect();
    if !crate::check_labels(&all_labels) {
        return;
    }

    let start = Instant::now();

    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // Run before_each + just_before_each + steps, catching any panic
        // so that after_each and cleanups are guaranteed to run.
        let body_result = catch_unwind(AssertUnwindSafe(|| {
            for hook in &hooks.before_each {
                hook();
            }
            for hook in &hooks.just_before_each {
                hook();
            }

            let mut failures: Vec<Box<dyn std::any::Any + Send>> = Vec::new();
            let total = steps.len();

            for (i, step) in steps.iter().enumerate() {
                eprintln!("  [{}/{}] {}", i + 1, total, step.name);
                if *continue_on_failure {
                    if let Err(e) = catch_unwind(AssertUnwindSafe(|| (step.body)())) {
                        failures.push(e);
                    }
                } else {
                    (step.body)();
                }
            }

            if !failures.is_empty() {
                panic!(
                    "{} of {} ordered steps failed",
                    failures.len(),
                    steps.len()
                );
            }
        }));

        // after_each (innermost first) — each individually protected
        let mut after_each_panic = None;
        for hook in hooks.after_each.iter().rev() {
            if let Err(e) = catch_unwind(AssertUnwindSafe(hook)) {
                eprintln!("  warning: after_each hook panicked");
                if after_each_panic.is_none() {
                    after_each_panic = Some(e);
                }
            }
        }

        crate::run_deferred_cleanups();

        // Clear per-test setup values (mirrors the It path)
        crate::clear_setup_values();

        // Propagate the first failure: body takes priority over after_each
        if let Err(e) = body_result {
            std::panic::resume_unwind(e);
        }
        if let Some(e) = after_each_panic {
            std::panic::resume_unwind(e);
        }
    }));

    report_outcome(&indent, name, &full_path, outcome, start, result);
}

/// Mark all descendant It nodes as pending (for xdescribe).
fn run_nodes_pending(nodes: &[TestNode], depth: usize, result: &mut RunResult) {
    let indent = "  ".repeat(depth);
    for node in nodes {
        match node {
            TestNode::Describe { name, children, .. } => {
                println!("{indent}{}", bold(&dim(name)));
                run_nodes_pending(children, depth + 1, result);
            }
            TestNode::It { name, .. } => {
                println!("{indent}{} {}", yellow("-"), dim(name));
                result.pending += 1;
            }
            TestNode::Ordered { name, .. } => {
                println!("{indent}{} {}", yellow("-"), dim(name));
                result.pending += 1;
            }
        }
    }
}

fn report_outcome(
    indent: &str,
    name: &str,
    full_path: &str,
    outcome: Result<(), Box<dyn std::any::Any + Send>>,
    start: Instant,
    result: &mut RunResult,
) {
    let elapsed = start.elapsed();
    let ms = elapsed.as_millis();
    let time_str = if ms > 100 {
        format!(" {}", dim(&format!("({ms}ms)")))
    } else {
        String::new()
    };

    match outcome {
        Ok(()) => {
            println!("{indent}{} {}{}", green("✓"), name, time_str);
            result.passed += 1;
        }
        Err(e) => {
            let msg = panic_message(&*e);
            println!("{indent}{} {}{}", red("✗"), red(name), time_str);
            println!("{indent}  {}", red(&format!("Error: {msg}")));
            result.failed += 1;
            result.failures.push(format!("{full_path}: {msg}"));
        }
    }
}

/// Run a closure with a timeout.
///
/// The closure runs on the current thread. The timeout is checked *after*
/// the closure returns — the closure cannot be forcibly aborted mid-execution.
/// If the closure finishes within the deadline, its result is returned as-is.
/// If it exceeds the deadline, a timeout error is returned regardless of
/// whether the closure itself succeeded or failed.
fn run_with_timeout(
    ms: u64,
    f: &dyn Fn(),
) -> Result<(), Box<dyn std::any::Any + Send>> {
    use std::time::Duration;

    let start = Instant::now();
    let deadline = Duration::from_millis(ms);

    // Run the closure on the current thread
    // (Cleanups are already handled inside test_body before any panic re-raises.)
    let result = catch_unwind(AssertUnwindSafe(|| {
        f();
    }));

    // Check if the closure exceeded the deadline
    if start.elapsed() > deadline {
        // If the test also panicked, include the original error
        if let Err(e) = result {
            let msg = panic_message(&*e);
            Err(Box::new(format!("test timed out after {ms}ms (original error: {msg})")))
        } else {
            Err(Box::new(format!("test timed out after {ms}ms")))
        }
    } else {
        result
    }
}

fn print_summary(result: &RunResult, elapsed: std::time::Duration) {
    let elapsed_str = format!("{:.3}s", elapsed.as_secs_f64());

    let mut parts: Vec<String> = [
        (result.passed > 0).then(|| green(&format!("{} passed", result.passed))),
        (result.failed > 0).then(|| red(&format!("{} failed", result.failed))),
        (result.pending > 0).then(|| yellow(&format!("{} pending", result.pending))),
        (result.skipped > 0).then(|| dim(&format!("{} skipped", result.skipped))),
    ]
    .into_iter()
    .flatten()
    .collect();

    // Avoid an empty summary line when all tests are filtered out
    if parts.is_empty() {
        parts.push(dim("0 matched"));
    }

    let summary = format!("{} ({})", parts.join(", "), dim(&elapsed_str));

    println!();
    if result.failed > 0 {
        println!("{}", red("FAIL"));
        println!("{summary}");
        println!();
        println!("Failures:");
        for (i, failure) in result.failures.iter().enumerate() {
            println!("  {}. {}", i + 1, failure);
        }
        println!();
    } else {
        println!("{}", green("PASS"));
        println!("{summary}");
    }
}

fn list_tree(nodes: &[TestNode], path: &mut Vec<String>, config: &RunConfig) {
    for node in nodes {
        match node {
            TestNode::Describe { name, children, .. } => {
                path.push(name.clone());
                list_tree(children, path, config);
                path.pop();
            }
            TestNode::It { name, pending, .. } => {
                path.push(name.clone());
                let full_path = path.join(" > ");
                path.pop();

                if let Some(ref f) = config.filter {
                    if !full_path.to_lowercase().contains(&f.to_lowercase()) {
                        continue;
                    }
                }

                if *pending {
                    println!("{full_path} (pending)");
                } else {
                    println!("{full_path}");
                }
            }
            TestNode::Ordered { name, .. } => {
                path.push(name.clone());
                let full_path = path.join(" > ");
                path.pop();

                if let Some(ref f) = config.filter {
                    if !full_path.to_lowercase().contains(&f.to_lowercase()) {
                        continue;
                    }
                }

                println!("{full_path}");
            }
        }
    }
}

fn tree_has_focus(nodes: &[TestNode]) -> bool {
    nodes.iter().any(|node| match node {
        TestNode::It { focused, .. } => *focused,
        TestNode::Describe {
            focused, children, ..
        } => *focused || tree_has_focus(children),
        TestNode::Ordered { .. } => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::time::Duration;

    #[test]
    fn ordered_is_skipped_when_focus_mode_is_active() {
        static ORDERED_RAN: AtomicBool = AtomicBool::new(false);
        ORDERED_RAN.store(false, Ordering::SeqCst);

        let nodes = vec![TestNode::describe(
            "root",
            vec![
                TestNode::fit("focused", || {}),
                TestNode::Ordered {
                    name: "ordered".to_string(),
                    labels: Vec::new(),
                    continue_on_failure: false,
                    steps: vec![OrderedStep {
                        name: "step".to_string(),
                        body: Box::new(|| {
                            ORDERED_RAN.store(true, Ordering::SeqCst);
                        }),
                    }],
                },
            ],
        )];

        let config = RunConfig {
            filter: None,
            list: false,
            include_ignored: false,
        };
        let result = run_tree(&nodes, &config);

        assert_eq!(result.failed, 0);
        assert_eq!(result.passed, 1);
        assert_eq!(result.skipped, 1);
        assert!(!ORDERED_RAN.load(Ordering::SeqCst));
    }

    // C3 regression: skip!() should report as skipped, not passed
    #[test]
    fn skip_reports_as_skipped_not_passed() {
        let nodes = vec![TestNode::it("skippable", || {
            crate::skip("not ready");
            // skip!() macro does `skip() + return`, but we can't use the macro
            // in a Fn closure, so just call skip() — the runner checks the flag
            // regardless of whether the closure returned early.
        })];

        let config = RunConfig {
            filter: None,
            list: false,
            include_ignored: false,
        };
        let result = run_tree(&nodes, &config);

        assert_eq!(result.skipped, 1, "should be reported as skipped");
        assert_eq!(result.passed, 0, "should not be reported as passed");
        assert_eq!(result.failed, 0);
    }

    // I1 regression: before_all panic should fail gracefully, not abort
    #[test]
    fn before_all_panic_reports_failure_and_runs_after_all() {
        static AFTER_ALL_RAN: AtomicBool = AtomicBool::new(false);
        AFTER_ALL_RAN.store(false, Ordering::SeqCst);

        let nodes = vec![TestNode::describe_with_hooks(
            "broken setup",
            vec![Box::new(|| panic!("setup exploded"))],
            vec![Box::new(|| {
                AFTER_ALL_RAN.store(true, Ordering::SeqCst);
            })],
            vec![TestNode::it("should not run", || {
                panic!("child should be skipped");
            })],
        )];

        let config = RunConfig {
            filter: None,
            list: false,
            include_ignored: false,
        };
        let result = run_tree(&nodes, &config);

        assert_eq!(result.failed, 1, "before_all failure counted");
        assert_eq!(result.passed, 0, "child should not have run");
        assert!(AFTER_ALL_RAN.load(Ordering::SeqCst), "after_all must still run");
    }

    // I1 regression: after_all panic should report failure
    #[test]
    fn after_all_panic_reports_failure() {
        let nodes = vec![TestNode::describe_with_hooks(
            "broken teardown",
            vec![],
            vec![Box::new(|| panic!("teardown exploded"))],
            vec![TestNode::it("passes", || {})],
        )];

        let config = RunConfig {
            filter: None,
            list: false,
            include_ignored: false,
        };
        let result = run_tree(&nodes, &config);

        assert_eq!(result.passed, 1, "test itself passed");
        assert_eq!(result.failed, 1, "after_all failure counted");
    }

    // I3 regression: one cleanup panic should not prevent other cleanups
    #[test]
    fn deferred_cleanup_panic_does_not_skip_remaining() {
        static SECOND_CLEANUP_RAN: AtomicBool = AtomicBool::new(false);
        SECOND_CLEANUP_RAN.store(false, Ordering::SeqCst);

        let nodes = vec![TestNode::it("cleanup test", || {
            // First registered = runs last (LIFO)
            crate::defer_cleanup(|| {
                SECOND_CLEANUP_RAN.store(true, Ordering::SeqCst);
            });
            // Second registered = runs first, and panics
            crate::defer_cleanup(|| {
                panic!("cleanup boom");
            });
        })];

        let config = RunConfig {
            filter: None,
            list: false,
            include_ignored: false,
        };
        let result = run_tree(&nodes, &config);

        // The test body itself passed, but cleanup panicked → reported as failure
        assert_eq!(result.failed, 1);
        assert!(
            SECOND_CLEANUP_RAN.load(Ordering::SeqCst),
            "second cleanup must run despite first panicking"
        );
    }

    // C1 regression: before_each panic must still run after_each
    #[test]
    fn before_each_panic_still_runs_after_each() {
        static AFTER_EACH_RAN: AtomicBool = AtomicBool::new(false);
        AFTER_EACH_RAN.store(false, Ordering::SeqCst);

        let nodes = vec![TestNode::describe_with_each_hooks(
            "broken before_each",
            vec![Box::new(|| panic!("before_each exploded"))],
            vec![Box::new(|| {
                AFTER_EACH_RAN.store(true, Ordering::SeqCst);
            })],
            vec![TestNode::it("test", || {})],
        )];

        let config = RunConfig {
            filter: None,
            list: false,
            include_ignored: false,
        };
        let result = run_tree(&nodes, &config);

        assert_eq!(result.failed, 1, "before_each failure reported");
        assert!(AFTER_EACH_RAN.load(Ordering::SeqCst), "after_each must still run");
    }

    // C2 regression: after_each panic must not lose the original test failure
    #[test]
    fn after_each_panic_preserves_test_failure() {
        let nodes = vec![TestNode::describe_with_each_hooks(
            "both fail",
            vec![],
            vec![Box::new(|| panic!("after_each exploded"))],
            vec![TestNode::it("fails", || {
                panic!("test body failed");
            })],
        )];

        let config = RunConfig {
            filter: None,
            list: false,
            include_ignored: false,
        };
        let result = run_tree(&nodes, &config);

        assert_eq!(result.failed, 1);
        // The failure message should contain the body's error, not after_each's
        assert!(
            result.failures[0].contains("test body failed"),
            "original test failure must be reported, got: {}",
            result.failures[0]
        );
    }

    // C2 regression: one after_each panic must not skip remaining after_each hooks
    #[test]
    fn after_each_panic_runs_remaining_hooks() {
        static SECOND_AFTER_EACH_RAN: AtomicBool = AtomicBool::new(false);
        SECOND_AFTER_EACH_RAN.store(false, Ordering::SeqCst);

        // Outer describe has one after_each, inner describe has another that panics.
        // The outer after_each must still run (after_each runs innermost first).
        let inner = TestNode::describe_with_each_hooks(
            "inner",
            vec![],
            vec![Box::new(|| panic!("inner after_each panicked"))],
            vec![TestNode::it("test", || {})],
        );
        let outer = TestNode::describe_with_each_hooks(
            "outer",
            vec![],
            vec![Box::new(|| {
                SECOND_AFTER_EACH_RAN.store(true, Ordering::SeqCst);
            })],
            vec![inner],
        );

        let config = RunConfig {
            filter: None,
            list: false,
            include_ignored: false,
        };
        let result = run_tree(&[outer], &config);

        assert_eq!(result.failed, 1);
        assert!(
            SECOND_AFTER_EACH_RAN.load(Ordering::SeqCst),
            "outer after_each must still run despite inner after_each panicking"
        );
    }

    // I7 regression: mixed +, filter is rejected
    #[test]
    fn mixed_and_or_filter_is_rejected() {
        assert!(!crate::labels_match_filter(&["a", "b"], "a+b,c"));
    }

    #[test]
    fn retries_and_timeout_compose() {
        static ATTEMPTS: AtomicU32 = AtomicU32::new(0);
        ATTEMPTS.store(0, Ordering::SeqCst);

        let nodes = vec![TestNode::It {
            name: "combined".to_string(),
            focused: false,
            pending: false,
            labels: Vec::new(),
            retries: Some(2),
            timeout_ms: Some(5),
            must_pass_repeatedly: None,
            test_fn: Box::new(|| {
                let n = ATTEMPTS.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(10));
                assert!(n >= 2, "attempt {n}");
            }),
        }];

        let config = RunConfig {
            filter: None,
            list: false,
            include_ignored: false,
        };
        let result = run_tree(&nodes, &config);

        assert_eq!(ATTEMPTS.load(Ordering::SeqCst), 3);
        assert_eq!(result.failed, 1);
    }

    #[test]
    fn retries_and_must_pass_repeatedly_compose() {
        static ATTEMPTS: AtomicU32 = AtomicU32::new(0);
        ATTEMPTS.store(0, Ordering::SeqCst);

        let nodes = vec![TestNode::It {
            name: "combined".to_string(),
            focused: false,
            pending: false,
            labels: Vec::new(),
            retries: Some(1),
            timeout_ms: None,
            must_pass_repeatedly: Some(2),
            test_fn: Box::new(|| {
                let n = ATTEMPTS.fetch_add(1, Ordering::SeqCst);
                assert!(n > 0, "first call should fail and retry");
            }),
        }];

        let config = RunConfig {
            filter: None,
            list: false,
            include_ignored: false,
        };
        let result = run_tree(&nodes, &config);

        assert_eq!(ATTEMPTS.load(Ordering::SeqCst), 3);
        assert_eq!(result.failed, 0);
        assert_eq!(result.passed, 1);
    }

    // ---- detect_libtest_args regression tests ----

    fn args(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn detect_libtest_args_catches_format() {
        assert!(detect_libtest_args(&args(&["--format=json"])).is_some());
        assert!(detect_libtest_args(&args(&["--format=pretty"])).is_some());
        assert!(detect_libtest_args(&args(&["--format", "json"])).is_some());
    }

    #[test]
    fn detect_libtest_args_catches_test_threads() {
        assert!(detect_libtest_args(&args(&["--test-threads=4"])).is_some());
        assert!(detect_libtest_args(&args(&["--test-threads", "2"])).is_some());
    }

    #[test]
    fn detect_libtest_args_catches_other_libtest_flags() {
        assert!(detect_libtest_args(&args(&["--show-output"])).is_some());
        assert!(detect_libtest_args(&args(&["--logfile", "out.log"])).is_some());
        assert!(detect_libtest_args(&args(&["-Zunstable-options"])).is_some());
    }

    #[test]
    fn detect_libtest_args_ignores_rsspec_args() {
        assert!(detect_libtest_args(&args(&["--list"])).is_none());
        assert!(detect_libtest_args(&args(&["--include-ignored"])).is_none());
        assert!(detect_libtest_args(&args(&["my_filter"])).is_none());
        assert!(detect_libtest_args(&args(&[])).is_none());
    }
}
