# One core, every surface

You declare a service once, and every surface comes from that declaration. No
hand-written handler drifts from the contract.

## The trait is the boundary

Every method in a `#[service]` trait is generated into every surface, with no
per-method opt-in or opt-out. There is no attribute to forget and no argument
list for the macro to quietly ignore.

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

Every method takes the caller: `ctx: &Ctx` (principal, tenant, trace,
cancellation) is the first parameter, and each surface fills it in — HTTP from
headers, MCP from `_meta`, the CLI from `--principal` / `--tenant` / `--trace`,
the client by stamping the same headers the route reads. Authorization is
representable because the contract can see who is calling.

## Generated artifacts

`#[service]` generates behind cargo features, so each host compiles only what it
needs:

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

## One error enum, three mappings

`ErrorKind::NotFound` gives 404, MCP `-32001`, and CLI exit 3 from one match arm
in `crates/gabion/src/error.rs`. The CLI's failure carries the same flat
`WireError` the route sends, which is what lets the generated conformance suite
compare the four surfaces and fail when one of them drops the kind or the detail.

## The reviewable artifacts

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

## The look belongs to the app

The framework ships no components, no tokens and no default theme —
`framework-ui` fails the gate if a framework crate starts to render. A design
system is an app-level crate, named `*-shared` so the edge table lets every
`*-render` crate and every host use it and lets it depend on the runtime only. The
examples' one lives in `example/shared/ui-shared`: tokens as CSS, `Surface`,
`Heading`, `Action`, `Empty`, `Failure`, `Notice`. Replace the file; nothing in
the framework reads it.

That shape is deliberate. Features live inside an app, so a feature's render crate
may use its app's kit — which is why composing two features on one screen produces
one look. A component library meant to travel between *apps* would need headless
components and a theme parameter, and that is a different product.
