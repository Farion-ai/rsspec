//! In-situ smoke test: a realistic "arrange & act once, assert often" seam in
//! macro form, with the fixture read IMPLICITLY.
//!
//! The pattern this library exists for: where today each behaviour would be a
//! separate `#[test]` that re-runs an expensive `analyze()` — so the work runs
//! once *per assertion* — folding the behaviours into one `describe!` with
//! `before_all!` runs the analysis once for the whole group. Every `it!` then
//! reads the shared `report` by its bare name (no `|report: &Report|`) and
//! asserts one property. The `ANALYZE_CALLS == 1` assertion is the act-once proof.

use rsspec::describe;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};

/// The analysis result: a word-frequency report over a document.
struct Report {
    total_words: usize,
    counts: HashMap<String, usize>,
}

impl Report {
    fn unique_words(&self) -> usize {
        self.counts.len()
    }
    fn count(&self, word: &str) -> usize {
        self.counts.get(word).copied().unwrap_or(0)
    }
}

const DOCUMENT: &str = "the quick brown fox the lazy dog the end";

static ANALYZE_CALLS: AtomicU32 = AtomicU32::new(0);

/// The expensive "act": tokenize a document and fold it into a frequency report.
/// Counts its own invocations so the test can prove it runs once per group.
fn analyze(text: &str) -> Report {
    ANALYZE_CALLS.fetch_add(1, SeqCst);
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut total_words = 0;
    for word in text.split_whitespace() {
        *counts.entry(word.to_string()).or_insert(0) += 1;
        total_words += 1;
    }
    Report {
        total_words,
        counts,
    }
}

#[test]
fn document_analysis_seam_in_macro_form_acts_once() {
    ANALYZE_CALLS.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("word frequency for the sample document", {
            // arrange & act ONCE for the whole group
            before_all!(report: Report = analyze(DOCUMENT));

            it!("counts every whitespace-separated token", {
                assert_eq!(report.total_words, 9);
            });

            it!("collapses repeats into unique words", {
                assert_eq!(report.unique_words(), 7);
            });

            it!("attributes the most frequent word", {
                assert_eq!(report.count("the"), 3);
            });
        });
    });

    // The whole point: three assertions, one analysis — not one analysis each.
    assert_eq!(
        ANALYZE_CALLS.load(SeqCst),
        1,
        "before_all! runs the act exactly once for all specs in the describe"
    );
}
