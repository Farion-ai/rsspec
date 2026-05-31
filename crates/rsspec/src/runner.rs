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
    pub body: crate::TestBody,
}

/// A node in the BDD test tree.
pub(crate) enum TestNode {
    /// A describe/context/when container.
    Describe {
        name: String,
        focused: bool,
        pending: bool,
        labels: Vec<String>,
        before_each: Vec<crate::TestBody>,
        after_each: Vec<crate::TestBody>,
        before_all: Vec<crate::TestBody>,
        after_all: Vec<crate::TestBody>,
        just_before_each: Vec<crate::TestBody>,
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
        test_fn: crate::TestBody,
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
        before_all: Vec<crate::TestBody>,
        after_all: Vec<crate::TestBody>,
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
        before_each: Vec<crate::TestBody>,
        after_each: Vec<crate::TestBody>,
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

    fn it(name: impl Into<String>, f: impl Fn() + crate::MaybeSend + 'static) -> Self {
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

    fn fit(name: impl Into<String>, f: impl Fn() + crate::MaybeSend + 'static) -> Self {
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
            // Coerce `&(dyn Fn() + Send)` (parallel build) or `&dyn Fn()`
            // (sequential build) to a uniform `&dyn Fn()` chain element.
            self.before_each
                .extend(before_each.iter().map(|b| b.as_ref() as &dyn Fn()));
            self.after_each
                .extend(after_each.iter().map(|b| b.as_ref() as &dyn Fn()));
            self.just_before_each
                .extend(just_before_each.iter().map(|b| b.as_ref() as &dyn Fn()));
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

impl RunResult {
    /// Fold another result into this one, summing counters and appending
    /// failures. Merge in tree order so failure numbering stays deterministic.
    #[cfg_attr(not(feature = "parallel"), allow(dead_code))]
    fn merge(&mut self, other: RunResult) {
        self.passed += other.passed;
        self.failed += other.failed;
        self.pending += other.pending;
        self.skipped += other.skipped;
        self.failures.extend(other.failures);
    }
}

/// Where tree output is written. The sequential path writes straight to stdout
/// (streaming, byte-for-byte unchanged); parallel workers write into a per-unit
/// buffer that the main thread flushes in tree order.
pub(crate) enum Sink<'a> {
    Stdout,
    #[cfg_attr(not(feature = "parallel"), allow(dead_code))]
    Buffer(&'a mut String),
}

impl Sink<'_> {
    /// Write a line (text followed by a newline).
    fn line(&mut self, s: &str) {
        match self {
            Sink::Stdout => println!("{s}"),
            Sink::Buffer(buf) => {
                buf.push_str(s);
                buf.push('\n');
            }
        }
    }

    /// Write a blank line.
    fn blank(&mut self) {
        match self {
            Sink::Stdout => println!(),
            Sink::Buffer(buf) => buf.push('\n'),
        }
    }
}

/// Run-invariant context threaded through the tree walk: the parsed config, the
/// focus-mode flag, and the output sink. Bundling these into one argument keeps
/// the recursive walk functions under the argument-count lint. The mutable
/// `RunResult` accumulator is passed separately so a worker can return its
/// buffer and result without partial-move gymnastics.
struct Ctx<'s> {
    config: &'s RunConfig,
    focus_mode: bool,
    sink: Sink<'s>,
}

/// Configuration parsed from command-line args.
pub(crate) struct RunConfig {
    /// Filter string — only run tests whose full path contains this.
    pub filter: Option<String>,
    /// Only list tests, don't run them.
    pub list: bool,
    /// Include ignored/pending tests in the run.
    pub include_ignored: bool,
    /// Number of worker threads for top-level parallelism. `1` = sequential.
    pub parallelism: usize,
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
        let mut parallel_spec: Option<String> = None;

        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--list" => list = true,
                "--include-ignored" | "--ignored" => include_ignored = true,
                "--parallel" => {
                    // Accept an optional following value (`--parallel 4`,
                    // `--parallel auto`); otherwise default to `auto`.
                    let next_is_spec = args.get(i + 1).is_some_and(|n| {
                        n.eq_ignore_ascii_case("auto") || n.parse::<usize>().is_ok()
                    });
                    if next_is_spec {
                        parallel_spec = Some(args[i + 1].clone());
                        i += 1;
                    } else {
                        parallel_spec = Some("auto".to_string());
                    }
                }
                a if a.starts_with("--parallel=") => {
                    parallel_spec = Some(a["--parallel=".len()..].to_string());
                }
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
            parallelism: resolve_parallelism(parallel_spec.as_deref()),
        }
    }
}

