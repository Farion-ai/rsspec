// Plain `assert!(true)` bodies are used to demonstrate hook wiring where the assertion
// itself is not the point of the test.
#![allow(clippy::assertions_on_constants)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

fn main() {
    rsspec::run(|ctx| {
        // =================================================================
        // before_each — returning values to it blocks
        // =================================================================
        ctx.describe("before_each", |ctx| {
            ctx.context("returning a simple value", |ctx| {
                ctx.before_each(|| -> String { format!("hello-{}", 42) });

                ctx.it(
                    "passes the return value to it via &T",
                    |greeting: &String| {
                        assert_eq!(greeting, "hello-42");
                    },
                );
            });

            ctx.context("returning a struct", |ctx| {
                struct Response {
                    status: u16,
                    body: String,
                }

                ctx.before_each(|| -> Response {
                    Response {
                        status: 200,
                        body: "OK".to_string(),
                    }
                });

                ctx.it(
                    "works with any type, no Clone required",
                    |resp: &Response| {
                        assert_eq!(resp.status, 200);
                        assert_eq!(resp.body, "OK");
                    },
                );
            });

            ctx.context("with no return value", |ctx| {
                ctx.before_each(|| {
                    // side-effect only — backward compatible
                });

                ctx.it("still works with plain Fn() closures", || {
                    assert!(true);
                });
            });

            ctx.context("test isolation", |ctx| {
                ctx.before_each(|| -> Vec<i32> { vec![1, 2, 3] });

                ctx.it("provides a fresh value to each test", |v: &Vec<i32>| {
                    assert_eq!(v, &[1, 2, 3]);
                });

                ctx.it("is not affected by previous tests", |v: &Vec<i32>| {
                    assert_eq!(v, &[1, 2, 3]);
                });
            });

            ctx.context("nested contexts", |ctx| {
                ctx.before_each(|| -> String { "outer".to_string() });

                ctx.context("inner scope with its own before_each", |ctx| {
                    ctx.before_each(|| -> String { "inner".to_string() });

                    ctx.it(
                        "receives the inner value for the same type",
                        |val: &String| {
                            assert_eq!(val, "inner");
                        },
                    );
                });
            });
        });

        // =================================================================
        // before_each — acting (doing real work, not just returning data)
        // =================================================================
        ctx.describe("before_each as action", |ctx| {
            ctx.context("parsing and transforming data", |ctx| {
                ctx.before_each(|| -> HashMap<String, i32> {
                    let raw = "alice=10,bob=20,carol=30";
                    raw.split(',')
                        .filter_map(|pair| {
                            let mut parts = pair.split('=');
                            let key = parts.next()?.to_string();
                            let val = parts.next()?.parse().ok()?;
                            Some((key, val))
                        })
                        .collect()
                });

                ctx.it("parses all entries", |scores: &HashMap<String, i32>| {
                    assert_eq!(scores.len(), 3);
                });

                ctx.it(
                    "extracts correct values",
                    |scores: &HashMap<String, i32>| {
                        assert_eq!(scores["alice"], 10);
                        assert_eq!(scores["bob"], 20);
                        assert_eq!(scores["carol"], 30);
                    },
                );
            });

            ctx.context("building a complex object graph", |ctx| {
                #[allow(dead_code)]
                struct Tree {
                    label: String,
                    children: Vec<Tree>,
                }

                impl Tree {
                    fn depth(&self) -> usize {
                        1 + self.children.iter().map(|c| c.depth()).max().unwrap_or(0)
                    }

                    fn count(&self) -> usize {
                        1 + self.children.iter().map(|c| c.count()).sum::<usize>()
                    }
                }

                ctx.before_each(|| -> Tree {
                    Tree {
                        label: "root".into(),
                        children: vec![
                            Tree {
                                label: "child-1".into(),
                                children: vec![
                                    Tree {
                                        label: "grandchild-1a".into(),
                                        children: vec![],
                                    },
                                    Tree {
                                        label: "grandchild-1b".into(),
                                        children: vec![],
                                    },
                                ],
                            },
                            Tree {
                                label: "child-2".into(),
                                children: vec![],
                            },
                        ],
                    }
                });

                ctx.it("builds the correct depth", |tree: &Tree| {
                    assert_eq!(tree.depth(), 3);
                });

                ctx.it("builds the correct node count", |tree: &Tree| {
                    assert_eq!(tree.count(), 5);
                });
            });

            ctx.context("filtering and aggregating a dataset", |ctx| {
                struct Summary {
                    total: i32,
                    count: usize,
                    above_threshold: Vec<i32>,
                }

                ctx.before_each(|| -> Summary {
                    let data = [5i32, 12, 3, 18, 7, 25, 1, 14];
                    let threshold = 10;
                    let above: Vec<i32> = data.iter().copied().filter(|&v| v > threshold).collect();
                    Summary {
                        total: data.iter().sum(),
                        count: data.len(),
                        above_threshold: above,
                    }
                });

                ctx.it("computes the correct total", |s: &Summary| {
                    assert_eq!(s.total, 85);
                });

                ctx.it("counts all elements", |s: &Summary| {
                    assert_eq!(s.count, 8);
                });

                ctx.it("filters values above threshold", |s: &Summary| {
                    assert_eq!(s.above_threshold, vec![12, 18, 25, 14]);
                });
            });

            ctx.context("multi-step pipeline", |ctx| {
                struct Pipeline {
                    input: String,
                    tokens: Vec<String>,
                    frequencies: HashMap<String, usize>,
                }

                ctx.before_each(|| -> Pipeline {
                    let input = "the quick brown fox jumps over the lazy fox".to_string();
                    let tokens: Vec<String> = input.split_whitespace().map(String::from).collect();
                    let mut frequencies = HashMap::new();
                    for token in &tokens {
                        *frequencies.entry(token.clone()).or_insert(0) += 1;
                    }
                    Pipeline {
                        input,
                        tokens,
                        frequencies,
                    }
                });

                ctx.it("tokenizes the input", |p: &Pipeline| {
                    assert_eq!(p.tokens.len(), 9);
                });

                ctx.it("counts word frequencies", |p: &Pipeline| {
                    assert_eq!(p.frequencies["the"], 2);
                    assert_eq!(p.frequencies["fox"], 2);
                    assert_eq!(p.frequencies["quick"], 1);
                });

                ctx.it("preserves the original input", |p: &Pipeline| {
                    assert!(p.input.starts_with("the quick"));
                });
            });
        });

        // =================================================================
        // after_each
        // =================================================================
        ctx.describe("after_each", |ctx| {
            // Static needed: verifies after_each ran across tests
            static AE_RAN: AtomicU32 = AtomicU32::new(0);

            ctx.after_each(|| {
                AE_RAN.fetch_add(1, Ordering::SeqCst);
            });

            ctx.it("runs after normal completion", || {
                assert!(true);
            });

            ctx.it("confirms it ran for the previous test", || {
                assert!(AE_RAN.load(Ordering::SeqCst) >= 1);
            });
        });

        // =================================================================
        // before_all / after_all — per-scope hooks
        // =================================================================
        ctx.describe("before_all and after_all", |ctx| {
            ctx.context("returning a value", |ctx| {
                ctx.before_all(|| -> String { "shared-config".to_string() });

                ctx.it("is available in the first test", |config: &String| {
                    assert_eq!(config, "shared-config");
                });

                ctx.it("persists across subsequent tests", |config: &String| {
                    assert_eq!(config, "shared-config");
                });
            });

            ctx.context("runs exactly once", |ctx| {
                // Static needed: counts invocations across tests
                static BA_COUNTER: AtomicU32 = AtomicU32::new(0);

                ctx.before_all(|| {
                    BA_COUNTER.fetch_add(1, Ordering::SeqCst);
                });

                ctx.it("has run once after the first test", || {
                    assert_eq!(BA_COUNTER.load(Ordering::SeqCst), 1);
                });

                ctx.it("still only once after the second test", || {
                    assert_eq!(BA_COUNTER.load(Ordering::SeqCst), 1);
                });
            });

            ctx.context("expensive one-time setup", |ctx| {
                struct Config {
                    db_url: String,
                    max_connections: u32,
                    features: Vec<String>,
                }

                ctx.before_all(|| -> Config {
                    Config {
                        db_url: "postgres://localhost:5432/test".to_string(),
                        max_connections: 10,
                        features: vec!["auth".into(), "logging".into(), "metrics".into()],
                    }
                });

                ctx.it("provides config to the first test", |config: &Config| {
                    assert!(config.db_url.contains("postgres"));
                    assert_eq!(config.max_connections, 10);
                });

                ctx.it(
                    "provides the same config to subsequent tests",
                    |config: &Config| {
                        assert_eq!(config.features.len(), 3);
                        assert!(config.features.contains(&"auth".to_string()));
                    },
                );
            });
        });

        // =================================================================
        // before_each — same-type overwrite (last-registered-wins)
        // =================================================================
        ctx.describe("before_each same-type overwrite", |ctx| {
            ctx.context("two hooks returning the same type", |ctx| {
                ctx.before_each(|| -> String { "first".to_string() });
                ctx.before_each(|| -> String { "second".to_string() });

                ctx.it("receives the last-registered value", |val: &String| {
                    // The second before_each overwrites the first in the type map.
                    // This is the documented last-registered-wins behavior.
                    assert_eq!(val, "second");
                });
            });
        });

        // =================================================================
        // before_each vs before_all — priority when same type
        // =================================================================
        ctx.describe("before_each takes priority over before_all", |ctx| {
            ctx.context("both hooks return the same type", |ctx| {
                ctx.before_all(|| -> u32 { 1 });
                ctx.before_each(|| -> u32 { 2 });

                ctx.it("receives the before_each value", |val: &u32| {
                    // before_each stores into the per-test store, which is checked
                    // before the per-scope store (before_all). So before_each wins.
                    assert_eq!(*val, 2);
                });
            });
        });

        // =================================================================
        // before_all — nested scope isolation
        // =================================================================
        ctx.describe("before_all nested scope isolation", |ctx| {
            ctx.context(
                "outer and inner both register before_all for the same type",
                |ctx| {
                    ctx.before_all(|| -> String { "outer".to_string() });

                    ctx.context("inner scope", |ctx| {
                        ctx.before_all(|| -> String { "inner".to_string() });

                        ctx.it(
                            "receives the inner value inside the inner scope",
                            |val: &String| {
                                assert_eq!(val, "inner");
                            },
                        );
                    });

                    // This test runs after the inner scope has closed.
                    // Regression: a flat-map implementation would have cleared the outer
                    // value when the inner scope ended. The scope stack must restore it.
                    ctx.it(
                        "receives the outer value after inner scope closes",
                        |val: &String| {
                            assert_eq!(val, "outer");
                        },
                    );
                },
            );
        });

        // =================================================================
        // just_before_each — runs after before_each, before the test
        // =================================================================
        ctx.describe("just_before_each", |ctx| {
            ctx.before_each(|| -> u32 { 1 });

            ctx.just_before_each(|| {
                // Runs after before_each — the return value is already stored
            });

            ctx.it(
                "runs after before_each but before the test body",
                |val: &u32| {
                    assert_eq!(*val, 1);
                },
            );
        });
    });
}
