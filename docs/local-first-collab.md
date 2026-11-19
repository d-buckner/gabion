# Local-First Collaboration Design (gabion-collab)

Status: DRAFT v0 — design thinking for optional local-first sync. Decisions
marked [OPEN] need sign-off. Prior art: **p2piano** (automerge 0.9 + WS room
relay + WebRTC signaling types already proven there).

## 0. The one-line thesis

Collaboration is **another capability**, not another architecture — and the
app-facing surface is *two markers* (`#[collab]` on a data structure,
`#[collab(event)]` on a message type) plus one line of host wiring. The
consuming app never sees a transport, a room join, a sync message, or a
reconnect. Automerge is pure Rust and wasm-safe, so the CRDT lives *inside*
`*-core`; everything that touches a network is an effect behind the `Collab`
capability, exactly like `Store` and `Clock`. Apps opt in by adding one
feature; apps that don't, compile the same binary they always did.

## 1. Goals

1. **Optional**: the app-facing adoption cost is `#[collab]` on a struct +
   `#[collab(event)]` on a message type + one `.with_collab(url)` line in the
   host. Zero generated-code changes for non-collab apps; the same app code
   runs single-player with no URL wired (§4.2).
2. **Local-first**: the app is fully functional offline. Sync is a background
   reconciler, never a request/response dependency. Closing the laptop mid-edit
   loses nothing; reopening converges.
3. **Any app**: any gabion feature that opts in gets rooms, presence, and
   conflict-free merge for free — same "one trait, four surfaces" leverage.
4. **Server optional but useful**: signaling + relay + durable replica. The
   server is *a* peer, not *the* database.

## 2. Non-goals (v1)

- No mobile targets (consistent with framework spec §2).
- No E2EE room keys in v1 [OPEN: v1.1 — invite-URL key fragment is cheap].
- No iroh/quinn native overlay in v1 — native P2P rides WebRTC (see §5.1),
  which covers every peer combination with one protocol.
- No CRDT schema language — automerge *is* the schema.
- No replacing the four surfaces. HTTP/MCP/CLI stay; collab is additive.

## 3. Why this fits gabion's walls (the load-bearing observation)

The framework spec's whole thesis is that the crate graph and compile targets
do the policing. Collab slots in without bending a single rule:

| Concern | Where it goes | Why the wall holds |
|---|---|---|
| `automerge` crate | `*-core` deps | Pure Rust, no tokio/fs/net, wasm-safe (proven: p2piano-client builds it to wasm). The effect ban doesn't blink. |
| Sync state machine | `gabion-collab` (pure, wasm-safe) | automerge's `sync::State` + `generate_message`/`receive_message` is *already* a pure input→output state machine. No clock, no socket inside. |
| WebSocket client | capability impl: `gabion-native` (tokio-tungstenite), `gabion-web` (browser WS via wasm-bindgen) | Networking is an effect → lives in the effect home, same as `FileStore`. |
| WebRTC DataChannel | `gabion-web` (browser `RTCDataChannel` via `web_sys`) + `gabion-native` (`str0m`, pure-Rust WebRTC, data channels) | Browsers can *only* P2P via WebRTC, so WebRTC-everywhere is the single protocol that covers browser↔browser, browser↔native, and native↔native. |
| Signaling/relay server | `host-server` (axum WS upgrade) | Hosts are the composition + effect layer; axum already lives there. |
| Persistence of the doc | `Store` capability (already exists) | automerge `save()` → bytes → the same key-value store the app already has. |

**The sync engine is a reactor, not an actor.** It never opens a socket. It
takes `Vec<PeerEvent>` in and returns `Vec<PeerCmd>` out (these types are
framework-internal — the consuming app never sees them, §4.2):

```rust
// gabion-collab (pure, wasm-safe, runtime-free)
pub enum PeerEvent {
    Connected { peer: PeerId },
    Disconnected { peer: PeerId },
    Bytes { peer: PeerId, bytes: Vec<u8> },   // automerge sync message or signal
    LocalDocChanged,                           // "someone called a service fn"
}
pub enum PeerCmd {
    Send { peer: PeerId, bytes: Vec<u8> },    // host's transport executes this
    Persist { snapshot: Vec<u8> },            // host's store executes this
    Signal { peer: PeerId, sdp_or_ice: Vec<u8> },
}
```

The host (native main, web main, server main) owns the sockets and *pumps*
events in / commands out. This is exactly p2piano's `ClientRuntime` pattern
("pure state machine" in `p2piano-client`), generalized. Consequence: the
conformance suite (spec §8) can run a two-replica sync test with `MockCaps`
and zero network — determinism preserved.

