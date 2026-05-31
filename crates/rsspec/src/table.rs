//! Table-driven tests — parameterized test cases via a builder.

use crate::context::with_builder;
use crate::runner::TestNode;
use std::sync::Arc;

/// Builder for table-driven (parameterized) tests.
///
/// Returned by [`Context::describe_table`](crate::Context::describe_table).
/// Call [`.case()`](Self::case) to add the first case, which fixes the data
/// type `T` and returns a [`TypedTableBuilder<T>`].
///
/// # Example
///
/// ```rust,no_run
/// # fn main() { rsspec::run(|ctx| {
/// ctx.describe_table("arithmetic")
///     .case("addition", (2i32, 3i32, 5i32))
///     .case("large numbers", (100, 200, 300))
///     .run(|(a, b, expected): &(i32, i32, i32)| {
///         assert_eq!(a + b, *expected);
///     });
/// # }); }
/// ```
pub struct TableBuilder {
    name: String,
}

impl TableBuilder {
    pub(crate) fn new(name: String) -> Self {
        TableBuilder { name }
    }

    /// Add the first named test case, fixing the data type for all subsequent cases.
    pub fn case<T: 'static>(self, label: &str, data: T) -> TypedTableBuilder<T> {
        TypedTableBuilder {
            name: self.name,
            cases: vec![(label.to_string(), data)],
            auto_index: 0,
        }
    }

    /// Add the first unnamed test case (auto-named `case_1`).
    pub fn case_unnamed<T: 'static>(self, data: T) -> TypedTableBuilder<T> {
        TypedTableBuilder {
            name: self.name,
            cases: vec![("case_1".to_string(), data)],
            auto_index: 1,
        }
    }
}

/// A table builder with a fixed data type `T`.
///
/// Created by [`TableBuilder::case`] or [`TableBuilder::case_unnamed`].
/// Add more cases with [`.case()`](Self::case), then call
/// [`.run()`](Self::run) to register the tests.
pub struct TypedTableBuilder<T> {
    name: String,
    cases: Vec<(String, T)>,
    auto_index: usize,
}

impl<T: 'static> TypedTableBuilder<T> {
    /// Add a named test case with parameter data.
    pub fn case(mut self, label: &str, data: T) -> Self {
        self.cases.push((label.to_string(), data));
        self
    }

    /// Add an unnamed test case (auto-named `case_1`, `case_2`, ...).
    pub fn case_unnamed(mut self, data: T) -> Self {
        self.auto_index += 1;
        let label = format!("case_{}", self.auto_index);
        self.cases.push((label, data));
        self
    }

    /// Run all cases. Each case becomes a separate test node.
    ///
    /// The test function receives a reference to the data for each case.
    pub fn run(self, test_fn: impl Fn(&T) + crate::MaybeSendSync + 'static)
    where
        T: crate::MaybeSend,
    {
        with_builder(|b| b.push_group(self.name, false, false));

        let test_fn = Arc::new(test_fn);

        for (label, data) in self.cases {
            // O(1) refcount bump — shares one test_fn across all cases.
            let test_fn = Arc::clone(&test_fn);

            // Data is owned by the closure and passed by reference to test_fn.
            // This makes the closure Fn() — callable multiple times (for retries).
            let body = move || {
                test_fn(&data);
            };

            with_builder(|b| {
                b.add_node(TestNode::It {
                    name: label,
                    focused: false,
                    pending: false,
                    labels: Vec::new(),
                    retries: None,
                    timeout_ms: None,
                    must_pass_repeatedly: None,
                    test_fn: Box::new(body),
                });
            });
        }

        with_builder(|b| b.pop_group());
    }

    /// Run all cases with an async test function.
    ///
    /// Each case becomes a separate test node. The test function receives a
    /// reference to the data; copy/clone values into the async block as needed.
    ///
    /// ```rust,ignore
    /// ctx.describe_table("async endpoints")
    ///     .case("add", (2i32, 3i32, 5i32))
    ///     .async_run(|data: &(i32, i32, i32)| {
    ///         let (a, b, expected) = *data;
    ///         async move { assert_eq!(a + b, expected); }
    ///     });
    /// ```
    ///
    /// Available with the `tokio` feature.
    #[cfg(feature = "tokio")]
    pub fn async_run<F, Fut>(self, test_fn: F)
    where
        F: Fn(&T) -> Fut + crate::MaybeSendSync + 'static,
        Fut: std::future::Future<Output = ()> + 'static,
        T: crate::MaybeSend,
    {
        self.run(move |arg: &T| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("rsspec: failed to build Tokio runtime");
            rt.block_on(test_fn(arg));
        });
    }
}
