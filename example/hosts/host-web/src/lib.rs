//! Web host: the same core, compiled to wasm32. v0 uses an in-memory store and
//! a fixed clock (both wasm-safe); IndexedDB/js-date impls land with the
//! WebCaps pass. This crate's job right now is to PROVE the wall holds:
//! `cargo build -p host-web --target wasm32-unknown-unknown` must pass.

use sietch::caps::{Caps, Timestamp};
use sietch::testing::{InMemoryStore, MockClock};
use todo_core::TodoServiceImpl;

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

/// Entry point the browser glue would call (wired to JS in the next pass).
pub async fn demo() -> String {
    let caps = WasmCaps { clock: MockClock(Timestamp(0)), store: InMemoryStore::default() };
    let svc = TodoServiceImpl::new(caps);
    let _ = todo_render::on_add_clicked(&svc, "buy sandspice").await;
    todo_render::render_list(&svc).await.unwrap_or_default()
}