## 4. The opt-in: what "including the collaboration feature" means

### 4.1 The crate

```
crates/gabion-collab/    # pure: sync reactor, room/presence types, wire protocol
crates/gabion-native/    # + WsTransport (tokio-tungstenite)          [feature "collab"]
                         # + WebRtcTransport (str0m)                  [feature "collab-p2p"]
crates/gabion-web/       # NEW: browser WS + WebRTC DataChannel impls [feature "collab"]
```

`gabion-collab` deps: `automerge`, `serde`, `gabion`. Nothing else. It compiles
on wasm32 — that is the wall proof, same as `cargo build -p todo-core --target
wasm32-unknown-unknown` today.

### 4.2 The ideal DX: the app never sees the network

The bar is not "collab is available" — it is "collab is ambient". The
consuming app declares *what* is shared and never thinks about *how* it gets
there. Three declarations, zero networking concepts:

```rust
// todo-api — the entire collaboration surface of a feature:

#[collab]                                    // state: durable, convergent, offline-safe
pub struct TodoDoc {
    pub items: CollabVec<Todo>,              // sequence CRDT (RGA)
    pub notes: CollabMap<TodoId, Note>,      // map CRDT, merges per key
    pub draft: CollabText,                   // text CRDT (collaborative textarea)
    pub title: String,                       // plain field → LWW register
}

#[collab(event)]                             // events: ephemeral fanout, not persisted
pub struct Typing { pub peer: PeerId, pub item: TodoId }
```

```rust
// todo-core — business logic just edits. Sync is ambient.
impl<C: Caps> TodoService for TodoServiceImpl<C> {
    async fn create(&self, input: CreateTodo) -> Result<Todo, TodoErr> {
        let doc = self.caps.collab().doc::<TodoDoc>();  // local replica; always available, even offline
        let todo = Todo { id: new_id(), title: input.title, done: false };
        doc.items.push(todo.clone());                  // ← sync happens. App doesn't know.
        self.caps.collab().emit(Typing { .. });        // ephemeral fanout, same deal
        Ok(todo)
    }
}
```

```rust
// todo-render — reads and live updates; no fetch, no polling:
let typing: impl Stream<Item = Typing> = collab.subscribe::<Typing>();
let peers: Vec<Presence> = collab.presence();
```

Host wiring — the *only* place networking appears, and it's config, not code:

```rust
let caps = NativeCaps::new().with_collab(CollabConfig { url: env("MYAPP_SYNC") });
```

No URL / no `.with_collab` → the `Collab` capability degrades to a **local
null impl**: same app code, single-player, zero network. "Optional" is
wiring-enforced, never a code fork.

**Two primitives, two semantics** — everything the app shares is one of them:

| App writes/reads | Primitive | Semantics |
|---|---|---|
| `doc.items.push(..)` | automerge (CRDT) | durable, convergent, offline-buffered, replayed on reconnect |
| `collab.emit(Typing{..})` | ephemeral fanout | at-most-once, dropped while offline (right for typing/cursors/toasts) |
| `collab.presence()` | framework-synthesized join/leave | ephemeral, never in the CRDT |

**Framework-owned, app never touches:** `Transport`, `PeerEvent`/`PeerCmd`,
rooms (default = feature name), sync messages, reconnect/backoff, offline
buffering, ladder selection (try T2, fall back to T1), persist debouncing,
presence synthesis. The §3 reactor types are internal plumbing.

Principles (mechanical, gabion-style):

1. **Registration without wiring, same as everything else.** `#[collab]`
   registers via linkme; the sync loop lives *inside* the `Collab` capability
   impl in gabion-native/gabion-web — not in the app's host main(). (§8.3
   resolved.)
2. **Writes dirty automatically.** Mutation through a `Collab` handle marks
   the doc dirty; the capability impl flushes (persist + sync) at the service
   call boundary / on debounce. There is no `save()` or `broadcast()` to
   forget — forgetting is unrepresentable, that's the wall.
3. **Reads never block on the network.** `doc::<T>()` is local memory; there
   is no async "fetch room state" API to accidentally write.

Codegen deltas when a feature contains `#[collab]` items (apps without any:
byte-identical output, same discipline as the surface lock: no codegen unless
the feature asks for it):

