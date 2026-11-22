# Streaming surfaces — the decision

Status: **DECIDED** — this closes open question 4 of the framework spec (§10). It
answers a downstream proposal for terminal-pane streams and the ask in
[local-first-collab.md](local-first-collab.md) §4.4 and §7.

Nothing here is built. The refusal in `gabion-macros` stays until stage 3 of
§11, and the house rule still applies: the feature lands with the failing
fixtures in §12, not before them.

## 1. Verdict

**v1, on one WebSocket, with the policy carried by the type.**

| | |
|---|---|
| **In v1** | Stream ops in a `#[service]` trait, as a pair of ordinary shapes: a **listen** op returns a stream, a **send** op is an ordinary op carried on the same connection. Policy is the return-type marker (`Each<T>` / `Latest<T>`), not an attribute. Transport is **one WebSocket per host**, multiplexed by feature namespace, either side able to speak. Per-stream ordering, explicit termination, typed error mapping. CLI prints newline-delimited JSON on stdout. MCP carries it over stdio with progress notifications. A wasm host can consume one. `TestCaps` can script one. |
| **Deferred, named** | Per-stream flow control over the shared connection (§5), replay, resume, at-least-once, cross-process fan-out, a subscription registry, a binary framing. |
| **Declined** | Building the HTTP cancellation channel on the strength of this feature (§10). The name `lossless`. Any default policy. |

One of the proposal's thirteen requirements is in conflict with another as written
(S3 against S4, §5) and the resolution is a documented limit rather than a
protocol. One of its two arguments for "now" also turns out to be an argument for
a smaller feature than the one proposed (§2).

## 2. The two consumers do not need the same thing

The proposal says gabion-collab is the stronger argument, because
`subscribe::<E>()` "forces the framework's undefined stream contract". Reading
the collab doc, it does not.

`collab.subscribe::<Typing>()` (collab §4.2, §4.4) is a method on the `Collab`
**capability handle**, not an op on a `#[service]` trait:

```rust
let typing: impl Stream<Item = Typing> = collab.subscribe::<Typing>();
```

And the WebSocket route collab needs — `GET /collab/ws/{room}`, collab §4.2's
codegen table — is collab's own CRDT sync transport: its own protocol, its own
route, framework machinery rather than a generated service surface. It will sit
next to the generated stream endpoint (§5), not on it. So what collab needs from
this decision is **a stream type the framework can name in a capability
signature**, wasm-safe. It needs nothing from `#[service]`.

The second consumer needs the service op: a `watch(pane_id)` a typed client can
call over a socket it does not own — a terminal renderer with fifty panes on one
connection, where polling a unary `read_grid` either burns a round trip per frame
or adds latency a user sees.

That is still two consumers, and still worth designing once — but the shared part
is the *type and its semantics*, not the transport count. It also means collab's
"budget for designing `Stream` in v1" is satisfied by stage 1 below, which is a
day of work and is the one piece of this that is independent of the transport.

## 3. The contract

Two primitives, and they are the two an app already reaches for: **listen** and
**send**. Only the cardinality of the output changes for a listen op; a send op is
an ordinary op and looks like every other op in the trait.

```rust
#[service]
pub trait TerminalService {
    async fn pane_get(&self, ctx: &Ctx, input: PaneId) -> Result<Pane, DhErr>;

    // listen, conflating: the newest frame is what matters
    async fn watch(&self, ctx: &Ctx, input: PaneId) -> Result<Latest<Frame>, DhErr>;

    // listen, every item: the producer waits for the subscriber
    async fn subscribe(&self, ctx: &Ctx, input: RoomId) -> Result<Each<PresenceEvent>, DhErr>;

    // send: an ordinary op, carried on the connection that carries the stream
    async fn send_keys(&self, ctx: &Ctx, input: Keys) -> Result<(), DhErr>;
}
```

A duplex feature is a listen op and a send op in the same namespace, not one op
with two directions. That is the whole trick, and it is what keeps this feature
small: the inbound side needs no new vocabulary, no new macro path, no new lock
semantics and no new conformance case — `send_keys` is a normal op that happens to
travel on the socket the client already has open.

