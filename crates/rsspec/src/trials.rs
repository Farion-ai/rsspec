//! Isolation-root enumeration for the libtest/nextest protocol.
//!
//! `cargo nextest` runs one process per test and enumerates tests over the
//! libtest protocol (`--list --format terse`, then `<name> --exact`). rsspec is
//! a runtime-built tree, so we map it onto that protocol by computing one
//! [`Trial`] per **isolation root** — the shallowest scope on the path to a spec
//! that owns shared `before_all`/`after_all` state.
//!
//! - A scope with a shared `before_all`/`after_all` is one Trial for its whole
//!   subtree (the act runs once; its specs co-locate because they must).
//! - A scope with only per-spec hooks recurses, so each independent `it` / table
//!   case / ordered block becomes its own Trial — full per-test isolation.
//!
//! The granularity is derived from the fixture structure, never chosen by hand.

use crate::runner::TestNode;
use std::fmt::Write as _;

/// One libtest-protocol test: an isolation root in the rsspec tree. A scope with
/// a shared `before_all`/`after_all` yields one Trial for its whole subtree; an
/// independent leaf spec yields its own — the distinction is encoded in the
/// `locator` (which node it points at) and the name, not a separate tag.
pub(crate) struct Trial {
    /// `::`-joined path from the suite root to the isolation root. This is the
    /// exact string emitted by `--list` and matched by `<name> --exact`.
    pub name: String,
    /// Pending (`xit`/`xdescribe`) anywhere on the path — reported as `ignored`.
    pub ignored: bool,
    /// Child indices from the suite root to this trial's node, so the runner can
    /// descend to exactly this subtree without re-deriving the path.
    pub locator: Vec<usize>,
}

/// Walk the tree and emit one [`Trial`] per isolation root (see module docs).
pub(crate) fn enumerate_trials(nodes: &[TestNode]) -> Vec<Trial> {
    let mut out = Vec::new();
    let mut locator = Vec::new();
    let mut segments: Vec<&str> = Vec::new();
    walk(nodes, &mut locator, &mut segments, false, &mut out);
    out
}

/// Recursive descent. `ignored` carries a pending ancestor downward so an
/// `xdescribe`'s specs surface as ignored trials. `segments`/`locator` borrow no
/// owned state beyond node names; only the emitted name and the snapshot locator
/// allocate.
fn walk<'a>(
    nodes: &'a [TestNode],
    locator: &mut Vec<usize>,
    segments: &mut Vec<&'a str>,
    ignored: bool,
    out: &mut Vec<Trial>,
) {
    for (i, node) in nodes.iter().enumerate() {
        locator.push(i);
        match node {
            TestNode::Describe {
                name,
                pending,
                before_all,
                after_all,
                children,
                ..
            } => {
                segments.push(name);
                let group_ignored = ignored || *pending;
                // A shared act/teardown makes this the isolation root: the whole
                // subtree is one trial and we stop descending. Only per-spec
                // hooks (before_each/…) recurse, so independent specs split out.
                if before_all.is_empty() && after_all.is_empty() {
                    walk(children, locator, segments, group_ignored, out);
                } else {
                    out.push(Trial {
                        name: segments.join("::"),
                        ignored: group_ignored,
                        // snapshot: `locator` keeps mutating as the walk continues
                        locator: locator.clone(),
                    });
                }
                segments.pop();
            }
            TestNode::It { name, pending, .. } => {
                segments.push(name);
                out.push(Trial {
                    name: segments.join("::"),
                    ignored: ignored || *pending,
                    locator: locator.clone(),
                });
                segments.pop();
            }
            TestNode::Ordered { name, .. } => {
                segments.push(name);
                out.push(Trial {
                    name: segments.join("::"),
                    ignored,
                    locator: locator.clone(),
                });
                segments.pop();
            }
        }
        locator.pop();
    }
}

