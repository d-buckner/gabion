//! Web host: the same core, compiled to wasm32. v0 uses an in-memory store and
//! a fixed clock (both wasm-safe); IndexedDB/js-date impls land with the
//! WebCaps pass. This crate's job right now is to PROVE the wall holds:
//! `cargo build -p host-web --target wasm32-unknown-unknown` must pass.

use gabion::caps::{Caps, Timestamp};
use gabion::testing::{InMemoryStore, MockClock};
use std::sync::Arc;
use todo_api::TodoService;
use todo_core::TodoServiceImpl;
use wasm_bindgen::prelude::*;

#[derive(Clone)]
pub struct WasmCaps {
    clock: MockClock,
    store: InMemoryStore,
}

impl Caps for WasmCaps {
    type Clock = MockClock;
    type Store = InMemoryStore;
    fn clock(&self) -> &Self::Clock {
        &self.clock
    }
    fn store(&self) -> &Self::Store {
        &self.store
    }
}

/// Browser entry: launches the same Dioxus `TodoApp` the desktop host uses,
/// with the wasm-side service injected via launch context.
#[wasm_bindgen(start)]
pub fn start() {
    let svc: Arc<dyn TodoService + Send + Sync> = Arc::new(TodoServiceImpl::new(wasm_caps()));
    dioxus::LaunchBuilder::new()
        .with_context(todo_render::Svc(svc))
        .launch(todo_render::TodoApp);
}

fn wasm_caps() -> WasmCaps {
    WasmCaps { clock: MockClock(Timestamp(0)), store: InMemoryStore::default() }
}

/// Entry point the browser glue would call (wired to JS in the next pass).
pub async fn demo() -> String {
    let caps = wasm_caps();
    let svc = TodoServiceImpl::new(caps);
    let _ = todo_render::on_add_clicked(&svc, "buy sandspice").await;
    todo_render::render_list(&svc).await.unwrap_or_default()
}
