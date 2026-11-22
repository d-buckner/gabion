# The rulebook — every shortcut, and the block that stops it

`gabion check` is one command with one exit code. Green means the architecture is
correct. This page is the long form: what each layer enforces, the full table of
rules, and how to tune the wall for an app without patching the framework.

For what the walls do *not* yet hold, see [GAPS.md](GAPS.md). For the
decisions behind them, see [framework-spec.md](framework-spec.md).

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
caught, and [GAPS.md](GAPS.md) says so.

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

The gate runs the tools that already exist, then adds only the rules that need to
know gabion's shape. It does not reimplement a linter.

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
suppression. An agent with zero context can satisfy the gate without reading this
file.

## The rules

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
| The framework growing a default theme, components or a stylesheet | `framework-ui`: the framework ships no UI. A look belongs to the app, in a `*-shared` design-system crate. |
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
| A streaming return in a `#[service]` trait | A hard compile error today: no surface carries a stream yet, and generating them anyway produced an E0038 from code the author did not write. [streaming.md](streaming.md) is the decided shape and the stage that lifts this refusal; until then `Latest` / `Each` are reserved and refused by name. |
| A stream response that stops without its terminator line | Not built yet, and it is the fail-open the design would otherwise ship: a reset body must read as a transport failure, never as a clean end (streaming.md §7). |
| A changed input or output type on an existing op | `surface-snapshot`, whose lines carry a hash of the signature. |
| A TODO in a test file, or a 900-line test | `no-todo`, `file-length`, `too-many-branches` — `tests/` is scanned by the shape rules. |
| A `#[surface(..)]` or `#[internal]` left in a contract | The macro refuses to compile. Per-method surface attributes are gone. |
| `#[allow]` smuggled into a pure crate | `allow-allowlist`, `effect-home-allow` |
| A ban or threshold deleted from `clippy.toml` | `clippy-config` |
| A long file, a deep function, a wide signature | `file-length` 400, `too-many-branches` 10, cognitive complexity 15, 80 lines, 6 arguments |
| A `TODO`, `FIXME`, or `XXX` | `no-todo` |
| Unused dependencies, bad licenses, advisories | `cargo shear`, `cargo deny`. A missing tool is a named failure, or a named SKIP with `--allow-missing-tools` — never silence. |

## Tuning the wall for your app

The edge table is data, not code. An app widens or tightens its own walls in
`Cargo.toml` without patching the framework:

```toml
[workspace.metadata.gabion]
effect-homes = ["myapp-cli"]     # crates allowed to perform effects; `*-native` is already one
max-file-lines = 400
max-branches = 10

[workspace.metadata.gabion.edges]
render = ["framework", "shared", "api"]   # render may never reach core
```

gabion classifies a crate from its name and path, never from a hand-written
label. A name must end in `-api`, `-core`, `-render`, or `-shared`, start with
`host-` or end with `-host`, sit under a `hosts/` directory, or appear in
`framework-crates`. Anything else is an `unclassified-crate` finding, because an
unclassified crate slips past the whole edge table.

The required bans and clippy thresholds work the other way. An app may tighten
them through `max-thresholds`. The gate only ever tightens them: a value that
would raise a limit is ignored, and the raised `clippy.toml` is then a
`clippy-config` finding.