| Surface | Delta |
|---|---|
| core impl | `Caps` gains `type Collab: Collab`; impl gains the `collab()` handle |
| server | + `GET /collab/ws/{room}` upgrade; relay + durable replica (auto, via linkme) |
| client (generated) | unchanged signatures — mutations still go through the trait |
| render | + `subscribe`/`presence` reads via the stream contract (§4.4) |
| CLI | + `myapp collab status` (diagnostics only — the app didn't write it) |

### 4.3 Typed collab structures (the hard part, now named)

`#[derive(Collab)]` maps typed Rust onto the schemaless automerge tree — this
*is* the design work §8.4 points at:

- plain fields (scalars/small structs) → LWW put at path `field`
- `CollabVec<T>` → automerge sequence (RGA: concurrent inserts both survive)
- `CollabMap<K, V>` → automerge map (merge per key)
- `CollabText` → automerge text (char-level merge)

Codegen is serde-style but emits automerge ops (`put_scalar`/`put_map`/
`insert`), not JSON. [OPEN: v1 escape hatch — whole-struct serde blob at one
key (LWW per field, coarse but correct) so the derive can be refined
incrementally?]

### 4.4 Peer events

- **At-most-once**, fire-and-forget; dropped while offline. Correct for
  typing indicators, cursors, toasts, "user joined" sounds — and wrong for
  anything the user would call data. That's what the doc is for.
- `subscribe::<E>()` returns a `futures::Stream` — this is the wedge that
  forces the framework's undefined stream contract (framework spec §2).
  Budget it in v1.
- Presence is a built-in event stream, not app code: the framework synthesizes
  join/leave/snapshot from the room protocol (§7).

### 4.5 Document granularity — [OPEN, the big one]

One automerge doc per **room**, room ≈ feature aggregate (`"todos"`), not per
entity and not per app.

- Per-entity: N docs, no cross-entity atomicity, sync storm (N sync states per
  peer). Reject.
- Per-app: one doc for everything — coarse invalidation, unrelated features
  contend. Tempting but wrong.
- Per-feature-room: matches the feature wall 1:1, sync states per feature,
  features already can't import each other's internals. **Default.**

Cross-feature references stay by ID (feature wall), so cross-doc joins are just
reads — no cross-doc transactions needed. This is a lucky alignment: the
feature wall *is* the document boundary.

### 4.6 Rooms: yes as machinery, no as chore

Yes, the framework has a room concept — because four concerns must share one
key, and if they're separate knobs apps will let them drift:

| Concern | What breaks without a shared room key |
|---|---|
| sync domain (one automerge doc) | peers with different doc sets can't converge |
| presence scope | "who's here" is meaningless without a "here" |
| authz scope | nothing for an invite / capability token to grant (§10.5) |
| persistence partition | no key for `doc:{room}` in `Store` (§6) |

But the app should barely notice it, per the §4.2 DX bar:

- **Default: implicit singleton room per feature.** `#[collab]` on `TodoDoc`
  ⇒ room `"todos"`. Most apps never write the word "room" at all.
- **Partitioned domains opt in with their own identity.** When the domain has
  partitions (per-project, per-playlist, per-document-slug), the app scopes
  the doc with an ID it *already has*:

  ```rust
  let doc   = collab.doc_in::<ProjectDoc>(RoomKey::of(project_id));
  let peers = doc.presence();          // presence is scoped to the doc's room
  ```

  That is domain vocabulary, not networking vocabulary — the framework
  invents no IDs, it borrows yours. `#[collab(partitioned_by = ProjectId)]`
  makes it codegen-visible so the server can enumerate rooms.
- **Peer ≠ room.** One connection, many rooms: the protocol multiplexes
  (`join`/`sync`/`signal` carry the room field). A peer can be in three
  project rooms plus the `"todos"` singleton simultaneously; the WS
  connection is per-peer, the room is a logical partition over its messages.
- **Two lifetimes under one key.** The presence set is ephemeral (dissolves
  when empty); the doc is durable (server replica keeps it, §5.2). "Room"
  names both — worth being explicit about, because "leave the room" must
  never imply "delete the data".
- **Room is the unit of authz.** An invite grants a room, never "the app's
  collab". This is why the room must exist as a named entity even when the
  app API hides it — tokens need an audience.

Naming note: "room" is p2piano's word and it's friendly, but the concept is
*sync domain* first, party second. [OPEN: room vs scope vs space — cheap to
rename now, expensive after public API.]

## 5. Sync protocol

Don't invent one. Automerge has a wire-ready sync protocol (`automerge::sync`:
`Message`, `State`, `generate_message`, `receive_message`) — p2piano already
ships its bytes as `RoomDocumentSync { message: Vec<u8> }`.

