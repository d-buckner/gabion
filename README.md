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
└── todo-render/    # the UI. Depends on todo-api only.
```

Cargo only lets a crate `use` what is in its `[dependencies]`. So `todo-render`
cannot import `todo-core`. The import does not exist. There is no rule to
forget, no lint to disable, and no reviewer to catch it. The compiler says E0432.

The same shape gives the second wall: feature A may depend on feature B's
`*-api`, never on B's `*-core` or `*-render`. Cross-feature calls go through the
generated client.

DTOs live in `*-api`, not `*-core`. Put one in core and the render crate fails at
the wall immediately. The error message is the documentation.

## Layer 2: the compile target bans the effect

`*-core` crates compile for `wasm32-unknown-unknown`. That target does the
strictest work for free:

- `std::fs`, `std::net`, and `std::thread` do not exist on wasm. The build fails.
- `std::time`, `std::env`, and `std::process` **do** compile on wasm and trap at
  runtime. The target cannot catch those. `clippy.toml` denies them through
  `disallowed_types` and `disallowed_methods`, and gabion audits that list.

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
| `wasm` | Every `*-core` crate builds for `wasm32-unknown-unknown`. This is the wall proof, run per feature. |
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
`every_rule_bites` builds a workspace that violates one of everything and asserts
each rule fires. `dogfood.rs` runs the static gate against this repo, so a wall
that stops biting fails here first.

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
| A host crate holds business logic | `host-business-logic` |
| A workspace member gabion cannot classify | `unclassified-crate` |
| `std::fs`, `std::net`, `std::thread` in `*-core` | The wasm build fails |
| `std::time`, `std::env`, `std::process` in `*-core` | `disallowed_types` / `disallowed_methods` deny |
| `unwrap()` or `expect()` | `unwrap_used`, `expect_used` deny |
| `unwrap_or_default()` in `*-api` or `*-core` | `core-silent-default`. A serialization failure must not become an empty string. |
| A comment inside a `*-core` function body | `no-comments-in-service-bodies`. Name the concept and extract it. |
| A method added to a service trait without anyone reviewing it | `surface-snapshot`. The reachable set is locked; a new op is a diff. |
| A `#[service]` trait declared outside `*-api` | `service-outside-api`. The macro generates a route and a subcommand wherever the trait sits, so the lock scans every crate. |
| A `#[surface(..)]` or `#[internal]` left in a contract | The macro refuses to compile. Per-method surface attributes are gone. |
| `#[allow]` smuggled into a pure crate | `allow-allowlist`, `effect-home-allow` |
| A ban or threshold deleted from `clippy.toml` | `clippy-config` |
| A long file, a deep function, a wide signature | `file-length` 400, `too-many-branches` 10, cognitive complexity 15, 80 lines, 6 arguments |
| A `TODO`, `FIXME`, or `XXX` | `no-todo` |
| Unused dependencies, bad licenses, advisories | `cargo shear`, `cargo deny` |

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
new callable op should never be a silent event.

The lock records names, so a method that keeps its name and changes its input type
would pass it. The shape is locked by a second artifact: `#[service]` also emits
an OpenAPI 3.1 document built from the same types the router deserializes, the
example commits it to `.gabion/openapi/todo.json`, and a test fails when the code
and the committed document disagree. A changed input type is a diff you read, not
a client that breaks in production.

`#[service]` generates, behind cargo features, so each host compiles only what
it needs:

| Feature | Generated artifact | Status |
| --- | --- | --- |
| `cli` | A clap subcommand tree and async dispatch. Errors map to exit codes. | built |
| `server` | An axum router, one POST per exposed method. Errors map to HTTP status. | built |
| `mcp` | rmcp tool definitions | not built |
| (default) | A typed client for the render crate | not built |

One error enum maps to an HTTP status, an MCP error code, and a CLI exit code.
`ErrorKind::NotFound` gives 404, -32001, and exit 3 from one match arm in
`crates/gabion/src/error.rs`. No adapter hand-maps anything. The kind survives
the boundary: the wire carries a flat `WireError` that a client can deserialize
and, if it knows the app's error enum, turn back into the typed error.

The renderer calls the same trait. Natively that is an in-process call. In the
browser it is an HTTP call. Same signature, same types, same errors.

## Quick start

Rust comes from rustup. `rust-toolchain.toml` pins stable and adds the wasm
target, clippy, and rustfmt automatically.

```sh
# The server host: builds with no system packages.
cargo build -p host-server
./target/debug/myapp-server          # listening on http://127.0.0.1:3456

curl -X POST localhost:3456/todo/create -d '{"title":"buy sandspice"}'
curl -X POST localhost:3456/todo/list

# The wall proof: business logic compiles for the browser.
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

The full gate also runs `cargo shear` and `cargo deny`. Install them with
`cargo install cargo-shear cargo-deny`, or pass `--no-external` to skip them.

## Layout

```
crates/gabion/          runtime: capability traits, error model, test caps
crates/gabion-macros/   #[service]
crates/gabion-native/   native caps: SystemClock, FileStore (a declared effect home)
crates/gabion-cli/      gabion check: the gate
example/features/todo/  todo-api | todo-core | todo-render  (the colocation unit)
example/hosts/          host-native (CLI + UI) | host-server (axum) | host-web (wasm)
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

v0.1. The example runs end to end today: the todo feature works as a native
binary with a file-backed store and as an HTTP server, `todo-core` compiles clean
to wasm, and the gate passes on this repo.

**Built:** the three-crate feature shape, the edge-table audit, `#[service]` codegen
for clap and axum, the capability model (`Clock`, `Store`, `Random`, `Sleep`,
`Http`, plus a `timeout` combinator) with native and deterministic test impls, an
error model that survives the wire (`WireError`), the effect ban across time, env,
process, fs, net, and threads, the Cargo.toml wall itself (`unsafe_code = "forbid"`
with every member opted in), an OpenAPI 3.1 document generated from the same trait
as the router and committed as a reviewable contract, `gabion check` with a failing
fixture per rule, and CI on GitHub Actions.

**Not built:** the MCP surface, the generated typed client, `gabion new`
scaffolding, per-call `Ctx` (so caller identity and authorization are not
representable yet), and the conformance suite that drives one impl through every
adapter.

**Claimed but not enforced yet:** the surface lock records method
names, not input types; the OpenAPI snapshot closes that for a service that builds
the document, and a service that leaves the `openapi` feature off is back to names
alone. The Forgejo workflow still has no runner; GitHub Actions
runs the gate on every push and pull request, and that is what enforces it.

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
