mod common;

// Smoke-test for the echo plugin bundled in examples/plugins/echo/.
//
// Every subsequent plugin port mirrors this pattern: one file per plugin,
// three test cases — loads, expected shape, and edge-input behaviour.

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt")
}

#[test]
fn echo_plugin_loads() {
    rt().block_on(async {
        let tester = common::PluginTester::load("examples/plugins/echo")
            .await
            .expect("echo plugin should load without error");
        // A loaded tester is proof the manifest parsed and plugin.js evaluated.
        drop(tester);
    });
}

#[test]
fn echo_query_hello_matches_expected_shape() {
    rt().block_on(async {
        let tester = common::PluginTester::load("examples/plugins/echo")
            .await
            .expect("load echo");

        let results = tester.query("hello").await.expect("query should succeed");

        assert_eq!(
            results.len(),
            1,
            "echo yields exactly one result per non-empty input"
        );

        let r = &results[0];
        assert_eq!(r.key, "echo");
        assert_eq!(r.title, "echo: hello");
        assert_eq!(
            r.subtitle.as_deref(),
            Some("press Enter to copy to clipboard")
        );
        // `copy` is data — no live side effect, no stub needed.
        assert_eq!(
            r.as_copy(),
            Some("hello"),
            "action should be copy with the echoed input"
        );
        // Echo doesn't pin or weight its result — verify defaults hold.
        assert!(!r.pinned);
        assert!((r.weight - 0.0).abs() < f64::EPSILON);
        // Other action variants don't match a copy action.
        assert!(r.as_open_url().is_none());
        assert!(r.as_exec().is_none());
        assert!(r.as_reveal().is_none());
    });
}

#[test]
fn echo_query_empty_yields_nothing() {
    rt().block_on(async {
        let tester = common::PluginTester::load("examples/plugins/echo")
            .await
            .expect("load echo");

        let results = tester.query("").await.expect("empty query should succeed");

        assert!(
            results.is_empty(),
            "echo skips empty input — expected 0 results, got {}",
            results.len()
        );
    });
}
