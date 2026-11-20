# gabion

**A cross-platform Rust app framework with a wall built into it.**

You write one business core. It runs natively, in the browser as WebAssembly
(wasm), and on a server. gabion generates the surfaces from that core, and it
refuses to build the shortcuts.

gabion is named for [gabion walls](https://en.wikipedia.org/wiki/Gabion): cages
of rocks that hold back a slope while water drains through. A slope fails where
it finds a shortcut. A codebase fails the same way. The cages never rely on
someone remembering to maintain them. The geometry does the work. Here the
geometry is the crate graph, the compile targets, and the static gates.

## The problem: agents take the shortest path

"Slop" means code that satisfies the request in front of it and breaks a rule
nobody restated in the prompt. An agent does not damage your architecture out of
malice. It takes the shortest path that compiles. So the shortest path must be
the correct path.

A style guide cannot enforce that. A reviewer cannot enforce it at the rate an
agent produces code. Only a mechanical block works, and it has to be stricter
than a linter, because the failure gabion targets is not a typo. It is a
plausible shortcut that passes every standard check.

gabion builds that block in three layers. Each layer catches what the layer above
it cannot.

## Layer 1: the crate graph makes the wrong import unrepresentable

Every feature ships as three crates, not three modules:

```
features/todo/
├── todo-api/       # wire types + the #[service] trait. Compiles everywhere.
├── todo-core/      # the implementation. Pure. No tokio, no std::fs/net/time.
└── todo-render/    # the UI. Depends on todo-api (and other features' render
                    # crates, when composing two features on one screen).
```

Cargo only lets a crate `use` what is in its `[dependencies]`. So `todo-render`
cannot import `todo-core`. The import does not exist. There is no rule to
forget, no lint to disable, and no reviewer to catch it. The compiler says E0432.

One edge is deliberately open: **`render -> render`**. Composing two features on
one screen is the normal case, and a feature's UI crate is a library of
components, so the composition happens at the UI layer instead of being pushed
into a shell crate that owns nothing. What keeps that from becoming "reach into
another feature's private components" is `over-broad-visibility`, which holds a
`*-render` to the same standard as a `*-core`: a `pub` item is a component other
crates may name, and anything else is `pub(crate)`. A cycle between two render
crates is still a cargo error, so no crate can sit both below and above another.

The same shape gives the second wall: feature A may depend on feature B's
`*-api`, never on B's `*-core` or `*-render`. Cross-feature calls go through the
other feature's contract — `incidents-core` calls `CheckService`, the trait the
HTTP surface also serializes — and the generated client for that trait implements
the same trait over `Http`, so the same call works in-process or across a socket
with no change at the call site.

DTOs live in `*-api`, not `*-core`. Put one in core and the render crate fails at
the wall immediately. The error message is the documentation.

## Layer 2: the effect ban, because the target is not enough

`*-core` crates compile for `wasm32-unknown-unknown`, and the target catches a
crate that genuinely cannot build there (tokio's `net` feature, C-linked
libraries). It does **not** catch effects: rust's `std` ships wasm stubs for
`std::fs`, `std::net`, `std::thread`, and `std::time`, so a core that opens a
file, binds a socket, spawns a thread, or reads the clock still compiles and only
fails when it runs. tokio's `rt` feature builds for wasm too.

So the effect wall is mechanical in two other places, and the gate audits both:

- `clippy.toml` enumerates the banned paths — `std::fs::*`, `std::net::*`,
  `std::thread::*`, `std::time`, `std::env`, `std::process` — through
  `disallowed_types` and `disallowed_methods`. `clippy-config` fails when an
  entry is deleted.
- `core-runtime-dep` reads every `*-core` manifest and fails if it names a
  runtime, server, client, or storage engine (`tokio`, `axum`, `reqwest`,
  `sqlx`, …). Nothing else would catch that one.

The list is enumerated, so it is a wall with edges: a shortcut outside it is not
caught, and `docs/GAPS.md` says so.

Effects are not forbidden in the app. They move. Core code asks for a capability
(`Clock`, `Store`, `Sleep`, `Http`), and the host injects a per-target
implementation. A native host wires a real clock, a file store, a tokio timer,
and a rustls http client. A test wires a virtual clock that moves only when code
asks to wait, an in-memory store, and scripted http. Core code never sees the
difference.

## Layer 3: `gabion check` closes the rest, and audits itself

Cargo's wall holds only while the dependency stays out of `[dependencies]`. One
added line removes it. Clippy's wall holds only while `clippy.toml` keeps its
bans. So the gate audits the configuration files, not just the source.

```sh
cargo run -p gabion-cli -- check     # one command, one exit code
```

Green means the architecture is correct. The gate runs the tools that already
exist, then adds only the rules that need to know gabion's shape. It does not
reimplement a linter.

| Step | What it enforces |
| --- | --- |
| `fmt`, `clippy`, `test` | rustfmt. clippy with the effect ban, `unwrap_used` and `expect_used` deny, and complexity caps. The test suite, with every feature on, so a feature-gated test cannot be skipped into a vacuous green. |
| `shear`, `deny` | Unused dependencies. Licenses, advisories, and sources. |
| `wasm` | Every `*-core` crate builds for `wasm32-unknown-unknown`, per feature. This catches crates that genuinely cannot build there; the effect wall is the clippy ban list plus `core-runtime-dep`. |
| `edges` | Every `Cargo.toml` against the edge table: process wall, feature wall, contract purity, no upward leak, no host dependency. |
| `clippy.toml` | The required bans and thresholds are still present. A deleted wall is a named failure. |
| `workspace lints` | `[workspace.lints]` still forbids unsafe and denies the clippy wall, and every member still opts into it. |
| `surfaces` | `.gabion/surfaces.lock` matches the set of reachable operations. Publishing a new one is a reviewable diff. |
| `sources` | No comments in `*-core` bodies, no silent defaults, no glob re-export from a contract, hosts stay composition roots, `#[allow]` only where declared, branch and file caps, no TODOs. |

Two rules are the point of the whole design:

- **`clippy-config`** fails the build when someone loosens a threshold or deletes
  a ban instead of fixing the code. Its sibling **`workspace-lints`** does the same
  for `[workspace.lints]`, including a member that never opts in. The wall cannot
  be negotiated away in a one-line diff.
- **`effect-home-allow`** fails the build when an `#[allow]` for a gabion-managed
  lint appears outside a declared effect home. The escape hatch stays visible and
  audited. One line in a named crate, not twelve scattered ones.

Every rule has a failing fixture in `crates/gabion-cli/tests/`. The test
`every_rule_bites` builds a workspace that violates the rulebook all at once and
asserts the static rules fire; individual tests cover the rest. `dogfood.rs` runs
the static gate against this repo, so a wall that stops biting fails here first.

## What a violation looks like

Add `todo-core` to `todo-render` and run the gate:

```text
FAIL process-wall
  at:     todo-render -> todo-core
  what:   todo-render (render) may not depend on todo-core (core)
  fix:    the render crate may name the service *trait* (from *-api) only; the
          host injects the impl. Never depend on *-core.

gabion check: 1 rule violation(s), 0 failing step(s)
```

Every finding carries a rule id, the location, and the correct fix rather than a
suppression. An agent with zero context can satisfy the gate without reading
this file.

## The rulebook

Each row names a shortcut and the block. The rule id is what the gate prints.

| Shortcut | Block |
| --- | --- |
| A render crate imports a core crate | Crate graph, then `process-wall` |
| Feature A imports feature B's internals | Crate graph, then `feature-wall` |
| A hand-edited `Cargo.toml` sneaks a forbidden dependency | `edges` audit of every manifest |
| `*-shared` depends on a feature (upward leak) | `upward-leak` |
| A `*-api` crate depends on a core or render crate | `contract-purity` |
| A host crate implements a service trait | `host-business-logic` |
| A workspace member gabion cannot classify | `unclassified-crate` |
| Composing two features on one screen | Allowed: `render -> render`. The gate's job is the *surface*: a `pub` item in a render crate nothing outside names is `over-broad-visibility`, so composition means "use that feature's components", not "reach into its internals". |
| A core crate imports a render crate | `core-imports-render` |
| The gabion runtime depends on an app feature | `framework-purity` |
| A crate depends on a host | `host-dependency` |
| `std::fs`, `std::net`, `std::thread`, `std::time`, `std::env`, `std::process` in `*-core` | `disallowed_types` / `disallowed_methods` deny. The wasm target does not catch these. |
| `tokio`, `axum`, `reqwest`, `sqlx` in a `*-core` manifest | `core-runtime-dep` |
| A `pub use` glob in an `*-api` contract | `contract-glob` |
| `unwrap()` or `expect()` | `unwrap_used`, `expect_used` deny |
| `unwrap_or_default()` in `*-api` or `*-core` | `core-silent-default`. A serialization failure must not become an empty string. |
| A comment inside a `*-core` function body | `no-comments-in-service-bodies`. Name the concept and extract it. |
| A method added to a service trait without anyone reviewing it | `surface-snapshot`. The reachable set is locked; a new op is a diff. |
| A `#[service]` trait declared outside `*-api` | `service-outside-api`. The macro generates a route and a subcommand wherever the trait sits, so the lock scans every crate. |
| A contract no host mounts | `unserved-contract`. `gabion::registry` cannot see an absent call, so the gate checks that a host crate names the trait. |
| `pub` in a `*-core` that nothing names | `over-broad-visibility`, and `--fix` narrows it to `pub(crate)`. |
| A hand-written axum handler in a feature | `axum-outside-host`. The HTTP surface is generated; a hand-written route is one the OpenAPI document, the MCP tool and the client do not know about. |
| A workspace with no `*-core` | `no-wasm-proof`. The wasm step would otherwise pass having built nothing. |
| A `*-core` that implements nothing | `empty-core`. A placeholder core makes the wasm build green about a wall nothing stands behind. |
| A streaming return in a `#[service]` trait | A hard compile error: no surface carries a stream in v1, and generating them anyway produced an E0038 from code the author did not write. |
| A changed input or output type on an existing op | `surface-snapshot`, whose lines carry a hash of the signature. |
| A TODO in a test file, or a 900-line test | `no-todo`, `file-length`, `too-many-branches` — `tests/` is scanned by the shape rules. |
| A `#[surface(..)]` or `#[internal]` left in a contract | The macro refuses to compile. Per-method surface attributes are gone. |
| `#[allow]` smuggled into a pure crate | `allow-allowlist`, `effect-home-allow` |
| A ban or threshold deleted from `clippy.toml` | `clippy-config` |
| A long file, a deep function, a wide signature | `file-length` 400, `too-many-branches` 10, cognitive complexity 15, 80 lines, 6 arguments |
| A `TODO`, `FIXME`, or `XXX` | `no-todo` |
| Unused dependencies, bad licenses, advisories | `cargo shear`, `cargo deny`. A missing tool is a named failure, or a named SKIP with `--allow-missing-tools` — never silence. |

## One core, every surface

The design constraint on the product side: you declare a service once, and every
surface comes from that declaration. No hand-written handler drifts from the
contract.

**The trait is the boundary.** Every method in a `#[service]` trait is generated
into every surface, with no per-method opt-in or opt-out. There is no attribute
to forget and no argument list for the macro to quietly ignore.

```rust
// features/todo/todo-api/src/lib.rs
use gabion::prelude::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTodo { pub title: String }

pub type TodoErr = ServiceError<TodoError>;

#[service]
pub trait TodoService {
    async fn create(&self, input: CreateTodo) -> Result<Todo, TodoErr>;
    async fn list(&self, _input: ()) -> Result<Vec<Todo>, TodoErr>;
}

// An op that must not be remote is not in this trait. A plain trait here: another
// feature can call it in-process, no surface can reach it.
#[async_trait]
pub trait TodoInternals {
    async fn recompute_stats(&self) -> Result<(), TodoErr>;
}

// And purely-local internals skip the api crate entirely: inherent methods on
// the impl in todo-core. Adapters hold Arc<dyn TodoService>, so they cannot see
// them. Structure, not lint.
```

Because there is no annotation to review, the reviewable artifact is the *set*:
`gabion check` locks every reachable operation in `.gabion/surfaces.lock`, so
adding a method fails the build until you run `--update-surfaces` and commit it.
Under this framework's own premise — agents take the shortest path — publishing a
new callable op should never be a silent event. Each line also carries a hash of
the operation's signature and the types it names, so a method that keeps its name
and changes its input type is a diff too.

A second artifact covers the shape a caller actually parses: `#[service]` emits an
OpenAPI 3.1 document built from the same types the router deserializes, the example
commits it to `.gabion/openapi/todo.json`, and a test fails when the code and the
committed document disagree.

The UI gets the same treatment, because a component can keep its props and change
what it shows: `.gabion/ui/` holds a committed snapshot per published panel
(`todo-list.txt`, `todo-panel.html`, `board-panel.html`), rendered from fixture
data and re-blessed with `GABION_BLESS_UI=1 cargo test`. A markup change is then a
diff someone reads rather than a surprise on someone's screen.

Every method takes the caller: `ctx: &Ctx` (principal, tenant, trace, cancellation)
is the first parameter, and each surface fills it in — HTTP from headers, MCP from
`_meta`, the CLI from `--principal` / `--tenant` / `--trace`, the client by
stamping the same headers the route reads. Authorization is representable because
the contract can see who is calling.

`#[service]` generates, behind cargo features, so each host compiles only what
it needs:

| Feature | Generated artifact |
| --- | --- |
| (default) | `<Feature>Client<H: Http>`: the service trait implemented over `Http`, so a renderer cannot tell a remote impl from a local one. The in-process impl *is* the trait object, so it needs no generated code. |
| `registry` (default) | `mount_on` on the trait object: a host mounts an impl and gets the routes, the subcommand and the tools |
| `server` | an axum router, one POST per exposed method. Errors map to HTTP status. |
| `server` + `openapi` | an OpenAPI 3.1 document for exactly those routes |
| `mcp` | one JSON-RPC tool per method, with an inlined input schema |
| `cli` | a clap subcommand tree and async dispatch. Errors map to exit codes. |
| `conformance` | `<ns>_conformance(svc, http, base, cases)`: one case driven through all four adapters and compared |

Wire shape: one POST per method, dotted under `/api` (`POST /api/todo.create`),
args as the body, the bare return type on `200`, and a flat `WireError` with the
status on failure. Caller identity travels as `x-gabion-principal` /
`-tenant` / `-trace` headers, and as `params._meta` over MCP.

MCP is served by `gabion::mcp` — JSON-RPC 2.0 over stdio (`gabion-native`'s pump)
or `POST /mcp` — rather than by an SDK: the three methods this framework uses are
the three it has to test, and the error mapping is the interesting part. `v1` has
no SSE stream; a tool call answers once.

One error enum maps to an HTTP status, an MCP error code, and a CLI exit code.
`ErrorKind::NotFound` gives 404, -32001, and exit 3 from one match arm in
`crates/gabion/src/error.rs`. The CLI's failure carries the same flat `WireError`
the route sends, which is what lets the generated conformance suite compare the
four surfaces and fail when one of them drops the kind or the detail.

## Two examples

`example/features/todo/` is the minimal shape: one feature, three crates, every
built surface. Everything the docs quote is from it.

`example/features/pulse/` is the same walls under load — a service monitor with
two features that talk to each other:

```
example/features/checks/      checks-api | checks-core    watch URLs, probe them
example/features/incidents/   incidents-api | incidents-core   open and close outages
example/features/pulse/       pulse-render                the status board
example/hosts/host-pulse-{cli,server,native,web}/        four hosts, one core
```

What it proves that the todo example cannot:

- **A feature calls a feature through its contract.** `incidents-core` computes
  uptime and outages from probe history, and its only window on the probes is
  `checks-api`. `checks-core` is not a dependency, so the wall is not a
  convention: the call the feature makes is the call the render crate makes.
- **The wait capabilities do real work.** A probe is three tries with a doubling
  wait and a deadline. Under `TestCaps` the ladder runs at full speed on a
  virtual clock and a test asserts the backoff was 250ms then 500ms; under
  `NativeCaps` the same code calls a real socket. One test file probes a live
  axum route, a route that returns 503, and a port nobody listens on.
- **Four hosts, one core.** `pulse` (CLI), `pulse-server` (axum, both routers,
  `/mcp`, plus `/openapi.json`), `pulse-gui` (a Dioxus desktop window), and the
  wasm status board all drive the same two impls. The spec's *four surfaces* are
  CLI, HTTP, MCP and the typed client; `host-pulse-server/tests/conformance.rs`
  drives one case through all four and compares them.
- **The gate's own gaps are closed.** `over-broad-visibility`, `axum-outside-host`,
  `unserved-contract`, `no-wasm-proof`, `--fix` for mechanical findings, missing
  tools reported as a named SKIP, and `tests/` scanned by the shape rules.
- **UIs compose and look alike.** `render -> render` is allowed, so two features can
  share a screen; the wall moved to `over-broad-visibility`, which holds a render
  crate to "`pub` means a component another feature may mount". Panels take props
  (data from `*-api` plus callbacks), not service handles; `example/shared/ui-shared`
  holds the tokens and the shared panel states, so two panels on one page look like
  one app; and each published panel has a committed HTML snapshot.
- **The board is playable, not a screenshot.** Add a watch, sweep what is due,
  probe one now, evaluate outages, and watch uptime move — every button is a
  call on one of the two contracts, and the render crate still names no core.
- **Every wall is a passing rule, not a claim.** Adding these two features made
  `gabion check` fail on `surface-snapshot` until the new ops were reviewed and
  committed, and the feature wall, the effect ban, and the contract-purity rules
  all pass with the cross-feature edge in the graph.

## Quick start

Rust comes from rustup. `rust-toolchain.toml` pins stable and adds the wasm
target, clippy, and rustfmt automatically.

Starting a new app, with the walls already in it:

```sh
cargo run -p gabion-cli -- new myapp --dir /tmp/myapp --gabion-path "$PWD"
cd /tmp/myapp
cargo run -p gabion-cli -- check          # green before you write a line
cargo run -p gabion-cli -- feature add billing
cargo run -p gabion-cli -- feature remove billing   # refuses while a host uses it
cargo run -p gabion-cli -- dev            # watch, rebuild, restart
```

`gabion new` writes the workspace metadata, the edge table, `[workspace.lints]`,
the effect-ban `clippy.toml`, a toolchain pin, and a surface lock generated by the
gate's own collector. `gabion check --fix` applies the mechanical findings (a
narrowed `pub`, the lock, a missing lints opt-in, a deleted threshold) and leaves
the rest for a decision.

```sh
# The server host: builds with no system packages.
cargo build -p host-server
./target/debug/myapp-server          # listening on http://127.0.0.1:3456

curl -X POST localhost:3456/api/todo.create -d '{"title":"buy sandspice"}'
curl -X POST localhost:3456/api/todo.list

# The target check: business logic compiles for the browser.
cargo build -p todo-core --target wasm32-unknown-unknown

# The gate. Static rules only, no subprocesses.
cargo run -p gabion-cli -- check --fast
```

The native host adds a CLI and a desktop UI:

```sh
cargo build -p host-native           # needs pkg-config and GTK dev files
export MYAPP_STORE=/tmp/s.json       # default: ./myapp.store.json
./target/debug/myapp todo create --json '{"title":"buy sandspice"}'
./target/debug/myapp todo list
./target/debug/myapp ui              # the render crate, in-process
./target/debug/myapp gui             # a Dioxus desktop window
```

The monitor example runs on the same surfaces:

```sh
cargo run -p host-pulse-cli -- check add --json '{"name":"api","url":"https://example.com"}'
cargo run -p host-pulse-cli -- check sweep     # probe what is due, with retries
cargo run -p host-pulse-cli -- incident evaluate
cargo run -p host-pulse-server                 # both routers on :3457, plus /openapi.json

# The desktop board. First run seeds two demo watches (one answers, one cannot).
cargo run -p host-pulse-native                 # $PULSE_STORE, default ./pulse-gui.json

# The browser board: builds wasm, generates the js glue, serves on :3458.
./example/hosts/host-pulse-web/serve.sh        # open http://127.0.0.1:3458
```

The browser host is not a wasm-cleanliness proof any more: it wires `WebCaps` —
js clock, localStorage store, `crypto.getRandomValues`, `setTimeout`, `fetch` —
so the same `checks-core` and `incidents-core` run in the page, and the board
survives a reload. Two browser facts come with it: `fetch` is CORS-bound (a probe
target must allow the page's origin), and localStorage is per-origin, so the
board is private to wherever `serve.sh` is serving from. `serve.sh` downloads a
`wasm-bindgen` CLI matching `Cargo.lock` when the one on `PATH` does not match.

The full gate also runs `cargo shear` and `cargo deny`. Install them with
`cargo install cargo-shear cargo-deny`, or pass `--no-external` to skip them.

## Layout

```
crates/gabion/          runtime: capability traits, Ctx, error model, MCP, registry, test caps
crates/gabion-macros/   #[service]: cli, server, openapi, mcp, client, registry, conformance
crates/gabion-native/   native caps (SystemClock, FileStore with written-by sidecar,
                        TokioSleep, ReqwestHttp), the concurrent MCP stdio pump and
                        the axum host router
crates/gabion-cli/      gabion check (the gate) + new / feature add / dev
example/features/todo/  todo-api | todo-core | todo-render  (the colocation unit)
example/features/checks/     checks-api | checks-core     (pulse: watch URLs)
example/features/incidents/  incidents-api | incidents-core (pulse: outages, reads checks)
example/features/pulse/      pulse-render                 (pulse: the status board)
example/shared/ui-shared/    design tokens + the shared panel states
example/hosts/          host-native (CLI + UI) | host-server (axum) | host-web (wasm)
                        host-pulse-cli | host-pulse-server | host-pulse-native | host-pulse-web
docs/                   the spec, the inspection log, the collab proposal
```

## Tuning the wall for your app

The edge table is data, not code. An app widens or tightens its own walls in
`Cargo.toml` without patching the framework:

```toml
[workspace.metadata.gabion]
effect-homes = ["myapp-cli"]     # crates allowed to perform effects
max-file-lines = 400
max-branches = 10

[workspace.metadata.gabion.edges]
render = ["framework", "shared", "api"]   # render may never reach core
```

Gabion classifies a crate from its name and path, never from a hand-written
label. A name must end in `-api`, `-core`, `-render`, or `-shared`, start with
`host-` or end with `-host`, sit under a `hosts/` directory, or appear in
`framework-crates`. Anything else is an `unclassified-crate` finding, because an
unclassified crate slips past the whole edge table.

The required bans and clippy thresholds work the other way. An app may tighten
them through `max-thresholds`. The gate only ever tightens them: a value that
would raise a limit is ignored, and the raised `clippy.toml` is then a
`clippy-config` finding.

## Status

v0.1. The examples run end to end: the todo feature works as a native binary with a
file-backed store and as an HTTP server, `todo-core` compiles clean to wasm, and
the gate passes on this repo. The pulse monitor runs two features that call each
other through a contract, over CLI, HTTP, MCP, and a wasm board.

**Built:** the three-crate feature shape and its edge-table audit; `#[service]`
codegen for clap, axum, MCP (JSON-RPC 2.0 over stdio and HTTP), the OpenAPI 3.1
document, and a typed client that *is* the service trait over `Http`; automatic
registration (`mount_on`) so a host names the impl and nothing else; per-call
`Ctx` on every surface; the capability model (`Clock`, `Store`, `Random`, `Sleep`,
`Http`, plus a `timeout` combinator) with native and deterministic test impls; an
error model that survives the wire (`WireError`, `FromWire`); the effect ban across
time, env, process, fs, net and threads (clippy's enumerated list, audited, plus
`core-runtime-dep`); the Cargo.toml wall (`unsafe_code = "forbid"`, every member
opted in); a generated four-adapter conformance suite; `gabion new` /
`gabion feature add` / `gabion dev`; `gabion check` with a failing fixture per rule
— including `over-broad-visibility`, `axum-outside-host`, `unserved-contract`,
`no-wasm-proof` and `--fix`; and CI on GitHub Actions.

**Not built:** streaming surfaces (the refusal is built, the feature is the spec's
open v1/v2 question); an HTTP cancellation channel (MCP has one); `gabion host add`;
a UI gallery; a renderer other than Dioxus; and any publish story — v0.1 is
path-dependency only, so `gabion new` takes `--gabion-path` and points at a
checkout.

**Claimed but with edges, and the doc says where:** the visibility check reads
names, so a mention in a comment counts as a use and it cannot prove a `pub` item
is wanted; `axum-outside-host` reads paths, so a re-export slips past it;
`unserved-contract` proves a host names a trait, not that it mounts it; the
conformance suite cannot compare a create, because two calls are two creates; the
lock's hash follows a signature's named types one level, so a nested struct is
hashed by name; the UI snapshots cover the view layer, not interaction or the async
containers; cancellation is offered to a tool, not imposed on it; and the Forgejo
workflow still has no runner, so GitHub Actions is what enforces the wall.

[docs/GAPS.md](docs/GAPS.md) is the full inspection log, ordered by how much
damage each silence does. The house rule there: a gap closes when a failing test
exists for it, not when the code looks better.

## Docs

- [docs/framework-spec.md](docs/framework-spec.md) — the signed-off decisions
  behind each wall, and the shortcuts each one seals.
- [docs/GAPS.md](docs/GAPS.md) — what the walls do not yet hold.
- [docs/local-first-collab.md](docs/local-first-collab.md) — a design proposal
  for optional local-first sync (rooms, presence, CRDT merge). Not implemented.

MIT licensed. See [LICENSE](LICENSE).