The alternative — a marker in an argument position, so one op carries both
directions — is rejected. It adds a second, position-sensitive grammar to the
macro, breaks the "`&Ctx` and exactly one input" grain every generator assumes
(`collect_methods` exists precisely to enforce that grain), and buys nothing the
pair does not. If a backpressured *inbound* stream is ever genuinely needed, that
is the v2 escalation, and it is named in §13 rather than left as a rumour.

`gabion::stream::{Each, Latest, BoxStream, Stream}` are the whole vocabulary.

- `Each<T>` and `Latest<T>` are **signature markers**, and they appear in the return
  position only. They are never serialized, never appear on the wire, and are
  constructed only by the impl (`Latest::new(stream)`). `T: Serialize + JsonSchema`
  is the only bound they add to a contract.
- They live in `gabion`, so a contract crate gains **no new dependency** to stream.
- The macro recognises them the way it recognises `&Ctx`: by the last segment of
  the path, so `gabion::stream::Latest<..>` and a re-export both read the same.
- A core builds a stream by *composing* — map, filter, take, merge, chain, and
  `caps::timeout` over a source it was handed. It does not create a source; a
  source arrives from a capability or from another service's op (§8).

## 4. The three sub-decisions

### 4.1 The stream type: alias `futures_core::Stream`, boxed and pinned

```rust
pub use futures_core::Stream;
```

Not a gabion-owned `poll_next`-shaped trait. A private trait would break interop
with every consumer's existing `futures` code — the collab doc already writes
`impl Stream<Item = Typing>`, and a real frame source is a `futures` stream
already. `futures-core` is in `Cargo.lock` today (transitively), has no runtime,
no unsafe in its own API surface, and builds for `wasm32-unknown-unknown`. It is
one new direct dependency on `gabion`, and it is the smallest crate in the
ecosystem that means this.

### 4.2 `Send` on wasm: yes, split — and it is bigger than the alias

```rust
#[cfg(not(target_arch = "wasm32"))]
pub type BoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + Send + 'a>>;
#[cfg(target_arch = "wasm32")]
pub type BoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + 'a>>;
```

The split is not optional, and it is not only the alias. The marker appears in the
method's `Output`, so on wasm the future `async_trait` boxes for that method also
stops being `Send`, and the macro must emit `#[async_trait(?Send)]` for wasm
builds. **This is the item most likely to cost a day**, because it changes every
method's boxed future on that target, not just the streaming ones.

It is also not theoretical. `caps::Timer` is `Pin<Box<dyn Future + Send>>` today
and can stay: a JS `setTimeout` future is `Send`. A `web_sys::WebSocket` is not —
`js_sys` objects are `!Send` — so a wasm host that streams from a real JS source
cannot build a `+ Send` stream at any cost. That is S11, and the split is its
answer. Keep both `cfg`s in `gabion::stream` plus one in the macro; nothing else
in the framework or in generated code is allowed to write `Send` into a stream
signature.

### 4.3 Policy is a type, not an attribute — and the reason is the lock

`crates/gabion-cli/src/surfaces/mod.rs` hashes `f.sig.to_token_stream()`. A
method's **attributes are not part of `sig`**. So
`#[stream(policy = "latest")]` changed to `#[stream(policy = "lossless")]` would
**not move the hash in `.gabion/surfaces.lock`** — and a policy flip changes
whether items get dropped, which is exactly the kind of wire-visible change the
lock exists to catch. That is a fail-open diff of the class GAPS §1 ranks worst,
and it would be built into the feature from day one.

With the marker in the return type, the flip is a hash change for free, and
`the_lock_moves_when_a_policy_flips` is a two-line fixture.

Two supporting reasons:

- Spec §10.5 decided there are no per-method attributes, and `#[surface]` was
  deleted rather than fixed. `#[stream]` is about cardinality rather than
  reachability, but it is still the first per-method modifier after that
  decision, and the type keeps the rule literally true instead of requiring a
  paragraph of exception prose.
- The generator needs the policy anyway — it picks the shape of the transport loop
  (conflate in one slot, or await each write). Reading it from the type is no more
  work than reading it from an attribute.

Cost, stated: the OpenAPI generator and the conformance generator must unwrap the
marker to reach `T`, and until they do, the marker must be refused (§11, stage 0).

`lossless` is rejected as a name because it lies: with no replay and no
persistence, a dropped connection loses whatever was in flight. `Each<T>` says
what it promises — every item that is produced before the stream ends.