/// Parse a parallelism spec: a positive integer, or `auto` for the detected
/// core count. Returns `None` for an unparseable spec.
fn parse_parallel_spec(s: &str) -> Option<usize> {
    if s.eq_ignore_ascii_case("auto") {
        Some(
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
        )
    } else {
        s.parse::<usize>().ok().map(|n| n.max(1))
    }
}

/// Resolve the effective worker count: CLI spec > `RSSPEC_PARALLEL` env > 1.
/// Clamped to 1 when the `parallel` feature is not compiled in.
fn resolve_parallelism(cli: Option<&str>) -> usize {
    let spec = cli.map(str::to_string).or_else(|| {
        std::env::var("RSSPEC_PARALLEL")
            .ok()
            .filter(|s| !s.is_empty())
    });
    let requested = spec.as_deref().and_then(parse_parallel_spec).unwrap_or(1);
    clamp_parallelism(requested)
}

#[cfg(feature = "parallel")]
fn clamp_parallelism(n: usize) -> usize {
    n.max(1)
}

#[cfg(not(feature = "parallel"))]
fn clamp_parallelism(n: usize) -> usize {
    if n > 1 {
        eprintln!(
            "rsspec: parallel execution requested but the `parallel` feature is not enabled; \
             running sequentially. Rebuild with `--features parallel`."
        );
    }
    1
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

    let mut ctx = Ctx {
        config,
        focus_mode,
        sink: Sink::Stdout,
    };
    ctx.sink.blank();
    let mut hooks = HookChain::default();
    let mut path = Vec::new();
    run_nodes(
        nodes,
        0,
        &mut path,
        &mut hooks,
        false,
        &mut ctx,
        &mut result,
    );
    print_summary(&result, start.elapsed());

    result
}

/// Run multiple named suites, printing a header per suite and a combined summary.
pub(crate) fn run_suites(suites: Vec<Suite>, config: &RunConfig) -> RunResult {
    let focus_mode = suites.iter().any(|s| tree_has_focus(&s.nodes));
    let start = Instant::now();

    if config.list {
        let mut path = Vec::new();
        for suite in &suites {
            list_tree(&suite.nodes, &mut path, config);
        }
        return RunResult::default();
    }

    if config.parallelism <= 1 {
        return run_suites_sequential(&suites, focus_mode, config, start);
    }

    // `parallelism` is clamped to 1 when the `parallel` feature is off, so this
    // branch is only reachable in feature-on builds. The fallback keeps the
    // feature-off build compiling without requiring `TestNode: Send`.
    #[cfg(feature = "parallel")]
    {
        run_suites_parallel(suites, focus_mode, config, start)
    }
    #[cfg(not(feature = "parallel"))]
    {
        run_suites_sequential(&suites, focus_mode, config, start)
    }
}

/// Sequential execution: stream each suite's tree to stdout as it runs.
/// Behaviorally identical to the pre-parallel runner.
fn run_suites_sequential(
    suites: &[Suite],
    focus_mode: bool,
    config: &RunConfig,
    start: Instant,
) -> RunResult {
    let mut result = RunResult::default();
    let mut ctx = Ctx {
        config,
        focus_mode,
        sink: Sink::Stdout,
    };
    ctx.sink.blank();

    for suite in suites {
        if !suite.name.is_empty() {
            ctx.sink.line(&dim(&format!("--- {} ---", suite.name)));
            ctx.sink.blank();
        }

        let mut hooks = HookChain::default();
        let mut path = Vec::new();
        run_nodes(
            &suite.nodes,
            0,
            &mut path,
            &mut hooks,
            false,
            &mut ctx,
            &mut result,
        );

        if suites.len() > 1 {
            ctx.sink.blank();
        }
    }

    print_summary(&result, start.elapsed());

    result
}

