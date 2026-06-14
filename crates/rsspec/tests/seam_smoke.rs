//! In-situ smoke test: a realistic "arrange & act once, assert often" seam
//! rewritten with the macro layer.
//!
//! Modeled on a real route-analyzer seam test (`library_detection.rs`), where
//! today each behaviour is a separate `#[test]` that calls
//! `analyze_fixture() -> RouteResult` — so the expensive analysis runs once
//! *per assertion*. Folded into one `describe!` with `before_all!`, the analysis
//! runs once for the whole group and every `it!` reads the shared result. The
//! `ANALYZE_CALLS == 1` assertion is the act-once proof.

use rsspec::{before_all, describe, it};
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};

// ---- stand-ins mirroring the route-analyzer's `domain::RouteResult` shape ----

struct RouteResult {
    backend_routes: Vec<Route>,
}
struct Route {
    external_libs: Vec<Lib>,
    property_graph: Option<PropertyGraph>,
}
struct Lib {
    name: String,
    purl: Option<String>,
}
struct PropertyGraph {
    nodes: Vec<Node>,
}
struct Node {
    kind: NodeKind,
    fqname: Option<String>,
}
#[derive(PartialEq)]
enum NodeKind {
    Library,
    Other,
}

static ANALYZE_CALLS: AtomicU32 = AtomicU32::new(0);

/// The expensive "act": copies a fixture and runs `analyze()`. Here it just
/// builds a representative result and counts how many times it is invoked.
fn analyze_fixture() -> RouteResult {
    ANALYZE_CALLS.fetch_add(1, SeqCst);
    RouteResult {
        backend_routes: vec![Route {
            external_libs: vec![Lib {
                name: "rusqlite".to_string(),
                purl: Some("pkg:cargo/rusqlite@0.31.0".to_string()),
            }],
            property_graph: Some(PropertyGraph {
                nodes: vec![
                    Node {
                        kind: NodeKind::Other,
                        fqname: None,
                    },
                    Node {
                        kind: NodeKind::Library,
                        fqname: Some("rusqlite::Connection#open".to_string()),
                    },
                ],
            }),
        }],
    }
}

#[test]
fn library_detection_seam_in_macro_form_acts_once() {
    ANALYZE_CALLS.store(0, SeqCst);

    rsspec::run_inline(|_| {
        describe!("library detection for the axum fixture", {
            // arrange & act ONCE for the whole group
            before_all!(result: RouteResult = analyze_fixture());

            it!(
                "attributes rusqlite to its resolved crate",
                |result: &RouteResult| {
                    let route = result
                        .backend_routes
                        .first()
                        .expect("axum fixture defines a route");
                    let names: Vec<&str> = route
                        .external_libs
                        .iter()
                        .map(|l| l.name.as_str())
                        .collect();
                    assert!(
                        names.contains(&"rusqlite"),
                        "rusqlite must be attributed; got {names:?}"
                    );
                }
            );

            it!(
                "combines package name and version into a purl",
                |result: &RouteResult| {
                    let route = result.backend_routes.first().unwrap();
                    let lib = route
                        .external_libs
                        .iter()
                        .find(|l| l.name == "rusqlite")
                        .unwrap();
                    assert_eq!(lib.purl.as_deref(), Some("pkg:cargo/rusqlite@0.31.0"));
                }
            );

            it!(
                "emits a library node for the called method",
                |result: &RouteResult| {
                    let graph = result.backend_routes[0]
                        .property_graph
                        .as_ref()
                        .expect("route carries a property graph");
                    let has_open = graph
                        .nodes
                        .iter()
                        .filter(|n| n.kind == NodeKind::Library)
                        .any(|n| n.fqname.as_deref() == Some("rusqlite::Connection#open"));
                    assert!(
                        has_open,
                        "library node for rusqlite::Connection#open must exist"
                    );
                }
            );
        });
    });

    // The whole point: three assertions, one analysis — not one analysis each.
    assert_eq!(
        ANALYZE_CALLS.load(SeqCst),
        1,
        "before_all! runs the act exactly once for all specs in the describe"
    );
}