## 5. Transport: one WebSocket, namespaced by feature

Full duplex is a requirement, and that settles the transport: **one WebSocket per
host**, carrying every stream op — and, for a feature that has one, that feature's
ordinary ops too. A request/response route can stream a *response*, but it cannot
let the server initiate a message on a channel the client opened, so NDJSON on the
existing POST route is not the transport — it was the alternative only while
streams were one-way reads.

Multiplexing is deliberately not a protocol project. One connection, one frame
envelope, and the unit of separation is the **feature namespace** — which the
macro already computes (`Contract::ns`: the trait name minus `Service`,
snake_cased — `TodoService` is `todo`, `HTTPService` is `http`, because a run of
capitals is one word; the same string that spells `route()` and the MCP tool
name). No new naming scheme is introduced.

Frames, both directions, one JSON object each:

```text
client -> server   open   {id, op: "terminal.watch", input}      -> item… end | error
                   call   {id, op: "terminal.send_keys", input}  -> reply | error
                   close  {id}
server -> client   item   {id, ns: "terminal", data}
                   reply  {id, ok}
                   end    {id, reason: "done" | "cancelled"}
                   error  {id, error: WireError}
```

`op` is the tool name gabion already mints (`{ns}.{method}`), so a frame names an
operation the way a route, an MCP tool and a CLI subcommand name it.

**A feature that streams puts all of its ops on the connection.** If a feature has
at least one listen op, the generated client carries that feature's other ops as
`call` frames on the same channel instead of as separate POSTs. A feature with no
listen op is untouched — same POSTs, byte for byte, which is the same discipline
the codegen already follows for `#[collab]`. One rule, one sentence, and it means a
terminal client holds one socket rather than a socket plus a connection pool, and
`send_keys` can be ordered against the stream it feeds.

**Ordering, and the one subtlety.** `call` frames are dispatched **in arrival order
per namespace**, so two keystrokes cannot overtake each other. Control frames
(`close`) are handled immediately, out of band. That split is not decoration:
gabion's MCP stdio pump had to be made *concurrent* precisely because a sequential
pump could not read the cancellation until the call it meant to stop had finished
(GAPS §7), and a pump that is sequential all the way through reintroduces that bug
here. Data in order, control out of order — one queue per namespace, one immediate
branch for `close`.

**The namespace rule is a wall, not a convention**, and it is enforced at both
ends. On the server: a frame may only address an op in the namespace of a stream
that is **live** on this connection — the record leaves the registry when the pump
stops stating the stream, so "a stream ended" closes the namespace again rather
than leaving it open because it was opened once — and a client may only `close` a
stream it opened itself. The server compares the frame's `op` namespace against
the open-stream record for that `id` and refuses a mismatch by name. On the client: the generated
code delivers an `item` frame only to the feature whose namespace it names, so a
feature's code cannot listen to another feature's events even by accident. Two
features on one page therefore cannot read each other's events or write into each
other's channel, and the check is one comparison in one place — the registry of
open streams — not a policy each app configures.

**Every refusal names its kind.** A frame the server cannot honour is answered with
an `error` frame whose `ErrorKind` says what happened, because a client that
retries on one kind and alerts on another cannot recover from a wrong answer. The
surface sends exactly these:

| Situation | Kind |
| --- | --- |
| A frame that will not parse | `InvalidInput` |
| An op no mounted feature serves, on `open` or on `call` | `NotFound` |
| A `call` outside every namespace this connection opened | `Forbidden` |
| A `close` for a `StreamId` this connection never opened | `NotFound` |
| A frame only the server may send (`item`, `reply`, `end`, `error`) | `InvalidInput` |
| A second `open` under an id already live | `Conflict` |

The middle pair is the one worth keeping apart. An unknown op is `NotFound` on
every surface — HTTP, MCP, the CLI and here — and only a namespace the connection
never opened is `Forbidden`, the one kind that means "known caller, not allowed".
One kind in `pump::fail`'s signature, chosen by the site that knows what went
wrong, is what keeps those two from collapsing into one answer.

**Flow control, honestly.** One connection has one TCP write queue, so an `Each<T>`
stream whose subscriber is slow blocks the connection and everything else on it
waits. That is not solved here, and it is not hidden either:

- **The connection is the isolation unit, not the stream.** An app that must not let
  a slow `Each` consumer stall its other streams opens a second connection and puts
  it on that one. Nothing forbids a second connection; the namespace rule is
  enforced per connection.
- `Latest<T>` cannot stall the connection, because its pump holds one slot per
  stream and replaces an untaken value. The high-rate case — terminal frames — is
  `Latest`, so the case that motivated the feature is the case that cannot block
  anybody.
- What gabion will not do is bound a per-stream buffer and drop on overflow for a
  stream declared `Each`. That is `Latest` semantics smuggled into a declaration
  that says otherwise, which is the silent default the house rule bans. If a
  bounded overflow policy is ever wanted, it arrives as a third marker, spelled
  out in the signature.

What this costs, stated plainly:

- **OpenAPI cannot describe it.** There is no 3.1 spelling for a request-initiated
  substream multiplexed onto a shared WebSocket. Stream ops therefore get **no fake
  `paths` entry** — spec §5's own rule is that a contract cannot describe a route
  that is not served. They are listed in a root-level `x-gabion-streams` extension
  keyed by op, each carrying its policy and a `$ref` to the item
  schema in `components/schemas`. The item types stay codegen-able, and the
  committed document still fails its snapshot test when a stream op drifts.
- **The typed client needs a WebSocket client.** Native: `tokio-tungstenite`, a new
  dependency of `gabion-native`. Wasm: `web_sys::WebSocket`, which is `!Send` and is
  exactly what §4.2 pays for.
- **One endpoint, not one per op.** `GET /api/ws`, feature-gated on `server`,
  mounted once by `gabion_native::host_router` the way `/mcp` is mounted today.
  The upgrade is plain HTTP, so it works over loopback TCP and over a unix socket
  (`axum::serve` takes any listener); a raw pipe with no HTTP server on the far end
  is what the MCP stdio transport is for.

## 6. The prerequisite nobody named: a capability that opens a channel

`caps::Http::send` returns `HttpResponse { body: Vec<u8> }` — a whole body in
memory. The generated client is `<Feature>Client<H: Http>`, and it calls
`self.http.send(..)` then parses the complete body. So **no stream op can be
implemented by the generated client on today's capability set** — and the client
must implement the whole trait, because the whole point of decisions 10.1 and 10.5
is that a renderer cannot tell a remote impl from a local one.

So `Caps` gains a verb, the same way waiting and cross-service calls arrived
(GAPS §7: "there was no way to express waiting" → `Caps::Sleep`; "a core could not
call another service" → `Caps::Http`):

```rust
pub trait Streams: Clone + Send + Sync + 'static {
    /// Open the channel `request` names and hand back both directions.
    fn open(&self, request: HttpRequest) -> Result<Channel, StreamError>;
}

/// One opened channel: send frames out, take frames in, close it.
pub struct Channel { /* … */ }

impl Channel {
    /// Boxed future, the way `caps::Timer` is, so it can be held by a pump.
    pub fn send(&self, frame: Frame) -> SendFrame<'_>;
    pub fn incoming(&self) -> BoxStream<'static, Result<Frame, StreamError>>;
    pub fn close(&self);
}

pub trait Caps {
    // ...
    type Streams: Streams;
    fn streams(&self) -> &Self::Streams;
}
```

`Frame` here is gabion's own envelope (the `open`/`send`/`item`/`end`/`error`
shape), encoded as one JSON object per WebSocket text frame. The codec that turns
a channel into a typed `Stream<T>` — decode `item`, raise the typed error on
`error`, end on `end`, and treat a socket that dies without an `end` as a
transport failure — is one small module in `gabion`, separately testable, and it is
where §7.4 lives.

Impls: native over `tokio-tungstenite`; wasm over `web_sys::WebSocket`, which is
the `!Send` case §4.2 pays for; and **`MockStreams` in `TestCaps`, which is not
optional**:

> **S14 (new requirement).** A test capability bundle must be able to *script* a
> stream: a fixed item list, a scripted failure mid-stream, and a scripted stall.
> Without it, stream conformance is nondeterministic by construction, and GAPS §8's
> "cannot compare a create" hole becomes "cannot compare a subscription".

## 7. Semantics

The proposal's six, corrected and extended:

1. **Ordering is per stream.** Free: one writer per stream, and a WebSocket frame
   carries its own stream id, so interleaving is not a hazard. Two streams on one
   connection have no relative order, and nothing pretends otherwise.