/// Parallel execution: render each top-level node on a worker thread into its
/// own buffer, then flush buffers in tree order and merge results.
///
/// The unit of parallelism is one top-level node (a top-level
/// `describe`/`it`/`ordered`). Because the fixture stores are thread-locals, an
/// entire subtree running on one worker keeps `before_all`-once and per-test
/// fixture isolation intact — distinct top-level subtrees never share state.
#[cfg(feature = "parallel")]
fn run_suites_parallel(
    suites: Vec<Suite>,
    focus_mode: bool,
    config: &RunConfig,
    start: Instant,
) -> RunResult {
    let (body, result) = render_suites_parallel(suites, focus_mode, config);
    print!("{body}");
    print_summary(&result, start.elapsed());
    result
}

/// Per-suite output layout captured before nodes are consumed by the workers.
#[cfg(feature = "parallel")]
struct SuiteLayout {
    name: String,
    node_count: usize,
}

/// Run all top-level units across worker threads and reassemble their buffered
/// output in tree order. Returns the assembled body (without the summary) and
/// the merged result. Factored out so tests can assert on the exact bytes and
/// ordering without going through stdout.
///
/// Three phases, each its own helper: flatten suites into ordered units, run
/// the units across workers, then reassemble buffers in tree order.
#[cfg(feature = "parallel")]
fn render_suites_parallel(
    suites: Vec<Suite>,
    focus_mode: bool,
    config: &RunConfig,
) -> (String, RunResult) {
    let (layout, units) = flatten_units(suites);
    let rendered = run_units(units, focus_mode, config);
    reassemble(&layout, rendered)
}

/// Flatten suites into `(layout, units)`, consuming nodes. `layout` records the
/// per-suite output structure; `units` are `(tree_order, node)` for the workers.
#[cfg(feature = "parallel")]
fn flatten_units(suites: Vec<Suite>) -> (Vec<SuiteLayout>, Vec<(usize, TestNode)>) {
    let mut layout: Vec<SuiteLayout> = Vec::new();
    let mut units: Vec<(usize, TestNode)> = Vec::new();
    for suite in suites {
        let Suite { name, nodes } = suite;
        layout.push(SuiteLayout {
            name,
            node_count: nodes.len(),
        });
        for node in nodes {
            let order = units.len();
            units.push((order, node));
        }
    }
    (layout, units)
}

/// Render every unit on a worker thread, returning per-`order` buffers+results.
/// Units are round-robined into chunks and *moved* into scoped workers, so each
/// worker needs only `TestNode: Send` (not `Sync` — nothing is shared).
#[cfg(feature = "parallel")]
fn run_units(
    units: Vec<(usize, TestNode)>,
    focus_mode: bool,
    config: &RunConfig,
) -> Vec<Option<(String, RunResult)>> {
    let total = units.len();
    let mut rendered: Vec<Option<(String, RunResult)>> = (0..total).map(|_| None).collect();
    if total == 0 {
        return rendered;
    }

    let n_workers = config.parallelism.min(total);
    let mut chunks: Vec<Vec<(usize, TestNode)>> = (0..n_workers).map(|_| Vec::new()).collect();
    for (i, unit) in units.into_iter().enumerate() {
        chunks[i % n_workers].push(unit);
    }

    std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .into_iter()
                        .map(|(order, node)| {
                            // render_unit already catches test-body panics; an
                            // escape here is a framework bug. Tag it with the
                            // unit index before propagating so the abort is
                            // localizable, instead of an anonymous worker panic.
                            match catch_unwind(AssertUnwindSafe(|| {
                                render_unit(&node, focus_mode, config)
                            })) {
                                Ok((buf, res)) => (order, buf, res),
                                Err(e) => {
                                    eprintln!(
                                        "rsspec: internal panic while rendering top-level unit #{order}"
                                    );
                                    std::panic::resume_unwind(e);
                                }
                            }
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();

        for handle in handles {
            let chunk_results = handle.join().expect("rsspec: worker thread panicked");
            for (order, buf, res) in chunk_results {
                rendered[order] = Some((buf, res));
            }
        }
    });

    rendered
}

