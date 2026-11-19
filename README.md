# gabion

A civil-engineering-inspired Rust framework for single-binary apps: **one
business core, four surfaces** (CLI, HTTP, MCP, typed client), natively and in
the browser as wasm — with architectural shortcuts *mechanically sealed*.

Named for [gabion walls](https://en.wikipedia.org/wiki/Gabion): cages of rocks
that hold back a slope while letting water drain through. Slopes fail by
taking shortcuts — so do codebases (the Grok-Bot-client lesson: agents take
shortcuts, so the shortest path must be the correct path). Gabions don't rely
on anyone remembering to maintain them; the geometry does the work. Here the
geometry is the crate graph, compile targets, and lint gates — enforcement is
mechanical, never a standard the author has to remember. The wall holds back
the slope; the drains (api DTOs, trait objects) still flow.

Today two of the four surfaces are built (CLI, HTTP); MCP and the typed client
are the next two. The gate that seals the walls is built, tested, and runs in
CI.

## The two walls (both proven in this repo)

1. **Process wall** — `*-render` crates import `*-api` only. Importing `*-core`
   is unrepresentable: it's not in `[dependencies]`, so `use todo_core::...`
   fails with E0432. No lint can forget.
2. **Feature wall** — feature A may depend on feature B's `*-api` only, never
   B's internals. Cross-feature calls go through the generated client.

And the effect ban: `*-core` is runtime-free and wasm-safe. `std::fs/net/thread`
don't exist on wasm (compile fails); `std::time` *does* compile on wasm and
panics at runtime, so `clippy::disallowed_types` deny (see `clippy.toml`) seals
it — verified: injecting `SystemTime::now()` into todo-core fails `cargo clippy`
with "use of a disallowed type".

## One trait, four surfaces

```rust
// The trait IS the boundary. Every method in it is generated into every
// surface — CLI subcommand, HTTP route, MCP tool, typed-client method — with
// no per-method opt-in or opt-out.
#[service]
pub trait TodoService {
    async fn create(&self, input: CreateTodo) -> Result<Todo, TodoErr>;
    async fn list(&self, _input: ()) -> Result<Vec<Todo>, TodoErr>;
}

// An op that must not be remote is not in this trait. Plain trait in the same
// api crate: another feature can call it in-proc, no surface can reach it.
pub trait TodoInternals {
    async fn recompute_stats(&self) -> Result<(), TodoErr>;
}

// And purely-local internals skip the api crate entirely — inherent methods on
// the impl. Adapters only hold `Arc<dyn TodoService>`, so they cannot see them.
```

There are no surface attributes because there is nothing to configure: the
shortest path is the correct path, and the reviewable artifact is the *set* of
reachable ops, locked in `.gabion/surfaces.lock` so publishing a new one shows
up as a diff in the PR.

`#[service]` generates (feature-gated): an axum router (`POST /todo/create`),
clap subcommands (`myapp todo create --json '{...}'`), an OpenAPI-ready envelope,
and (v2: rmcp tools + typed client). One error enum maps to HTTP status, MCP
error code, and CLI exit code — one mapping, everywhere.

## The gate

```sh
cargo run -p gabion-cli -- check     # one command, one exit code
```

Green means the architecture is correct. The gate orchestrates the standard
tools and adds only the rules that need to know gabion's shape — it does not
reimplement a linter:

| step | what it enforces |
|---|---|
| `fmt` `clippy` `test` | rustfmt; clippy with the effect ban and complexity caps; the test suite |
| `shear` `deny` | unused dependencies; licenses, advisories, sources |
| `wasm` | every `*-core` crate builds for `wasm32-unknown-unknown` |
| `edges` | the dependency edge table — process wall, feature wall, contract purity, no upward leaks |
| `clippy.toml` | the ban list and thresholds are still present; a deleted wall is a named failure |
| `surfaces` | `.gabion/surfaces.lock` matches the set of reachable ops — publishing a new one is a reviewable diff |
| `sources` | no comments in `*-core` bodies, no silent defaults, hosts stay composition roots, `#[allow]` only where declared, branch and file caps, no TODOs |

The edge table is data, not code — an app tunes it in `Cargo.toml`:

```toml
[workspace.metadata.gabion.edges]
render = ["framework", "shared", "api"]   # render may never reach core
```

Crates that legitimately perform effects (a native host, a `*-native` adapter,
the CLI itself) are declared in `[workspace.metadata.gabion] effect-homes`, and
the `#![allow(clippy::disallowed_*)]` that grants them is audited: an `#[allow]`
outside a declared effect home is itself a finding.

Every rule has a failing fixture in `crates/gabion-cli/tests/`, and
`dogfood.rs` runs the gate against this repo — a wall that stops biting fails
here first.

## Layout

```
crates/gabion/          runtime: capability traits, error model, testing caps
crates/gabion-macros/   #[service]
crates/gabion-native/   native caps: SystemClock, FileStore (a declared effect home)
crates/gabion-cli/      `gabion check`: the gate
example/features/todo/  todo-api | todo-core | todo-render  (the colocation unit)
example/hosts/          host-native (CLI+ui) | host-server (axum) | host-web (wasm)
```

## Try it

```sh
cargo build
export MYAPP_STORE=/tmp/s.json                       # default: ./myapp.store.json
./target/debug/myapp todo create --json '{"title":"buy sandspice"}'
./target/debug/myapp todo list
./target/debug/myapp ui                              # render crate, in-process
./target/debug/myapp-server                          # POST http://localhost:3456/todo/create
cargo build -p todo-core --target wasm32-unknown-unknown   # the wall proof
cargo run -p gabion-cli -- check                          # the whole gate
```

## Status

v0.1 skeleton, working end to end. The todo example runs as a native binary
(CLI, file-backed store) and as an HTTP server, and `todo-core` compiles clean
to `wasm32-unknown-unknown`. `host-web` compiles for wasm but has no bundler
config yet, so treat it as compile-proof, not a shipped browser app.

**Built:** `#[service]` codegen (clap + axum), the capability and error model,
`gabion check` with tests proving each rule bites, CI, and the effect ban across
threads / env / process / fs / net.

**Not built yet:** the MCP surface, the generated typed client, `gabion new`
scaffolding, per-call `Ctx`, and error round-tripping over the wire
(`WireError`). See [docs/GAPS.md](docs/GAPS.md) for the full inspection log —
including the places a wall is claimed but not yet enforced — and
`docs/framework-spec.md` for the signed-off decisions behind each.

MIT or Apache-2.0 dual licensed.