2. **`Latest<T>` conflates.** The *surface's* pump holds one pending value per
   stream and replaces an untaken one. The core does not know it is conflating —
   which is why the policy belongs in the signature: it selects the loop the
   generator emits.
3. **`Each<T>` backpressures.** The pump awaits each write — and over a shared
   connection that backpressure reaches the connection, which is §5's honest limit,
   not a bug to fix quietly. A deadline on a stream that cannot progress is the
   app's, applied with `caps::timeout` (built today) or `Ctx` cancellation; gabion
   adds no deadline knob of its own. A stall is `Timeout` and a namespace
   violation (§5) is `Forbidden`; neither is reported as `Internal`, because S7 is
   about the client recovering a *named* kind. Each kind owns one HTTP status, one
   JSON-RPC code and one exit code (`gabion/src/error.rs`), so it round-trips through
   `WireError` and `FromWire` instead of being flattened at the boundary.
4. **End of stream is explicit, and the framing must prove it.** A stream ends with
   an `end` frame (`done` / `cancelled`) or an `error` frame carrying a `WireError`.
   **A channel that dies without one of the three is a transport failure, not a
   clean end.** Otherwise a dropped socket reads as a completed stream — a fail-open
   of exactly the class GAPS §1 bans, and the single most likely thing to get
   "solved" by an adapter that treats EOF as success.
5. **Either side may speak, inside its namespace.** The client sends by calling an
   ordinary op on the connection (`call` → `reply`); the server sends by pushing an
   `item`. Both land on the same `Ctx` as everything else. A client may not `call`
   outside the namespace of a stream it has open, and may not `close` a stream it
   did not open (§5).
6. **`Ctx` still governs.** Principal, trace, cancellation, filled per surface,
   unchanged.
7. **No replay, no persistence, no broker.** A typed async iterator over one
   connection.

And the handoff S8 describes has a race the proposal does not mention:
`let snap = get().await; let tail = watch().await;` loses everything produced in
between. The framework cannot fix it without a sequence number in the item, which
is the app's type. So the doc states the safe pattern — **subscribe first, then
take the snapshot, then drop items the snapshot already contains** — and notes
that `Latest<T>` is the shape where the race is harmless, which is a third reason
a frame stream is `Latest` and not `Each`.

## 8. S12 is already a wall

The proposal asks for a rule: a core may produce items but may not spawn the
pump. It already has one, and it is mechanical:

- `std::thread` / `std::thread::spawn` are in the `clippy.toml` ban list;
- `tokio`, `async-std`, `smol`, `async-executor`, `rayon` and `mio` are in
  `core-runtime-dep`'s list (`crates/gabion-cli/src/edges.rs`).

Every way a core can spawn a pump on this toolchain is already a named finding.
The rule to write down is the one that follows from it: **a core composes streams,
it does not source them.** A source arrives as a capability, or from another
service's op; the pump that drains a real device (a pty, a timer, a socket) lives
in an effect home — `gabion-native`, or a host — which is already a declared
class. `empty-core` and `no-wasm-proof` are unaffected: a core that returns
`Latest::new(map(rx, decode))` implements something and holds no task.

## 9. What each surface owes

| Feature | Delta |
|---|---|
| (default) client | Returns `Result<Latest<T>, E>`; opens a channel on `Caps::Streams` and decodes `item` frames into `T`; an `error` frame maps through `FromWire` like any other failure. Once a channel is open for its namespace, the client's other ops in that namespace go out as `call` frames and come back as `reply`. |
| `registry` | Nothing. A stream op mounts like any op. |
| `server` | One `GET /api/ws` upgrade per host, not one route per op. The frame envelope, the open-stream registry, and the namespace check live in the generated server module. |
| `server` + `openapi` | No `paths` entry (there is no route to describe). A root-level `x-gabion-streams` extension keyed by op: policy and a `$ref` to the item schema in `components/schemas`. The lock line already moves with the marker (§4.3). |
| `mcp` (stdio) | One `notifications/progress` per item, then the final result. This needs `Server::handle` to take a sink: today it is `handle(&self, request: &str) -> Option<String>` — one request in, at most one response out — so it is a signature change that reaches both transports. Note the existing cancellation channel is *inbound*; streaming needs *outbound* notifications, which the current shape cannot express at all. |
| `mcp` (`POST /mcp`) | **Refused at mount, with a named reason.** v1 has no SSE (spec §5) and one JSON answer cannot carry N items. This is the one place "every method reaches every surface" gets a written exception, and it must be a refusal an agent can read rather than a tool that returns something strange. |
| `cli` | NDJSON on stdout, one item per line; exit code from the `end` or `error` frame. |
| `conformance` | Stream cases collect items + termination + error and compare them across adapters, driven from `MockStreams`. A listen op that fails before its first item is a case like any other, and every surface answers it as `{items: [], end: <error>}`: the MCP surface cannot answer a stream op with a bare value, so a bare error on one surface and a wrapped one on another is a disagreement about nothing. |