Envelope over the wire (JSON-tagged like p2piano's `ClientEvent`/`ServerEvent`,
binary payloads base64'd in v1; binary WS frames as optimization later):

```
client → server:  join {room, peer_id?} | sync {to, message} | signal {to, sdp/ice} | leave
server → client:  joined {self, peers} | peer_joined | peer_left
                  sync {from, to?, message} | signal {from, to, sdp/ice}
```

`to` absent = broadcast to room. That single shape covers relay *and*
signaling — it is literally p2piano's protocol with `NoteOn/NoteOff` replaced
by `sync`.

### 5.1 Transport ladder (all behind one `Transport` capability)

| Tier | Path | When | Status |
|---|---|---|---|
| T0 | in-process loopback | tests, single-binary demo | build first |
| T1 | WS relay via host-server | always works; server is durable peer | v1 |
| T2 | WebRTC DataChannel, WS = signaling only | **all** peer pairs: browser↔browser (`web_sys`), browser↔native + native↔native (`str0m` in gabion-native) | v1.1 (p2piano protocol already has `WebRtcSignal`) |
| T3 | iroh/quinn native overlay | optional native↔native upgrade (better hole-punching, persistent node IDs) | v2 [OPEN: only if native↔native outgrows WebRTC; cannot interop with browsers, so never a substitute for T2] |

Two consequences worth stating plainly:

1. **WebRTC-everywhere is forced, not chosen.** A browser can only do P2P over
   WebRTC (WebTransport is client→server only — the responder would need to
   host a TLS server behind NAT). So the only protocol that achieves true P2P
   for *every* peer combination is WebRTC on both sides. Native speaks WebRTC
   via `str0m` (pure Rust, data-channel capable, no C deps — keeps the
   single-binary promise) or `webrtc-rs`.
2. **No TURN infrastructure needed.** When ICE hole punching fails, the pair
   falls back to T1 — gabion's own WS relay *is* the relay. The framework
   ships its own TURN. (TURN servers remain an option for deployments that
   want them; the ICE config is just another host-wired capability.)

`Transport` trait (in `gabion`, next to `Store`):

```rust
#[async_trait]
pub trait Transport: Clone + Send + Sync + 'static {
    async fn send(&self, room: &RoomId, peer: Option<PeerId>, bytes: Vec<u8>) -> Result<(), TransportError>;
    // host pushes inbound bytes into the reactor; trait is outbound-only on purpose
}
```

Outbound-only is deliberate: inbound is the host calling
`reactor.handle(PeerEvent::Bytes{..})`, which keeps the reactor pure and makes
T0/T1/T2 interchangeable — the reactor doesn't know which tier it's on.

### 5.2 The server's role (why "optionally" works)

The server runs the *same* core + reactor as a client — it's just the peer
that's always up:

1. **Signaling**: relay `signal` messages (no WebRTC knowledge needed).
2. **Relay**: forward `sync` messages when peers can't P2P (T1 fallback).
3. **Durable replica**: keeps a full automerge doc per room so a lone offline
   client that synced weeks ago can catch up, and the server DB is just
   `Store` — no schema.

Authority model: **no authoritative server.** Every replica is equal; automerge
converges everyone. The HTTP surface (`POST /todo/create`) still works — it's
the server replica taking a write, identical to any peer's write. MCP and CLI
likewise. The four surfaces become "four ways to mutate one replica," which is
a genuinely elegant sentence and needs no new machinery.

## 6. Persistence

`Store` (JSON-string KV) + automerge:

- **v1**: on `LocalDocChanged` (debounced) → `store.set("doc:{room}", base64(save(doc)))`.
  Full snapshot, dead simple, fine below ~MB docs.
- **v1.1**: incremental — append `doc.get_changes(&[])` deltas + periodic
  compaction snapshot. Automerge's change log is designed for this.
- Reactor emits `PeerCmd::Persist`; core never calls store directly for the doc
  (the reactor owns doc persistence) — keeps service fns clean.

## 7. Presence

Copy p2piano's wire shape verbatim: `RoomJoined { self_user_id, users }`,
`UserJoined`, `UserLeft`, `PresenceSnapshot` on reconnect. Presence is
ephemeral (not in the CRDT — it must not persist). The framework synthesizes
it from the room protocol and surfaces it as `collab.presence()` + a
`PresenceEvent` stream (§4.4) — the app writes zero presence code. The old
[OPEN] on how render sees presence is resolved by the stream contract in
§4.4.

## 8. The honest tensions (where it actually hurts)

1. **Read path identity.** Today: render → client → core → store, one truth.
   Local-first: render reads the *local replica*, which may lag a peer's write
   by sync latency. Fine for todos; a lie for "bank balance". [OPEN: per-fn
   `#[consistency(strong)]` marker that routes through server? Or declare
   gabion-collab "eventual, full stop" and let strong-consistency features
   skip `#[collab]`? — I lean full-stop eventual.]
2. **Presence needs streaming** (§7) — collab is the first real demand for the
   framework's reserved-but-undefined stream contract. Budget for designing
   `Stream` in v1, not v2.
3. **Sync loop ownership.** ~~Each host grows a ~30-line `collab_loop` in
   main.~~ **Resolved by the DX bar (§4.2):** the loop lives inside the
   `Collab` capability impls (gabion-native: tokio task; gabion-web: JS task),
   auto-wired when the host calls `.with_collab(..)`. Hosts stay composition-
   only; `fw check` needs no carve-out.
4. **Automerge doc shape vs service DTOs.** Service fns take typed DTOs; the
   doc is a dynamic automerge tree. The `#[derive(Collab)]` design (§4.3) is
   the real design work left — it's where "typed service" meets "schemaless
   CRDT". p2piano's room document is a small precedent; gabion needs a
   repeatable, *derived* pattern.
5. **`Vec<u8>` in a JSON-string Store**: base64 bloat. Either accept it in v1
   or widen `Store` to bytes. [OPEN: widen now — it's a 3-line trait change and
   every future binary payload benefits.]

## 9. Cheapest experiments first (same style as spec §11)

1. **E1 — automerge passes the wall** (½ day): add `automerge` to `todo-core`,
   store todos in an `AutoCommit`, `cargo build -p todo-core --target
   wasm32-unknown-unknown && cargo clippy --workspace`. Expected: green on
   first try (p2piano proves it). This de-risks the entire premise.
2. **E2 — two replicas, zero network** (1–2 days): `gabion-collab` reactor + T0
   in-memory transport + the `Collab` capability with the §4.2 handle API
   (`doc::<T>()`, `emit`; blob-first derive is fine). Deterministic test: two
   replicas, interleaved offline edits, pump messages, assert `doc == doc'` —
   *through the app-facing API*, so the DX is proven before any network code
   exists. This is the conformance-suite seed for collab.
3. **E3 — WS relay** (1–2 days): `GET /collab/ws/{room}` in host-server
   (axum WS), T1 transport in gabion-native + gabion-web. Native CLI peer +
   browser peer converge through the server. This is "p2piano's server,
   generalized."
4. **E4 — WebRTC upgrade** (2–3 days): port p2piano's `WebRtcSignal` flow;
   DataChannel carries `sync`, WS demoted to signaling; automatic fallback to
   T1 on failure. Do browser↔browser first (browser API is free), then the
   `str0m` native peer — the native↔browser convergence test is the proof
   that P2P is peer-type-agnostic.
5. **E5 — `#[collab]` codegen + derive** (after E1–E3 prove the runtime):
   macro emits the WS route + auto-wires the capability into all hosts;
   `#[derive(Collab)]` v1 (blob or field-level per §4.3); `fw` scaffolds a
   collab feature.

Order matters: E1–E2 are pure and cheap and kill the two scariest unknowns
(wasm-safety, convergence determinism) before any network code exists.

## 10. Open decisions

1. Doc granularity default: per-feature room (§4.5) — sign-off.
2. Consistency posture: eventual-only vs per-fn strong escape hatch (§8.1).
3. `Store` widen to `Vec<u8>` now or base64 in v1 (§8.5).
4. Native WebRTC impl: `str0m` (minimal, pure Rust, we own ICE/SDP glue) vs
   `webrtc-rs` (batteries included, heavier). Decide at E4; the `Transport`
   trait makes it swappable. iroh stays a v2 maybe for native↔native only.
5. Room auth: open rooms + capability tokens? Room is already the authz unit
   (§4.6) — ties into gabion's auth story which doesn't exist yet; collab
   shouldn't invent it alone.
6. Room naming: room vs scope vs space (§4.6) — decide before public API.
6. E2EE: per-room key in invite URL fragment, encrypt `sync` payloads; cheap
   and local-first-appropriate — v1.1?
7. Does MCP-on-server mutate the server replica like any peer (yes, per §5.2 —
   confirm it stays that simple).
8. Automerge version pin: p2piano is on 0.9; check 0.9→latest for sync-protocol
   wire breaks before E3 freezes the envelope.
9. Derive granularity: field-level automerge ops from day 1, or serde-blob
   escape hatch in v1 (§4.3)?
10. Event semantics: is at-most-once forever, or does anything need
    `#[collab(event(buffered))]` with a small reconnect replay?
