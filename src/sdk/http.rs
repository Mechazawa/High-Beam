//! Host implementation of the `highbeam:http` module. Requires the `http`
//! capability.
//!
//! Cancellation: a plugin-supplied `signal` becomes a [`CancellationToken`]
//! via [`abort::token_from_js_signal`]; the send/body futures race that
//! token. UTF-8 only — binary responses are lossy-decoded for v1.

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::Client;
use rquickjs::function::Async;
use rquickjs::{Ctx, Function, Object, Result as JsResult, Value, module::ModuleDef};

use crate::sdk::abort;
use crate::sdk::errors::{throw_abort, throw_named};

/// Default per-request timeout when `opts.timeoutMs` is absent.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Shared client across the whole daemon — pools TCP/TLS sessions.
fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .user_agent("high-beam/0.1")
            .timeout(DEFAULT_TIMEOUT)
            // Default is on; spelled out for auditability.
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .unwrap_or_else(|_| Client::new())
    })
}

pub struct HttpModule;

impl ModuleDef for HttpModule {
    fn declare(decl: &rquickjs::module::Declarations<'_>) -> JsResult<()> {
        decl.declare("get")?;
        decl.declare("post")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &rquickjs::module::Exports<'js>) -> JsResult<()> {
        let get_fn = Function::new(
            ctx.clone(),
            Async(|ctx: Ctx<'js>, url: String, opts: Value<'js>| async move {
                request(ctx, "GET", url, None, opts).await
            }),
        )?;
        let post_fn = Function::new(
            ctx.clone(),
            Async(
                |ctx: Ctx<'js>, url: String, body: Value<'js>, opts: Value<'js>| async move {
                    request(ctx, "POST", url, Some(body), opts).await
                },
            ),
        )?;
        exports.export("get", get_fn)?;
        exports.export("post", post_fn)?;
        Ok(())
    }
}

struct HttpOpts<'js> {
    headers: Vec<(String, String)>,
    timeout: Option<Duration>,
    signal: Option<Object<'js>>,
}

fn parse_opts<'js>(opts: &Value<'js>) -> HttpOpts<'js> {
    let empty = HttpOpts {
        headers: Vec::new(),
        timeout: None,
        signal: None,
    };
    if opts.is_undefined() || opts.is_null() {
        return empty;
    }
    let Some(o) = opts.as_object() else {
        return empty;
    };

    let mut headers = Vec::new();
    if let Ok(h) = o.get::<_, Object<'js>>("headers") {
        for prop in h.props::<String, Value<'js>>().flatten() {
            let (k, v) = prop;
            if let Some(vs) = v.into_string()
                && let Ok(s) = vs.to_string()
            {
                headers.push((k, s));
            }
        }
    }

    let timeout = o
        .get::<_, f64>("timeoutMs")
        .ok()
        .filter(|ms| ms.is_finite() && *ms > 0.0)
        .map(|ms| {
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let millis = ms as u64;
            Duration::from_millis(millis)
        });

    let signal = o.get::<_, Object<'js>>("signal").ok();
    HttpOpts {
        headers,
        timeout,
        signal,
    }
}

async fn request<'js>(
    ctx: Ctx<'js>,
    method: &'static str,
    url: String,
    body: Option<Value<'js>>,
    opts: Value<'js>,
) -> JsResult<Object<'js>> {
    let HttpOpts {
        headers,
        timeout,
        signal,
    } = parse_opts(&opts);

    let mut builder = match method {
        "GET" => client().get(&url),
        "POST" => client().post(&url),
        _ => client().request(
            reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| {
                rquickjs::Error::new_loading_message("highbeam:http", e.to_string())
            })?,
            &url,
        ),
    };

    for (k, v) in headers {
        builder = builder.header(k, v);
    }
    if let Some(t) = timeout {
        builder = builder.timeout(t);
    }

    if let Some(body_val) = body {
        match coerce_body(&ctx, body_val)? {
            Some(BodyShape::Json(json_str)) => {
                builder = builder
                    .header("content-type", "application/json")
                    .body(json_str);
            }
            Some(BodyShape::Text(s)) => {
                builder = builder.body(s);
            }
            None => {}
        }
    }

    let token = if let Some(sig) = signal {
        abort::token_from_js_signal(&ctx, &sig)?
    } else {
        tokio_util::sync::CancellationToken::new()
    };

    let send_fut = builder.send();
    let response = tokio::select! {
        biased;
        () = token.cancelled() => {
            return Err(throw_abort(&ctx));
        }
        r = send_fut => match r {
            Ok(resp) => resp,
            Err(err) => return Err(throw_http(&ctx, &err.to_string())),
        }
    };

    let status = response.status();
    let status_u16 = status.as_u16();
    let status_text = status.canonical_reason().unwrap_or("").to_owned();
    let header_map: Vec<(String, String)> = response
        .headers()
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|s| (k.as_str().to_owned(), s.to_owned()))
        })
        .collect();

    let body_fut = response.text();
    let body_text = tokio::select! {
        biased;
        () = token.cancelled() => {
            return Err(throw_abort(&ctx));
        }
        r = body_fut => match r {
            Ok(t) => t,
            Err(err) => return Err(throw_http(&ctx, &err.to_string())),
        }
    };

    build_response(&ctx, status_u16, &status_text, &header_map, body_text)
}

enum BodyShape {
    Json(String),
    Text(String),
}

/// String values pass through as text; everything else goes through
/// `JSON.stringify` and sets `content-type: application/json`.
fn coerce_body<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> JsResult<Option<BodyShape>> {
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }
    if let Some(js_str) = value.clone().into_string() {
        return Ok(Some(BodyShape::Text(js_str.to_string()?)));
    }
    let json_global: Object<'js> = ctx.globals().get("JSON")?;
    let stringify: Function<'js> = json_global.get("stringify")?;
    let s: String = stringify.call((value,))?;
    Ok(Some(BodyShape::Json(s)))
}

fn build_response<'js>(
    ctx: &Ctx<'js>,
    status: u16,
    status_text: &str,
    headers: &[(String, String)],
    body: String,
) -> JsResult<Object<'js>> {
    let obj = Object::new(ctx.clone())?;
    obj.set("status", status)?;
    obj.set("statusText", status_text.to_owned())?;
    let header_obj = Object::new(ctx.clone())?;
    for (k, v) in headers {
        header_obj.set(k, v.clone())?;
    }
    obj.set("headers", header_obj)?;
    obj.set("body", body.clone())?;
    obj.set("ok", (200..=299).contains(&status))?;

    let body_for_text = body.clone();
    let text_fn = Function::new(ctx.clone(), move || body_for_text.clone())?;
    obj.set("text", text_fn)?;

    let body_for_json = body;
    let json_fn = Function::new(ctx.clone(), move |ctx: Ctx<'js>| -> JsResult<Value<'js>> {
        let json: Object<'js> = ctx.globals().get("JSON")?;
        let parse: Function<'js> = json.get("parse")?;
        parse.call((body_for_json.clone(),))
    })?;
    obj.set("json", json_fn)?;

    Ok(obj)
}

fn throw_http(ctx: &Ctx<'_>, message: &str) -> rquickjs::Error {
    throw_named(ctx, "HttpError", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_timeout_is_thirty_seconds() {
        assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(30));
    }

    #[test]
    fn client_is_lazily_constructed() {
        let a: *const Client = client();
        let b: *const Client = client();
        assert_eq!(a, b);
    }
}