/// Render trials in libtest's `--list --format terse` wire format — the listing
/// interface `cargo nextest` requires of a custom harness. Takes references so a
/// filtered (non-contiguous) selection can be rendered without cloning trials.
pub(crate) fn format_list_terse(trials: &[&Trial]) -> String {
    let mut s = String::new();
    for t in trials {
        let _ = writeln!(s, "{}: test", t.name);
    }
    let _ = write!(s, "\n{} tests, 0 benchmarks", trials.len());
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::TestNode;

    // ---- tree builders (explicit, so each test's structure is visible) ------

    fn it_(name: &str) -> TestNode {
        TestNode::It {
            name: name.to_string(),
            focused: false,
            pending: false,
            labels: Vec::new(),
            retries: None,
            timeout_ms: None,
            must_pass_repeatedly: None,
            test_fn: Box::new(|| {}),
        }
    }

    fn xit_(name: &str) -> TestNode {
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

    fn describe(name: &str, pending: bool, before_all: bool, after_all: bool, before_each: bool, children: Vec<TestNode>) -> TestNode {
        let hook = || -> Vec<crate::TestBody> { vec![Box::new(|| {})] };
        TestNode::Describe {
            name: name.to_string(),
            focused: false,
            pending,
            labels: Vec::new(),
            before_each: if before_each { hook() } else { Vec::new() },
            after_each: Vec::new(),
            before_all: if before_all { hook() } else { Vec::new() },
            after_all: if after_all { hook() } else { Vec::new() },
            just_before_each: Vec::new(),
            children,
        }
    }

    /// Plain group — no hooks at all (recurses).
    fn d(name: &str, children: Vec<TestNode>) -> TestNode {
        describe(name, false, false, false, false, children)
    }
    /// Group with a shared `before_all` (an isolation root).
    fn d_ba(name: &str, children: Vec<TestNode>) -> TestNode {
        describe(name, false, true, false, false, children)
    }
    /// Group with only a shared `after_all` (also an isolation root).
    fn d_aa(name: &str, children: Vec<TestNode>) -> TestNode {
        describe(name, false, false, true, false, children)
    }
    /// Group with only `before_each` — per-spec, does NOT force co-location.
    fn d_be(name: &str, children: Vec<TestNode>) -> TestNode {
        describe(name, false, false, false, true, children)
    }
    /// Pending group (`xdescribe`).
    fn xd(name: &str, children: Vec<TestNode>) -> TestNode {
        describe(name, true, false, false, false, children)
    }

    fn names(trials: &[Trial]) -> Vec<String> {
        trials.iter().map(|t| t.name.clone()).collect()
    }

    // ---- Mode A: shared act → one process per subtree ----------------------

    // Spec: a top-level describe with a shared before_all is ONE Trial for the
    // whole subtree, even with many specs inside — the act runs once.
    #[test]
    fn mode_a_shared_before_all_is_a_single_subtree_trial() {
        let tree = vec![d_ba("Seam", vec![it_("a"), it_("b"), it_("c")])];
        let trials = enumerate_trials(&tree);

        assert_eq!(trials.len(), 1, "the whole subtree collapses to one trial");
        assert_eq!(trials[0].name, "Seam");
        assert!(!trials[0].ignored);
        assert_eq!(trials[0].locator, vec![0]);
    }

    // Spec: after_all alone is also shared teardown state → forces an isolation
    // root, exactly like before_all.
    #[test]
    fn after_all_alone_forces_a_subtree_trial() {
        let tree = vec![d_aa("teardown seam", vec![it_("x"), it_("y")])];
        let trials = enumerate_trials(&tree);

        assert_eq!(trials.len(), 1);
        assert_eq!(trials[0].name, "teardown seam");
    }

    // Spec: the isolation root is the SHALLOWEST scope with a shared hook. An
    // inner before_all under an outer before_all must NOT create a second trial.
    #[test]
    fn shallowest_scope_with_a_shared_hook_wins() {
        let tree = vec![d_ba(
            "outer",
            vec![d_ba("inner", vec![it_("deep")]), it_("mid")],
        )];
        let trials = enumerate_trials(&tree);

        assert_eq!(trials.len(), 1, "outer is the isolation root; inner does not split");
        assert_eq!(trials[0].name, "outer");
    }

    // ---- Mode B/C: independent specs → one process each --------------------

    // Spec: a group with only before_each (no shared act) recurses — each
    // independent spec becomes its own Trial with `::`-joined nesting.
    #[test]
    fn mode_b_independent_specs_each_become_a_leaf_trial() {
        let tree = vec![d_be(
            "Integration",
            vec![it_("scenario one"), it_("scenario two"), it_("scenario three")],
        )];
        let trials = enumerate_trials(&tree);

        assert_eq!(
            names(&trials),
            vec![
                "Integration::scenario one",
                "Integration::scenario two",
                "Integration::scenario three"
            ]
        );
    }

    // Spec: table-driven / pure specs (a plain group of `it`s) → each case is its
    // own Trial.
    #[test]
    fn mode_c_plain_group_specs_each_become_a_leaf_trial() {
        let tree = vec![d("arithmetic", vec![it_("addition"), it_("subtraction")])];
        let trials = enumerate_trials(&tree);

        assert_eq!(names(&trials), vec!["arithmetic::addition", "arithmetic::subtraction"]);
    }

    // A bare top-level `it` (no enclosing describe, no shared act) is its own
    // trial, named just the spec.
    #[test]
    fn top_level_it_without_a_describe_is_its_own_trial() {
        let tree = vec![it_("standalone")];
        let trials = enumerate_trials(&tree);

        assert_eq!(trials.len(), 1);
        assert_eq!(trials[0].name, "standalone");
        assert_eq!(trials[0].locator, vec![0]);
    }

    // ---- Recursion across mixed nesting ------------------------------------

    // A plain outer group recurses: an inner shared-act subtree becomes one
    // Subtree trial, while an independent sibling spec becomes its own Leaf
    // trial. Locators must point at the actual nodes for the runner to descend.
    #[test]
    fn plain_group_recurses_isolating_at_the_inner_shared_hook() {
        let tree = vec![d(
            "outer",
            vec![d_ba("inner", vec![it_("d1"), it_("d2")]), it_("sibling")],
        )];
        let trials = enumerate_trials(&tree);

        // inner (a shared-act subtree) collapses to one trial at [0, 0]; the
        // independent sibling spec is its own trial at [0, 1].
        assert_eq!(names(&trials), vec!["outer::inner", "outer::sibling"]);
        assert_eq!(trials[0].locator, vec![0, 0]);
        assert_eq!(trials[1].locator, vec![0, 1]);
    }

    // A whole binary mixing a Mode-A seam and a Mode-B integration group yields
    // the exact trial set CI will shard over.
    #[test]
    fn mixed_tree_yields_the_expected_trial_set() {
        let tree = vec![
            d_ba("Seam", vec![it_("a"), it_("b")]),
            d_be("Integration", vec![it_("s1"), it_("s2")]),
        ];
        let trials = enumerate_trials(&tree);

        assert_eq!(names(&trials), vec!["Seam", "Integration::s1", "Integration::s2"]);
    }

    // ---- Pending → ignored --------------------------------------------------

    // `xit` and specs under an `xdescribe` are listed but flagged ignored, so
    // nextest reports them skipped rather than dropping them silently.
    #[test]
    fn pending_specs_are_listed_as_ignored_trials() {
        let tree = vec![
            xit_("todo"),
            d("g", vec![xit_("later")]),
            xd("whole group", vec![it_("inside")]),
        ];
        let trials = enumerate_trials(&tree);

        assert_eq!(names(&trials), vec!["todo", "g::later", "whole group::inside"]);
        assert!(
            trials.iter().all(|t| t.ignored),
            "every pending spec must be flagged ignored"
        );
    }

    // ---- Wire format: the libtest `--list --format terse` contract ----------

    // This is the exact byte stream `cargo nextest` parses to discover tests:
    // one `<name>: test` line per trial, a blank line, then the count summary.
    #[test]
    fn terse_list_output_matches_the_libtest_protocol() {
        let tree = vec![d_ba("Seam", vec![it_("a")]), d_be("Int", vec![it_("s1")])];
        let trials = enumerate_trials(&tree);
        let refs: Vec<&Trial> = trials.iter().collect();

        let out = format_list_terse(&refs);

        assert_eq!(out, "Seam: test\nInt::s1: test\n\n2 tests, 0 benchmarks\n");
    }
}
