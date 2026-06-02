//! Behavioural tests for the UTF-8 `TextEncoder` / `TextDecoder` polyfill.

use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, async_with};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt")
}

fn eval_str(script: &str) -> String {
    let rt = rt();
    rt.block_on(async {
        let async_rt = AsyncRuntime::new().expect("rt");
        let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
        async_with!(ctx => |ctx| {
            high_beam::sdk::text_codec::install(&ctx).catch(&ctx).expect("install");
            ctx.eval::<String, _>(script).catch(&ctx).expect("eval")
        })
        .await
    })
}

#[test]
fn encode_decode_round_trips_multibyte() {
    let out = eval_str(r#"new TextDecoder().decode(new TextEncoder().encode("héllo 🚀 ✓"))"#);
    assert_eq!(out, "héllo 🚀 ✓");
}

#[test]
fn decode_degrades_invalid_bytes_to_replacement_char() {
    // Non-fatal spec decoders map invalid sequences to U+FFFD; the host
    // side uses from_utf8_lossy which matches that.
    let out = eval_str("new TextDecoder().decode(new Uint8Array([0xff, 0x68, 0x69]))");
    assert_eq!(out, "\u{FFFD}hi");
}

#[test]
fn decode_accepts_arraybuffer_and_offset_views() {
    let out = eval_str(
        r#"
        const buf = new TextEncoder().encode("xxhi").buffer;
        const whole = new TextDecoder().decode(buf);
        const view = new TextDecoder().decode(new Uint8Array(buf, 2, 2));
        `${whole}|${view}`
        "#,
    );
    assert_eq!(out, "xxhi|hi");
}

#[test]
fn decode_of_undefined_is_empty_string() {
    let out = eval_str(r#"new TextDecoder().decode() === "" ? "ok" : "fail""#);
    assert_eq!(out, "ok");
}

#[test]
fn non_utf8_label_throws_range_error() {
    let out = eval_str(
        r#"
        let result = "no throw";
        try {
            new TextDecoder("latin1");
        } catch (e) {
            result = e instanceof RangeError ? "RangeError" : `wrong type: ${e}`;
        }
        result
        "#,
    );
    assert_eq!(out, "RangeError");
}

#[test]
fn decode_of_non_buffer_throws_type_error() {
    let out = eval_str(
        r#"
        let result = "no throw";
        try {
            new TextDecoder().decode("not bytes");
        } catch (e) {
            result = e.name;
        }
        result
        "#,
    );
    assert_eq!(out, "TypeError");
}

#[test]
fn encoding_property_reports_utf8() {
    let out = eval_str(r#"`${new TextEncoder().encoding}|${new TextDecoder("UTF-8").encoding}`"#);
    assert_eq!(out, "utf-8|utf-8");
}

#[test]
fn install_is_idempotent() {
    let rt = rt();
    let out = rt.block_on(async {
        let async_rt = AsyncRuntime::new().expect("rt");
        let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
        async_with!(ctx => |ctx| {
            high_beam::sdk::text_codec::install(&ctx).catch(&ctx).expect("first install");
            ctx.eval::<(), _>("globalThis.__probe = TextDecoder").catch(&ctx).expect("stash");
            high_beam::sdk::text_codec::install(&ctx).catch(&ctx).expect("second install");
            ctx.eval::<bool, _>("globalThis.__probe === TextDecoder").catch(&ctx).expect("compare")
        })
        .await
    });
    assert!(out, "second install must not replace the classes");
}
