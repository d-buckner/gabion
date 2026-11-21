//! Browser capabilities: the wasm counterpart of `gabion-native`'s bundle.
//!
//! Same five traits the cores already take — js clock, localStorage store,
//! `crypto.getRandomValues`, `setTimeout` timer, `fetch` http — so the identical
//! `checks-core` and `incidents-core` run here. No core knows it is in a browser.
//!
//! Two deliberate limits, both visible at the boundary:
//!
//! - `fetch` is CORS-bound: a probe of a site that sends no
//!   `Access-Control-Allow-Origin` fails as a transport error, which is an
//!   honest "did not answer" from this vantage point.
//! - Response headers are dropped (v1 carries JSON bodies), so the browser http
//!   capability reports an empty header list rather than pretending otherwise.

use crate::bridge::bridge;
use gabion::async_trait;
use gabion::caps::{
    Caps, Clock, Http, HttpError, HttpRequest, HttpResponse, Random, Sleep, Store, StoreError,
    Timer, Timestamp,
};
use gabion_web::WebStreams;
use js_sys::{ArrayBuffer, Date, Promise, Uint8Array};
use std::future::Future;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, Request, RequestInit, RequestMode, Response};

/// js `Date.now()`, truncated to whole seconds like the native clock.
#[derive(Clone, Copy, Default)]
pub struct WebClock;

#[async_trait]
impl Clock for WebClock {
    async fn now(&self) -> Timestamp {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // ms since epoch
        Timestamp((Date::now() / 1000.0) as u64)
    }
}

/// `crypto.getRandomValues`, with `Math.random` as the non-secure-context
/// fallback (plain-http pages have no `crypto`).
#[derive(Clone, Copy, Default)]
pub struct WebRandom;

#[async_trait]
impl Random for WebRandom {
    async fn next_u64(&self) -> u64 {
        let mut bytes = [0u8; 8];
        if fill_random(&mut bytes) {
            return u64::from_le_bytes(bytes);
        }
        fallback_random()
    }
}

fn fill_random(bytes: &mut [u8]) -> bool {
    web_sys::window()
        .and_then(|window| window.crypto().ok())
        .is_some_and(|crypto| crypto.get_random_values_with_u8_array(bytes).is_ok())
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn fallback_random() -> u64 {
    let hi = (js_sys::Math::random() * 4_294_967_296.0) as u64;
    let lo = (js_sys::Math::random() * 4_294_967_296.0) as u64;
    (hi << 32) | lo
}

/// localStorage, key for key: the same JSON strings the file store holds, so a
/// board exported from the desktop app can be pasted into the browser.
#[derive(Clone, Copy, Default)]
pub struct LocalStore;

fn io(error: &JsValue) -> StoreError {
    StoreError::Io(format!("{error:?}"))
}

fn storage() -> Result<web_sys::Storage, StoreError> {
    web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .ok_or_else(|| StoreError::Io("localStorage is unavailable".to_string()))
}

#[async_trait]
impl Store for LocalStore {
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        storage()?.get_item(key).map_err(|e| io(&e))
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        storage()?.set_item(key, value).map_err(|e| io(&e))
    }

    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        storage()?.remove_item(key).map_err(|e| io(&e))
    }

    async fn list(&self, prefix: &str) -> Result<Vec<(String, String)>, StoreError> {
        let store = storage()?;
        let len = store.length().map_err(|e| io(&e))?;
        let mut out = Vec::new();
        for index in 0..len {
            let Some(key) = store.key(index).map_err(|e| io(&e))? else {
                continue;
            };
            if !key.starts_with(prefix) {
                continue;
            }
            if let Some(value) = store.get_item(&key).map_err(|e| io(&e))? {
                out.push((key, value));
            }
        }
        // The native store scans in lexicographic order; match it, because
        // `checks-core` relies on that order for probe history.
        out.sort_by(|(a, _), (b, _)| a.cmp(b));
        Ok(out)
    }
}

/// `setTimeout`, as both a wait and a deadline. A real clock has no distinction.
#[derive(Clone, Copy, Default)]
pub struct WebSleep;

impl Sleep for WebSleep {
    fn sleep(&self, millis: u64) -> Timer {
        Box::pin(delay(millis))
    }