## 10. Requirements: added, kept with a limit, and free

| | |
|---|---|
| **Added S14** | A test bundle can script a stream (§6). Without it the conformance suite cannot drive a stream at all. |
| **Kept S3, with a limit** | Many streams on one connection, separated by feature namespace. The isolation unit is the connection: a slow `Each` consumer blocks everything sharing its socket, and an app that cares puts it on a second one (§5). |
| **Satisfied S13** | A `close` frame ends one stream without ending the connection, and a dropped socket ends the handler and every producer in it — in both directions, which is what full duplex buys here. The unary HTTP cancellation gap (GAPS §4) is untouched and is not a stream problem. |
| **Free S12** | Already enforced by the effect ban (§8). |

## 11. Stages, and why the refusal stays until stage 3

**Stage 0 — close the hole the decision opens.** `is_streaming` in
`gabion-macros/src/lib.rs` token-scans for `impl` and `Stream`, so
`Latest<Frame>` and `Each<T>` walk straight through it today and would be handed
to schemars as value types: an E0277 out of generated code, which is the exact
failure the refusal exists to prevent. So before anything else, the refusal names
the markers as reserved-but-unbuilt. Land the static fixtures here too.

**Stage 1 — the vocabulary.** `gabion::stream` (`Stream`, `BoxStream`, `Each`,
`Latest`, the frame envelope, the end rule), the wasm `?Send` split in the macro,
and the wasm proof. **This is all collab needs**, so it ships alone and is useful
the day it lands.

**Stage 2 — the capability.** `Caps::Streams`, native over `tokio-tungstenite`, web
over `web_sys::WebSocket`, and `MockStreams`.

**Stage 3 — the WebSocket.** `GET /api/ws`, the open-stream registry, the namespace
check, the per-namespace `call` queue, the generated client's channel, and the
`x-gabion-streams` OpenAPI extension. The refusal lifts here and not before: a
stream op the generated client cannot implement is the same fail-open class again.
Stages 1–3 are one unit of user-visible delivery.

**Stage 4 — CLI.** NDJSON on stdout.

**Stage 5 — MCP.** `handle` gains a sink, stdio emits progress, `POST /mcp`
refuses by name.

**Stage 6 — conformance.** Stream cases through every built adapter, from the
scripted source.

## 12. Tests that must land with it

The proposal's list, corrected. Style: fixtures in
`crates/gabion-cli/tests/rules.rs`, macro cases in `gabion-macros/src/tests.rs`.