/// Reassemble worker buffers into the final body in tree order: leading blank,
/// then per-suite headers and unit buffers, merging results in the same order
/// so failure numbering stays deterministic.
#[cfg(feature = "parallel")]
fn reassemble(
    layout: &[SuiteLayout],
    mut rendered: Vec<Option<(String, RunResult)>>,
) -> (String, RunResult) {
    let multi_suite = layout.len() > 1;
    let mut body = String::from("\n");
    let mut result = RunResult::default();
    let mut order = 0;
    for suite in layout {
        if !suite.name.is_empty() {
            body.push_str(&dim(&format!("--- {} ---", suite.name)));
            body.push('\n');
            body.push('\n');
        }
        for _ in 0..suite.node_count {
            let (buf, res) = rendered[order]
                .take()
                .expect("rsspec: missing unit output — internal error");
            body.push_str(&buf);
            result.merge(res);
            order += 1;
        }
        if multi_suite {
            body.push('\n');
        }
    }

    (body, result)
}

/// Render a single top-level node (a whole subtree) into a buffer on the
/// current worker thread. The thread-local fixture stores are this thread's
/// own, giving each subtree natural isolation.
#[cfg(feature = "parallel")]
fn render_unit(node: &TestNode, focus_mode: bool, config: &RunConfig) -> (String, RunResult) {
    let mut buf = String::new();
    let mut result = RunResult::default();
    {
        let mut ctx = Ctx {
            config,
            focus_mode,
            sink: Sink::Buffer(&mut buf),
        };
        let mut hooks = HookChain::default();
        let mut path = Vec::new();
        run_node(node, 0, &mut path, &mut hooks, false, &mut ctx, &mut result);
    } // ctx (and its borrow of buf) dropped here
    (buf, result)
}

/// Check if any tests in this subtree will actually execute, considering
/// focus mode, label filters, path filters, and pending status.
///
/// Used to skip `before_all`/`after_all` when all children are filtered out.
#[allow(clippy::too_many_lines)]
fn has_runnable_tests<'a>(
    nodes: &'a [TestNode],
    path: &mut Vec<String>,
    hooks: &mut HookChain<'a>,
    force_focused: bool,
    ctx: &Ctx,
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
                let has_any = has_runnable_tests(children, path, hooks, child_force_focused, ctx);
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
                if let Some(ref f) = ctx.config.filter {
                    if !full_path.to_lowercase().contains(&f.to_lowercase()) {
                        continue;
                    }
                }
                let effectively_focused = *focused || force_focused;
                if ctx.focus_mode && !effectively_focused && !ctx.config.include_ignored {
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
                if let Some(ref f) = ctx.config.filter {
                    if !full_path.to_lowercase().contains(&f.to_lowercase()) {
                        continue;
                    }
                }
                if ctx.focus_mode && !force_focused && !ctx.config.include_ignored {
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

fn run_nodes<'a>(
    nodes: &'a [TestNode],
    depth: usize,
    path: &mut Vec<String>,
    hooks: &mut HookChain<'a>,
    force_focused: bool,
    ctx: &mut Ctx,
    result: &mut RunResult,
) {
    for node in nodes {
        run_node(node, depth, path, hooks, force_focused, ctx, result);
    }
}

/// Dispatch a single test node to the appropriate handler.
fn run_node<'a>(
    node: &'a TestNode,
    depth: usize,
    path: &mut Vec<String>,
    hooks: &mut HookChain<'a>,
    force_focused: bool,
    ctx: &mut Ctx,
    result: &mut RunResult,
) {
    match node {
        TestNode::Describe { .. } => {
            run_describe_node(node, depth, path, hooks, force_focused, ctx, result);
        }
        TestNode::It { .. } => {
            run_it_node(node, depth, path, hooks, force_focused, ctx, result);
        }
        TestNode::Ordered { .. } => {
            run_ordered_node(node, depth, path, hooks, force_focused, ctx, result);
        }
    }
}

