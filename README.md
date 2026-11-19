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
#[service]
pub trait TodoService {
    #[surface(cli, http)]
    async fn create(&self, input: CreateTodo) -> Result<Todo, TodoErr>;

    #[internal] // explicit opt-out; every fn must choose
    async fn recompute_stats(&self, _input: ()) -> Result<(), TodoErr>;
}
```

`#[service]` generates (feature-gated): an axum router (`POST /todo/create`),
clap subcommands (`myapp todo create --json '{...}'`), an OpenAPI-ready envelope,
and (v2: rmcp tools + typed client). One error enum maps to HTTP status, MCP
error code, and CLI exit code — one mapping, everywhere.

## Layout

```
crates/gabion/          runtime: capability traits, error model, testing caps
crates/gabion-macros/   #[service]
crates/gabion-native/   native caps: SystemClock, FileStore (the effect home)
example/features/todo/  todo-api | todo-core | todo-render  (the colocation unit)
example/hosts/          host-native (CLI+ui) | host-server (axum) | host-web (wasm)
```

## Try it

```sh
cargo build
MYAPP_STORE=/tmp/s.json ./target/debug/myapp todo create --json '{"title":"buy sandspice"}'
MYAPP_STORE=/tmp/s.json ./target/debug/myapp todo list
MYAPP_STORE=/tmp/s.json ./target/debug/myapp ui          # render crate, in-process
./target/debug/myapp-server                              # POST http://localhost:3456/todo/create
cargo build -p todo-core --target wasm32-unknown-unknown # the wall proof
cargo clippy --workspace                                 # the effect ban
```

## Status

v0 spike: spec §11 experiments 1–2 done and verified (walls bite; one fn → CLI +
HTTP). Next: rmcp tools, generated typed client (`InProcess` + `Http` impls),
`fw check` driver, `fw new` scaffolding. See `docs/framework-spec.md` (+ `.html`).