| Fixture | Proves |
|---|---|
| `a_stream_marker_that_is_not_each_or_latest_is_a_hard_error` | There is no default policy, because there is no unmarked stream shape. Replaces "a stream op without a policy is a finding". |
| `the_lock_moves_when_a_policy_flips` | §4.3. The reason the policy is a type. Stage 0. |
| `a_stream_op_missing_from_the_lock_is_a_finding` | Already nearly free once the marker is in the signature. |
| `a_stream_that_dies_without_an_end_frame_is_a_transport_failure` | §7.4. The fail-open this design would otherwise ship. |
| `a_stream_error_round_trips_to_a_typed_error_kind` | S7, through the `error` frame and `WireError`. |
| `stream_item_order_survives_all_adapters` | S2, per stream, per adapter. |
| `a_conflating_stream_keeps_the_last_value` | S5, and that the newest is never dropped. |
| `a_lossless_stream_blocks_instead_of_growing_without_bound` | S4 — driven by `MockStreams` with a stalled subscriber, so it is deterministic and spends no wall time. |
| `a_frame_in_another_features_namespace_is_refused` | The namespace wall (§5), server side, with a named kind. |
| `a_feature_never_receives_another_features_frames` | The same wall on the client: delivery is by namespace, not by whoever happens to be polling. |
| `a_client_may_not_close_a_stream_it_did_not_open` | The other half of the same wall. |
| `two_sends_keep_their_order_on_one_connection` | The per-namespace ordering rule (§5) — a terminal cannot have its keystrokes overtake each other. |
| `a_close_is_handled_while_a_send_is_in_flight` | Control out of band, data in order. The bug gabion's MCP pump had to be rebuilt to avoid (GAPS §7). |
| `a_close_frame_ends_one_stream_and_leaves_the_others_up` | S6, and S3: one connection, many streams, independent endings. |
| `a_dropped_socket_ends_the_producer_even_while_it_is_idle` | S13. The read side notices, so an idle producer is cancelled too — the case a streaming HTTP response could not cover. |
| `a_core_that_streams_still_fails_the_effect_ban` | S12. Cheap, and belongs in stage 0. |
| `every_stream_op_is_driven_through_every_built_adapter` | With the `POST /mcp` exception written into the suite, so "a case it cannot drive is a rejection" does not get quietly bent. |
| `a_wasm_host_consumes_a_stream` | S11, and it is the test that forces §4.2. |

`every_rule_bites` and `dogfood.rs` then have to pass with a stream op in one of
the examples — which means one of the example features grows a stream, and that
is the real cost of the whole exercise: the framework's own examples have to
carry a stream before the wall is believed.

## 13. What this does not buy

Agreed with the proposal's §9, and extended: no broker, no persistence, no replay
log, no cross-process fan-out, no at-least-once, no subscription registry — and no
per-stream flow control of gabion's own (§5). Also not in v1: an **inbound stream
argument** (a marker in an argument position, so one op carries both directions).
`listen` + `send` covers what has been asked for; if a backpressured inbound stream
is ever genuinely needed, it arrives as that escalation and reopens §3.
The frame envelope stays a handful of fields over JSON text frames; the moment it
grows a window size, a resume token, or a priority, it has become a transport
project and should come back as its own proposal.

## 14. What changes for the consumer

The ask is accepted in v1, including the WebSocket. Two things come out different
from the shape that was proposed: the policy is in the return type rather than an
attribute (§4.3 — the reason is the lock, and it is not negotiable), and MCP
carries streams over stdio only.

Full duplex is in, as the pair the proposal named: `watch()` to listen, an ordinary
op to send, both in one feature namespace, both on one connection. A feature's
other ops travel as `call` frames on that connection once it is open, in arrival
order per namespace, so keystrokes cannot overtake each other. Frames are
namespaced by feature, and a client may only open, call and close inside the
namespace of a stream it has open.

For S9: loopback TCP and a unix socket are both just the serve path — the upgrade
runs over any listener. A raw SSH stdio channel with no HTTP server on the far end
is what MCP-over-stdio is for (stage 5); a WebSocket over SSH means forwarding a
port. gabion will not invent a second framing for one pipe.

The thing to design before stage 3 is the item type: if the snapshot-then-tail
handoff of S8 is to be safe, `Frame` has to carry something orderable (§7), and
that is the app's type, not the framework's.

## 15. What was checked in the code

- `crates/gabion-macros/src/lib.rs` — the refusal, `is_streaming`'s token scan, and
  `reject_retired_surface_attrs`.
- `crates/gabion-cli/src/surfaces/mod.rs` — the lock hashes `f.sig` only, so
  attributes are invisible to it (§4.3).
- `crates/gabion/src/caps.rs` and `crates/gabion-macros/src/client.rs` — `Http` is
  request/response, and the client parses a whole body (§6).
- `crates/gabion/src/mcp/server.rs` — `handle(&str) -> Option<String>`, and the
  in-flight registry that makes inbound cancellation work (§9).
- `crates/gabion/src/error.rs` — six `ErrorKind` variants (§7.3).
- `crates/gabion-cli/src/edges.rs` — `tokio`, `async-std`, `smol` are already
  banned in a core (§8).
- `docs/local-first-collab.md` §4.2, §4.4, §7 — the stream there is on the
  `Collab` capability, and the WS route is collab's own sync transport (§2).