/// Run a Describe node: print its name, run `before_all`/`after_all`, recurse.
///
/// Manages the scope-setup stack push/pop and hook-chain push/pop so that
/// `before_all` values and inherited hooks are correctly scoped and restored.
#[allow(clippy::too_many_lines)]
fn run_describe_node<'a>(
    node: &'a TestNode,
    depth: usize,
    path: &mut Vec<String>,
    hooks: &mut HookChain<'a>,
    force_focused: bool,
    ctx: &mut Ctx,
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
    ctx.sink.line(&format!("{indent}{}", bold(name)));

    // If this describe is pending, mark all children as pending
    if *pending {
        run_nodes_pending(children, depth + 1, &mut ctx.sink, result);
        return;
    }

    let child_force_focused = force_focused || *focused;

    // Push path segment and this node's hooks before checking runnability
    // so that has_runnable_tests sees the full accumulated context.
    path.push(name.clone());
    let saved_hooks = hooks.push_describe(node);

    let any_runnable = has_runnable_tests(children, path, hooks, child_force_focused, ctx);
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
            child_force_focused,
            ctx,
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
        ctx.sink.line(&format!(
            "{indent}  {} before_all failed: {}",
            red("✗"),
            red(msg.as_ref())
        ));
        result.failed += 1;
        result
            .failures
            .push(format!("{full_path} (before_all): {msg}"));
    } else {
        run_nodes(
            children,
            depth + 1,
            path,
            hooks,
            child_force_focused,
            ctx,
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
        ctx.sink.line(&format!(
            "{indent}  {} after_all failed: {}",
            red("✗"),
            red(msg.as_ref())
        ));
        result.failed += 1;
        result
            .failures
            .push(format!("{full_path} (after_all): {msg}"));
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
#[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
fn run_it_node<'a>(
    node: &'a TestNode,
    depth: usize,
    path: &mut Vec<String>,
    hooks: &mut HookChain<'a>,
    force_focused: bool,
    ctx: &mut Ctx,
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
    if let Some(ref f) = ctx.config.filter {
        if !full_path.to_lowercase().contains(&f.to_lowercase()) {
            return;
        }
    }

    // Pending
    if *pending {
        ctx.sink
            .line(&format!("{indent}{} {}", yellow("-"), dim(name)));
        result.pending += 1;
        return;
    }

    // Focus mode: skip non-focused
    let effectively_focused = *focused || force_focused;
    if ctx.focus_mode && !effectively_focused && !ctx.config.include_ignored {
        result.skipped += 1;
        return;
    }

    // Fail-on-focus CI check
    if effectively_focused && ctx.focus_mode {
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
            ctx.sink.line(&format!(
                "{indent}{} {} {}",
                yellow("-"),
                dim(name),
                dim(&format!("({reason})"))
            ));
            result.skipped += 1;
        } else {
            report_outcome(
                &indent,
                name,
                &full_path,
                outcome,
                start,
                &mut ctx.sink,
                result,
            );
        }
    } else {
        // Clear any skip flag set before the panic
        let _ = crate::take_skip_reason();
        report_outcome(
            &indent,
            name,
            &full_path,
            outcome,
            start,
            &mut ctx.sink,
            result,
        );
    }
}