    fn timer(&self, millis: u64) -> Timer {
        Box::pin(delay(millis))
    }
}

fn delay(millis: u64) -> impl Future<Output = ()> + Send + 'static {
    bridge(async move {
        let promise = Promise::new(&mut |resolve, _reject| {
            let Some(window) = web_sys::window() else {
                return;
            };
            #[allow(clippy::cast_possible_truncation)] // setTimeout takes an i32
            let timeout = millis.min(i32::MAX as u64) as i32;
            let handler: &js_sys::Function = resolve.unchecked_ref();
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(handler, timeout);
        });
        let _ = JsFuture::from(promise).await;
    })
}

/// `window.fetch`. A network failure is a transport error, never a status; a
/// 404 is a response, and `checks-core` decides what that means.
#[derive(Clone, Copy, Default)]
pub struct FetchHttp;

#[async_trait]
impl Http for FetchHttp {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        bridge(fetch(request)).await
    }
}

fn transport(error: &JsValue) -> HttpError {
    HttpError::Transport(message(error))
}

fn message(error: &JsValue) -> String {
    error
        .dyn_ref::<js_sys::Error>()
        .map(|err| String::from(err.message()))
        .unwrap_or_else(|| format!("{error:?}"))
}

async fn fetch(request: HttpRequest) -> Result<HttpResponse, HttpError> {
    let window = web_sys::window().ok_or_else(|| {
        HttpError::Transport("no window: this capability needs a browser".to_string())
    })?;
    let init = RequestInit::new();
    init.set_method(request.method.as_str());
    init.set_mode(RequestMode::Cors);
    if !request.body.is_empty() {
        let body: JsValue = Uint8Array::from(&request.body[..]).into();
        init.set_body(&body);
    }
    let headers = Headers::new().map_err(|e| transport(&e))?;
    for (name, value) in &request.headers {
        headers.set(name, value).map_err(|e| transport(&e))?;
    }
    init.set_headers(&headers);

    let start = Request::new_with_str_and_init(&request.url, &init).map_err(|e| transport(&e))?;
    let promise = window.fetch_with_request(&start);
    let reply: Response = JsFuture::from(promise)
        .await
        .map_err(|e| transport(&e))?
        .dyn_into()
        .map_err(|e| transport(&e))?;
    Ok(read(reply).await)
}

async fn read(reply: Response) -> HttpResponse {
    let status = reply.status();
    let headers = Vec::new();
    let body = match reply.array_buffer() {
        Ok(promise) => match JsFuture::from(promise).await {
            Ok(buffer) => bytes(&ArrayBuffer::from(buffer)),
            Err(_) => Vec::new(),
        },
        Err(_) => Vec::new(),
    };
    HttpResponse {
        status,
        headers,
        body,
    }
}

fn bytes(buffer: &ArrayBuffer) -> Vec<u8> {
    let view = Uint8Array::new(buffer);
    #[allow(clippy::cast_possible_truncation)] // wasm32 usize is the wasm memory size
    let mut out = vec![0u8; view.length() as usize];
    view.copy_to(&mut out);
    out
}

/// The browser bundle: js clock, localStorage, crypto random, setTimeout,
/// fetch. Cloned into both services by the host, exactly like `NativeCaps`.
#[derive(Clone, Default)]
pub struct WebCaps {
    clock: WebClock,
    store: LocalStore,
    random: WebRandom,
    sleep: WebSleep,
    http: FetchHttp,
    streams: WebStreams,
}

impl WebCaps {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Caps for WebCaps {
    type Clock = WebClock;
    type Store = LocalStore;
    type Random = WebRandom;
    type Sleep = WebSleep;
    type Http = FetchHttp;
    type Streams = WebStreams;

    fn clock(&self) -> &Self::Clock {
        &self.clock
    }
    fn store(&self) -> &Self::Store {
        &self.store
    }
    fn random(&self) -> &Self::Random {
        &self.random
    }
    fn sleep(&self) -> &Self::Sleep {
        &self.sleep
    }
    fn http(&self) -> &Self::Http {
        &self.http
    }
    fn streams(&self) -> &Self::Streams {
        &self.streams
    }
}
