//! Behavioural tests for `highbeam:match` — the fuzzy matcher is Rust but
//! only callable via JS, so we exercise it via rquickjs.

use rquickjs::loader::{Loader, Resolver};
use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, Ctx, Error as JsError, Module, async_with};

use high_beam::sdk::r#match::MatchModule;

struct OnlyMatch;

impl Resolver for OnlyMatch {
    fn resolve(&mut self, _ctx: &Ctx<'_>, _base: &str, name: &str) -> Result<String, JsError> {
        if name == "highbeam:match" || name == "test:harness" {
            Ok(name.to_owned())
        } else {
            Err(JsError::new_resolving("<test>", "unexpected import"))
        }
    }
}

impl Loader for OnlyMatch {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str) -> Result<Module<'js>, JsError> {
        Module::declare_def::<MatchModule, _>(ctx.clone(), name)
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt")
}

fn run_harness(script: &str) -> serde_json::Value {
    let rt = rt();
    rt.block_on(async {
        let async_rt = AsyncRuntime::new().expect("rt");
        async_rt.set_loader(OnlyMatch, OnlyMatch).await;
        let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
        let json_str: String = async_with!(ctx => |ctx| {
            let declared = Module::declare(
                ctx.clone(),
                "test:harness",
                script.as_bytes().to_vec(),
            )
            .catch(&ctx)
            .expect("declare");
            let (_module, eval) = declared.eval().catch(&ctx).expect("eval");
            eval.into_future::<()>().await.catch(&ctx).expect("await eval");
            ctx.globals().get::<_, String>("__out").expect("read __out")
        })
        .await;
        serde_json::from_str(&json_str).expect("valid JSON")
    })
}

#[test]
fn fuzzy_returns_matches_sorted_by_score() {
    let out = run_harness(
        r"
        import { fuzzy } from 'highbeam:match';
        const items = ['Calculator', 'Calendar', 'Mail', 'Maps', 'Music', 'Notes'];
        const r = fuzzy(items, 'cal', { key: it => it });
        globalThis.__out = JSON.stringify(r);
        ",
    );
    let arr = out.as_array().expect("array");
    assert!(!arr.is_empty(), "got nothing for 'cal'");
    let titles: Vec<String> = arr.iter().map(|m| m["item"].as_str().unwrap().to_owned()).collect();
    assert!(titles.contains(&"Calculator".to_owned()));
    assert!(titles.contains(&"Calendar".to_owned()));
}

#[test]
fn fuzzy_respects_limit_option() {
    let out = run_harness(
        r"
        import { fuzzy } from 'highbeam:match';
        const r = fuzzy(['aa', 'ab', 'ac', 'ad', 'ae'], 'a', { key: it => it, limit: 2 });
        globalThis.__out = JSON.stringify(r);
        ",
    );
    let arr = out.as_array().expect("array");
    assert_eq!(arr.len(), 2);
}

#[test]
fn fuzzy_respects_threshold_option() {
    let out = run_harness(
        r"
        import { fuzzy } from 'highbeam:match';
        const r = fuzzy(['Calculator', 'xyzzy'], 'cal', { key: it => it, threshold: 0.99 });
        globalThis.__out = JSON.stringify(r);
        ",
    );
    let arr = out.as_array().expect("array");
    assert!(arr.is_empty(), "threshold should have filtered everything, got {arr:?}");
}

#[test]
fn fuzzy_produces_highlights_for_matched_chars() {
    let out = run_harness(
        r"
        import { fuzzy } from 'highbeam:match';
        const r = fuzzy(['Safari'], 'sf', { key: it => it });
        globalThis.__out = JSON.stringify(r);
        ",
    );
    let arr = out.as_array().expect("array");
    assert!(!arr.is_empty(), "Safari should match 'sf'");
    let highlights = arr[0]["highlights"].as_array().expect("highlights array");
    assert!(!highlights.is_empty(), "highlights should reference the matched chars");
}

#[test]
fn fuzzy_returns_empty_on_no_match() {
    let out = run_harness(
        r"
        import { fuzzy } from 'highbeam:match';
        const r = fuzzy(['apple', 'banana'], 'xyzzy', { key: it => it });
        globalThis.__out = JSON.stringify(r);
        ",
    );
    let arr = out.as_array().expect("array");
    assert!(arr.is_empty());
}

#[test]
fn fuzzy_is_deterministic_across_runs() {
    let script = r"
        import { fuzzy } from 'highbeam:match';
        const items = ['Slack', 'Safari', 'System Preferences', 'Spotify', 'Sketch'];
        const r = fuzzy(items, 'saf', { key: it => it });
        globalThis.__out = JSON.stringify(r.map(m => m.item));
    ";
    let a = run_harness(script);
    let b = run_harness(script);
    assert_eq!(a, b);
}
