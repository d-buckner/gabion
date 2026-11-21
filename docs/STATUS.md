# Status

v0.1, on crates.io. The examples run end to end: the todo feature works
as a native binary with a file-backed store and as an HTTP server, `todo-core`
compiles clean to wasm, and `gabion check` passes on this repo. The pulse monitor
runs two features that call each other through a contract, over CLI, HTTP, MCP,
and a wasm board.

## Built

The three-crate feature shape and its edge-table audit; `#[service]` codegen for
clap, axum, MCP (JSON-RPC 2.0 over stdio and HTTP), the OpenAPI 3.1 document, and
a typed client that *is* the service trait over `Http`; automatic registration
(`mount_on`) so a host names the impl and nothing else; per-call `Ctx` on every
surface; the capability model (`Clock`, `Store`, `Random`, `Sleep`, `Http`, plus a
`timeout` combinator) with native and deterministic test impls; an error model
that survives the wire (`WireError`, `FromWire`); the effect ban across time, env,
process, fs, net and threads (clippy's enumerated list, audited, plus
`core-runtime-dep`); the Cargo.toml wall (`unsafe_code = "forbid"`, every member
opted in); a generated four-adapter conformance suite; `gabion new` /
`gabion feature add` / `gabion dev`; `gabion check` with a failing fixture per
rule — including `over-broad-visibility`, `axum-outside-host`,
`unserved-contract`, `no-wasm-proof` and `--fix`; and CI on GitHub Actions.

## Built since the last rewrite

Streaming surfaces, all six stages of
[docs/streaming.md](streaming.md): `gabion::stream` with the `Each` and `Latest`
policy markers; `Caps::Streams`, native over `tokio-tungstenite` and web over
`web_sys::WebSocket`; `GET /api/ws` with the open-stream registry, the namespace
wall and the per-namespace `call` queue; NDJSON on stdout for a listen op; MCP
stdio with one `notifications/progress` per item; and stream cases in the
generated conformance suite. `checks` carries the pair that proves it: `watch`
(`Each`), `tail` (`Latest`) and `probe` as the send op on the same socket.

## Not built

An HTTP
cancellation channel (MCP has one); `gabion host add`; a UI
gallery; a renderer other than Dioxus.

## Claimed, with edges — and the doc says where

The visibility check reads names, so a mention in a comment counts as a use and it
cannot prove a `pub` item is wanted; `axum-outside-host` reads paths, so a
re-export slips past it; `unserved-contract` proves a host names a trait, not that
it mounts it; the conformance suite cannot compare a create, because two calls are
two creates; the lock's hash follows a signature's named types one level, so a
nested struct is hashed by name; the UI snapshots cover the view layer, not
interaction or the async containers; cancellation is offered to a tool, not
imposed on it; a stream's `Each` backpressure reaches the whole connection, because
there is one write queue per connection, and `POST /mcp` refuses a listen op rather
than inventing an answer that could carry only the first item.

[GAPS.md](GAPS.md) is the full inspection log, ordered by how much damage each
silence does. The house rule there: a gap closes when a failing test exists for
it, not when the code looks better.
