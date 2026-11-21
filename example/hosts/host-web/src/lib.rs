//! Web host: the same core, compiled to wasm32, or the generated client where a
//! page should not carry the core at all.
//!
//! Two postures, both of them one line at the composition root (spec §10.3):
//! `TodoServiceImpl` over a capability bundle runs the core in the page, and
//! `TodoClient` over `Http` is the thin client — the renderer is handed the same
//! `Arc<dyn TodoService>` either way. This crate's job right now is to PROVE the
//! wall holds: `cargo build -p host-web --target wasm32-unknown-unknown` must pass.

use gabion::caps::{Caps, Timestamp};
use gabion::prelude::Ctx;
use gabion::testing::{InMemoryStore, MockClock, MockHttp, MockRandom, MockSleep, MockStreams};
use std::sync::Arc;
use todo_api::{TodoClient, TodoService};
use todo_core::TodoServiceImpl;
use wasm_bindgen::prelude::*;

#[derive(Clone)]
pub struct WasmCaps {
    clock: MockClock,
    store: InMemoryStore,
    random: MockRandom,
    sleep: MockSleep,
    http: MockHttp,
    streams: MockStreams,
}

impl Caps for WasmCaps {
    type Clock = MockClock;
    type Store = InMemoryStore;
    type Random = MockRandom;
    type Sleep = MockSleep;
    type Http = MockHttp;
    type Streams = MockStreams;
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

/// Browser entry: launches the same Dioxus `TodoApp` the desktop host uses,
/// with the wasm-side service injected via launch context.
#[wasm_bindgen(start)]
pub fn start() {
    let svc: Arc<dyn TodoService + Send + Sync> = Arc::new(TodoServiceImpl::new(wasm_caps()));
    dioxus::LaunchBuilder::new()
        .with_context(todo_render::Svc::new(svc))
        .launch(todo_render::TodoApp);
}

fn wasm_caps() -> WasmCaps {
    let clock = MockClock::new(Timestamp(0));
    WasmCaps {
        sleep: MockSleep::new(clock.time().clone()),
        http: MockHttp::default(),
        store: InMemoryStore::default(),
        // Deterministic on purpose: this host is a wall proof, not a browser
        // app. A js-crypto impl lands with the WebCaps pass.
        random: MockRandom::new(0xA570_A570),
        clock,
        streams: MockStreams::new(),
    }
}

/// Entry point the browser glue would call (wired to JS in the next pass). The
/// second half is the thin-client posture: the same `TodoService` methods, off
/// the socket instead of the core.
pub async fn demo() -> String {
    let caps = wasm_caps();
    let svc = TodoServiceImpl::new(caps.clone());
    let _ = todo_render::on_add_clicked(&svc, &Ctx::anonymous(), "buy sandspice").await;
    let local = todo_render::render_list(&svc, &Ctx::anonymous())
        .await
        .unwrap_or_default();

    // A page that must not carry the core talks to a server instead. Nothing in
    // `todo-render` changes: it holds a trait object either way.
    let remote: &(dyn TodoService + Send + Sync) =
        &TodoClient::new("http://127.0.0.1:3456", caps.http().clone());
    let over_the_socket = todo_render::render_list(remote, &Ctx::anonymous())
        .await
        .unwrap_or_default();
    format!("{local}---\n{over_the_socket}")
}
