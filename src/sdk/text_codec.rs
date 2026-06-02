//! UTF-8-only `TextEncoder` / `TextDecoder` polyfill.
//!
//! Raw `QuickJS` ships neither, but `fs.readCache` hands plugins a
//! `Uint8Array` — without a decoder the bytes are a dead end. Conversion
//! runs host-side (Rust strings are UTF-8 already; invalid sequences
//! degrade to U+FFFD like a non-fatal spec decoder). The JS layer is a
//! thin class wrapper; constructing `TextDecoder` with any label other
//! than UTF-8 throws `RangeError`.

use rquickjs::{Ctx, Error as JsError, Function, TypedArray, Value};

use crate::sdk::errors::throw_named;

const TEXT_CODEC_JS: &str = include_str!("js/text_codec.js");

const ENCODE_GLOBAL: &str = "__highbeam_utf8_encode";
const DECODE_GLOBAL: &str = "__highbeam_utf8_decode";

/// Install `TextEncoder` / `TextDecoder` on the global object. Idempotent.
///
/// # Errors
///
/// Propagates JS errors from constructing the host functions or evaluating
/// the wrapper script.
pub fn install<'js>(ctx: &Ctx<'js>) -> Result<(), JsError> {
    let encode = Function::new(ctx.clone(), |ctx: Ctx<'js>, text: String| {
        TypedArray::new(ctx, text.into_bytes()).map(TypedArray::into_value)
    })?;
    ctx.globals().set(ENCODE_GLOBAL, encode)?;

    let decode = Function::new(ctx.clone(), |ctx: Ctx<'js>, value: Value<'js>| {
        let Ok(bytes) = TypedArray::<u8>::from_value(value) else {
            return Err(throw_named(
                &ctx,
                "TypeError",
                "TextDecoder.decode: expected a BufferSource",
            ));
        };
        let slice: &[u8] = bytes.as_ref();
        Ok(String::from_utf8_lossy(slice).into_owned())
    })?;
    ctx.globals().set(DECODE_GLOBAL, decode)?;

    ctx.eval::<(), _>(TEXT_CODEC_JS)
}