/// Run an Ordered node: execute steps in sequence with fail-fast semantics.
///
/// Applies before_each/after_each hooks around the entire sequence.
#[allow(clippy::too_many_lines)]
fn run_ordered_node<'a>(
    node: &'a TestNode,
    depth: usize,
    path: &mut Vec<String>,
    hooks: &mut HookChain<'a>,
    force_focused: bool,
    ctx: &mut Ctx,
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
    if let Some(ref f) = ctx.config.filter {
        if !full_path.to_lowercase().contains(&f.to_lowercase()) {
            return;
        }
    }

    // Focus mode: skip non-focused ordered tests unless include_ignored is set.
    if ctx.focus_mode && !force_focused && !ctx.config.include_ignored {
        result.skipped += 1;
        return;
    }

    // Fail-on-focus CI check for ordered tests inside focused containers.
    if force_focused && ctx.focus_mode {
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
                panic!("{} of {} ordered steps failed", failures.len(), steps.len());
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

    report_outcome(
        &indent,
        name,
        &full_path,
        outcome,
        start,
        &mut ctx.sink,
        result,
    );
}

/// Mark all descendant It nodes as pending (for xdescribe).
fn run_nodes_pending(nodes: &[TestNode], depth: usize, sink: &mut Sink, result: &mut RunResult) {
    let indent = "  ".repeat(depth);
    for node in nodes {
        match node {
            TestNode::Describe { name, children, .. } => {
                sink.line(&format!("{indent}{}", bold(&dim(name))));
                run_nodes_pending(children, depth + 1, sink, result);
            }
            TestNode::It { name, .. } => {
                sink.line(&format!("{indent}{} {}", yellow("-"), dim(name)));
                result.pending += 1;
            }
            TestNode::Ordered { name, .. } => {
                sink.line(&format!("{indent}{} {}", yellow("-"), dim(name)));
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
    sink: &mut Sink,
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
            sink.line(&format!("{indent}{} {}{}", green("✓"), name, time_str));
            result.passed += 1;
        }
        Err(e) => {
            let msg = panic_message(&*e);
            sink.line(&format!("{indent}{} {}{}", red("✗"), red(name), time_str));
            sink.line(&format!("{indent}  {}", red(&format!("Error: {msg}"))));
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
fn run_with_timeout(ms: u64, f: &dyn Fn()) -> Result<(), Box<dyn std::any::Any + Send>> {
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
            Err(Box::new(format!(
                "test timed out after {ms}ms (original error: {msg})"
            )))
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
            parallelism: 1,
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
            parallelism: 1,
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
            parallelism: 1,
        };
        let result = run_tree(&nodes, &config);

        assert_eq!(result.failed, 1, "before_all failure counted");
        assert_eq!(result.passed, 0, "child should not have run");
        assert!(
            AFTER_ALL_RAN.load(Ordering::SeqCst),
            "after_all must still run"
        );
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
            parallelism: 1,
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
            parallelism: 1,
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
            parallelism: 1,
        };
        let result = run_tree(&nodes, &config);

        assert_eq!(result.failed, 1, "before_each failure reported");
        assert!(
            AFTER_EACH_RAN.load(Ordering::SeqCst),
            "after_each must still run"
        );
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
            parallelism: 1,
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
            parallelism: 1,
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
            parallelism: 1,
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
            parallelism: 1,
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

    // Feature-off guarantee: the default build must still accept `!Send` test
    // bodies (the framework deliberately lets tests capture `Rc`/`RefCell`).
    // Compiled out under `parallel`, where the `Send` bound is intentional.
    #[cfg(not(feature = "parallel"))]
    #[test]
    fn non_send_test_body_is_accepted_without_parallel_feature() {
        use std::rc::Rc;
        let shared = Rc::new(7);
        let nodes = vec![TestNode::it("uses Rc", move || {
            assert_eq!(*shared, 7);
        })];
        let config = RunConfig {
            filter: None,
            list: false,
            include_ignored: false,
            parallelism: 1,
        };
        let result = run_tree(&nodes, &config);
        assert_eq!(result.passed, 1);
    }

    // ---- parallelism spec parsing (feature-independent) ----

    #[test]
    fn parse_parallel_spec_accepts_integers_and_auto() {
        assert_eq!(parse_parallel_spec("4"), Some(4));
        assert_eq!(parse_parallel_spec("1"), Some(1));
        // zero clamps up to 1 (never a no-op run)
        assert_eq!(parse_parallel_spec("0"), Some(1));
        assert!(parse_parallel_spec("auto").is_some_and(|n| n >= 1));
        assert_eq!(parse_parallel_spec("nonsense"), None);
    }

    // ---- parallel execution (requires the `parallel` feature) ----
    //
    // These assert the parallel runner's contract: identical results to the
    // sequential runner, deterministic tree-ordered output, per-subtree fixture
    // isolation, that distinct subtrees actually run concurrently, and that
    // `before_all` still runs exactly once per subtree.
    #[cfg(feature = "parallel")]
    mod parallel {
        use super::*;
        use std::sync::atomic::AtomicUsize;
        use std::time::{Duration, Instant};

        fn cfg(parallelism: usize) -> RunConfig {
            RunConfig {
                filter: None,
                list: false,
                include_ignored: false,
                parallelism,
            }
        }

        fn it_pass(name: &'static str) -> TestNode {
            TestNode::it(name, || {})
        }

        fn it_fail(name: &'static str, msg: &'static str) -> TestNode {
            TestNode::it(name, move || panic!("{msg}"))
        }

        fn it_pending(name: &str) -> TestNode {
            TestNode::It {
                name: name.to_string(),
                focused: false,
                pending: true,
                labels: Vec::new(),
                retries: None,
                timeout_ms: None,
                must_pass_repeatedly: None,
                test_fn: Box::new(|| {}),
            }
        }

        // A fixed tree: 3 passing, 1 failing, 1 pending across two subtrees.
        // Expected counts are derived from this fixture, not from the runner.
        fn sample_tree() -> Vec<TestNode> {
            vec![
                TestNode::describe(
                    "Alpha",
                    vec![it_pass("a1"), it_fail("a2", "alpha boom"), it_pending("a3")],
                ),
                TestNode::describe("Beta", vec![it_pass("b1"), it_pass("b2")]),
            ]
        }

        #[test]
        fn parallel_counts_match_sequential() {
            let seq = run_tree(&sample_tree(), &cfg(1));
            let par = run_suites(vec![Suite::new("", sample_tree())], &cfg(4));

            assert_eq!(par.passed, seq.passed);
            assert_eq!(par.failed, seq.failed);
            assert_eq!(par.pending, seq.pending);
            assert_eq!(par.skipped, seq.skipped);

            let mut seq_failures = seq.failures.clone();
            seq_failures.sort();
            let mut par_failures = par.failures.clone();
            par_failures.sort();
            assert_eq!(par_failures, seq_failures);

            // Anchored to the fixture so a runner that silently drops tests fails.
            assert_eq!(seq.passed, 3);
            assert_eq!(seq.failed, 1);
            assert_eq!(seq.pending, 1);
        }

        // Alpha (first in tree) finishes last; Beta finishes first. Output must
        // still list Alpha before Beta — ordering follows the tree, not
        // completion order — and must be byte-identical across runs.
        fn ordered_tree() -> Vec<TestNode> {
            vec![
                TestNode::describe(
                    "Alpha",
                    vec![TestNode::it("slow", || {
                        std::thread::sleep(Duration::from_millis(30));
                    })],
                ),
                TestNode::describe("Beta", vec![it_pass("fast")]),
            ]
        }

        #[test]
        fn parallel_output_is_ordered_and_deterministic() {
            let (out1, _) =
                render_suites_parallel(vec![Suite::new("", ordered_tree())], false, &cfg(2));
            let (out2, _) =
                render_suites_parallel(vec![Suite::new("", ordered_tree())], false, &cfg(2));

            assert_eq!(out1, out2, "parallel output must be deterministic");

            let alpha = out1.find("Alpha").expect("Alpha rendered");
            let beta = out1.find("Beta").expect("Beta rendered");
            assert!(
                alpha < beta,
                "Alpha must render before Beta regardless of completion order"
            );
        }

        #[test]
        fn parallel_isolates_fixtures_per_subtree() {
            let tree = vec![
                TestNode::describe_with_hooks(
                    "Scope10",
                    vec![Box::new(|| crate::store_scope_setup_value(10i32))],
                    vec![],
                    vec![TestNode::it("sees 10", || {
                        crate::with_setup_value::<i32, _>(|v| assert_eq!(*v, 10));
                    })],
                ),
                TestNode::describe_with_hooks(
                    "Scope20",
                    vec![Box::new(|| crate::store_scope_setup_value(20i32))],
                    vec![],
                    vec![TestNode::it("sees 20", || {
                        crate::with_setup_value::<i32, _>(|v| assert_eq!(*v, 20));
                    })],
                ),
            ];

            let result = run_suites(vec![Suite::new("", tree)], &cfg(2));
            assert_eq!(
                result.failed, 0,
                "fixtures leaked across subtrees: {:?}",
                result.failures
            );
            assert_eq!(result.passed, 2);
        }

        #[test]
        fn parallel_runs_subtrees_concurrently() {
            static ARRIVED: AtomicUsize = AtomicUsize::new(0);
            static MAX_SEEN: AtomicUsize = AtomicUsize::new(0);
            ARRIVED.store(0, Ordering::SeqCst);
            MAX_SEEN.store(0, Ordering::SeqCst);

            // Each subtree signals arrival, then waits (bounded) for the other.
            // Under serial execution the first body waits out the deadline,
            // never sees a second arrival, and fails the assertion — so a
            // regression to sequential execution fails loudly rather than hangs.
            fn rendezvous() {
                let n = ARRIVED.fetch_add(1, Ordering::SeqCst) + 1;
                MAX_SEEN.fetch_max(n, Ordering::SeqCst);
                // 5s deadline: generous enough to avoid flaking on a single-core
                // CI runner where the two scope threads time-slice; the wait is
                // only ever paid in full on a genuine serial-execution regression.
                let deadline = Instant::now() + Duration::from_secs(5);
                while ARRIVED.load(Ordering::SeqCst) < 2 {
                    if Instant::now() >= deadline {
                        break;
                    }
                    std::thread::yield_now();
                }
                assert!(
                    ARRIVED.load(Ordering::SeqCst) >= 2,
                    "subtrees did not run concurrently"
                );
            }

            let tree = vec![
                TestNode::describe("Left", vec![TestNode::it("waits", rendezvous)]),
                TestNode::describe("Right", vec![TestNode::it("waits", rendezvous)]),
            ];

            let result = run_suites(vec![Suite::new("", tree)], &cfg(2));
            assert_eq!(result.failed, 0, "both subtrees should pass concurrently");
            assert_eq!(result.passed, 2);
            assert!(
                MAX_SEEN.load(Ordering::SeqCst) >= 2,
                "never observed two concurrent subtrees"
            );
        }

        #[test]
        fn parallel_runs_before_all_once_per_subtree() {
            static CALLS: AtomicUsize = AtomicUsize::new(0);
            CALLS.store(0, Ordering::SeqCst);

            let tree = vec![TestNode::describe_with_hooks(
                "OnceScope",
                vec![Box::new(|| {
                    CALLS.fetch_add(1, Ordering::SeqCst);
                })],
                vec![],
                vec![it_pass("t1"), it_pass("t2"), it_pass("t3")],
            )];

            let result = run_suites(vec![Suite::new("", tree)], &cfg(4));
            assert_eq!(result.passed, 3);
            assert_eq!(
                CALLS.load(Ordering::SeqCst),
                1,
                "before_all must run exactly once for the subtree"
            );
        }

        #[test]
        fn parallelism_one_matches_sequential() {
            let seq = run_tree(&sample_tree(), &cfg(1));
            let one = run_suites(vec![Suite::new("", sample_tree())], &cfg(1));
            assert_eq!(one.passed, seq.passed);
            assert_eq!(one.failed, seq.failed);
            assert_eq!(one.pending, seq.pending);
            assert_eq!(one.skipped, seq.skipped);
        }
    }
}
