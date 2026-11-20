# Gaps — what the walls do not yet hold

Gabion's thesis is that the shortest path must be the correct path, so an
inspection log is part of the structure. This file is that log: every place the
framework currently *claims* a wall it does not enforce, or enforces one in the
wrong place.

House rule: **a gap is closed when a failing test exists for it**, not when the
code looks better. Anything here without a linked test is still open, whatever
the code says.

Ordered by how much damage the silence does.

## 1. Fail-open defaults (worst class: it passes and means nothing)

Nothing open. Eight items have been here in total and each was confirmed by
running the checker against a fixture before it was fixed; the closed list is in
§7.

The one this pass added is worth recording because it was *ours*, and it was
introduced by the code that was meant to remove a gap: automatic registration was
first built on `linkme` distributed slices, which are collected by the linker only
when it happens to pull the object file that contains them. Measured on this
toolchain, a real host binary that used both services and read the slice collected
**nothing** — the registry came back empty and a host built on it would have served
no routes while reporting a successful start. A registry that silently misses a
contract is precisely the failure class this project exists to delete, so the
linker is not involved at all now (§7).

## 2. Missing capabilities

Nothing open. `crates/gabion/src/caps.rs` defines `Clock`, `Store`, `Random`,
`Sleep`, `Http`, the `timeout` combinator, and `Caps`. Every entry that was here is
closed with tests and recorded in §7.

## 3. Claims ahead of code

| Claim | Reality |
|---|---|
| "one trait, four surfaces" (README headline, spec §5) | All four exist: clap, axum, MCP over JSON-RPC, and a typed client that *is* the service trait implemented over `Http`. |
| auto-registration via `linkme` (spec §4) | Built, but not with `linkme` (§7). `#[service]` emits `mount_on` on the trait object and a host names only the impl. The browser host uses it too, which the linker version could not. |
| "the wasm target fails on `std::fs` / `std::net` / `std::thread` / `tokio`" | False on the pinned toolchain, as §7 records. The effect wall is the `clippy.toml` ban list plus `core-runtime-dep`. |
| "MCP via rmcp" (spec §5) | Deliberately not a dependency (§7). The runtime speaks the three methods this framework uses (`initialize`, `tools/list`, `tools/call`) over JSON-RPC 2.0, and every path is tested in-repo. No SSE stream: a tool call answers once. |
| `gabion dev` (spec §9) | Built as a watch/rebuild/restart loop over one host binary. It does not start a wasm bundler: that is `serve.sh` in an app that has one, and wiring it here is a host decision. |
| Generated conformance suite across all four adapters (spec §8) | Built, with one honest hole: a *create* is not repeatable, so the cases are observations and failures (see §8). |
| a `*-core` that names a runtime outside the banned list | `core-runtime-dep` reads an enumerated list (`tokio`, `axum`, `reqwest`, `sqlx`, …). A crate outside it gets through, and so does any effect outside `clippy.toml`'s enumerated paths. That is a wall with edges, not a proof. |
| `over-broad-visibility` (spec §7) | Built in the only decidable direction: a `pub` item in a `*-core` that no other crate names is a finding and `--fix` narrows it. It cannot prove the remaining `pub` items are wanted, and it reads names, so a mention in a comment counts as a use. |
| `axum` importable only in generated modules and host mains (spec §7) | Built as a source scan over every non-host app crate. It reads paths, not types: a crate that reaches axum through a re-export is not caught by it. |

The earlier rows about `unsafe_code = "forbid"` workspace-wide, the glob re-export
wall, the OpenAPI envelope, and the error kind dropped at the wire are closed and
recorded in §7.

## 4. Not built

- **Renderer-agnostic UI kit.** The render crates print or use Dioxus directly;
  there is no shared component kit (tokens, layout, table, button), and the spec
  still lists that as an open decision. Composition itself is no longer blocked —
  `render -> render` is open and panels take props — but two features' panels do not
  yet *look* like each other.
- **`Ctx` propagation into capability impls.** The caller reaches every service
  method, but a `Store` write does not know who is writing. Per-record attribution
  is an app decision until something needs it.
- **Streaming surfaces** (`Stream` return support) — v1 or v2 is still open in the
  spec, and nothing in the code reserves the shape beyond the trait being
  `async_trait`.
- **`Ctx` cancellation across a process boundary.** HTTP drops the handler future
  on disconnect, which cancels; there is no wire-level cancel message, so a client
  that stops listening learns nothing and a server that is still probing cannot be
  told to stop.
- **`gabion feature remove`**, and any scaffolding for a second host: `gabion new`
  writes two, `gabion feature add` writes crates.
- **The publish story.** v0.1 is path-dependency only (`publish = false`), so
  `gabion new` takes `--gabion-path` and every scaffolded app points at a
  checkout. Nothing here is ready for crates.io.

## 5. Browser and desktop, shipped for one app

`example/hosts/host-pulse-native` and `example/hosts/host-pulse-web` are runnable
apps, not compile proofs. Both run the unchanged `checks-core` and
`incidents-core` and show `pulse-render`: the desktop host over `FileCaps` (state
in `$PULSE_STORE`), the web host over a real `WebCaps` (js clock, localStorage
store, `crypto.getRandomValues`, `setTimeout` timers, `fetch` http), served with
`example/hosts/host-pulse-web/serve.sh`. First run seeds two demo watches so
there is something to sweep.

What the browser build comes with, and the doc says so rather than hiding it:
`fetch` is CORS-bound, so a probe target that sends no
`Access-Control-Allow-Origin` fails as a transport error; localStorage is
per-origin; and the browser http capability drops response headers (v1 carries
JSON bodies). The thin-client posture (spec §10.3) is now available — a page can
be handed `TodoClient` instead of the core, and `host-web` demonstrates both — but
no example *ships* that way: `host-pulse-web` runs the cores locally on purpose.

`example/hosts/host-web` (todo) remains a `TestCaps` wall proof with no bundler
config and no way to run it in a browser.

## 6. Deliberate allowances (not gaps, but they look like gaps)

- `deny.toml`: `unmaintained = "workspace"`. The transitive hits
  (`RUSTSEC-2024-0370` paste, `RUSTSEC-2024-0436` proc-macro-error) arrive via
  `dioxus-desktop → image → exr/rav1e` and an app cannot upgrade them.
  Vulnerabilities and notices are still deny.
- `deny.toml`: `allow-workspace` / `allow-wildcard-paths`, with `publish = false`
  on every member. Workspace path deps have no version requirement; Cargo.lock
  is what pins them. A `"*"` from crates.io still fails.
- `deny.toml`: `CDLA-Permissive-2.0` is an allowed license. One crate uses it:
  `webpki-roots`, the bundled Mozilla root-certificate set the native `Http`
  capability trusts. It is a data license, not a code license. The alternative is
  `rustls-tls-native-roots`, which reads the host's trust store and breaks the
  single-binary promise the http client exists to keep.
- `[package.metadata.cargo-shear] ignored = ["serde", "axum", "bytes", "clap"]`
  in the api crates: those are used only inside `#[service]`-generated code, which
  cargo-shear cannot see. The tool's own escape hatch, not a deleted wall.
- Branch-count cap lives in `gabion check` (limit 10), not clippy: clippy's
  `too_many_branches` threshold is hardcoded at 42 and has no config key.
- `tests/` and `examples/` are scanned by a *subset* of the source rules — file
  length, branch caps, TODOs, `#[allow]` placement — and not by the structural
  ones (comment rule, silent defaults, contract purity). An integration test is an
  effect home that ships nowhere, and applying the purity rules there was the
  reason it went unscanned; the shape rules apply because a reviewer reads those
  files too.

## 7. Closed (kept here so nobody reopens them)

- **A registry the linker could quietly empty.** Automatic registration was built
  on `linkme` and measured empty in a real host binary — see §1. Replaced by an
  inherent method the macro puts on the trait object (`impl dyn CheckService +
  Send + Sync { fn mount_on(self: Arc<Self>, host: &mut Host) }`), which needs no
  linker sections, works on wasm, and cannot be silently absent: the call is in
  the host's source. Proven by `todo-api/tests/registry.rs`,
  `host-pulse-server/tests/registry.rs` (mounted set, exposed ops, MCP tool list,
  a contract mounted twice), and `host-pulse-web/tests/board.rs` (the browser build
  reports the operations it holds).
- **A contract nothing serves.** The registry cannot see an absent call, so
  `unserved-contract` checks the other direction: a `#[service]` trait no host
  crate names is a finding. Proven by `a_contract_no_host_serves_is_a_finding`, and
  visible in `gabion feature add`'s own test, where a freshly added feature fails
  the gate until a host mounts it.
- **`pub` past the wall.** `over-broad-visibility` + `--fix`; see §3 for the limit.
  Proven by `a_pub_nobody_names_is_over_broad_and_the_fix_applies`, which asserts
  the item a host *does* name is left alone.
- **Feature UIs could not be composed.** `render-isolation` banned `render ->
  render`, which made "put two features on one screen" a rule violation and pushed
  the only legal composition into a crate that had to be hand-waved as not-a-render.
  The edge is open now, and the wall moved rather than disappeared: `over-broad-
  visibility` applies to `*-render`, so `pub` means "a component another feature may
  compose" and everything else is `pub(crate)`. `render -> core` is untouched, so
  UI composition can never reach business logic, and a cycle between two render
  crates is still a cargo error. Proven by
  `a_render_crate_may_compose_another_feature_render` (two feature renders plus a
  composition root pass) and `a_render_internal_marked_pub_is_a_finding` (a `pub`
  helper inside one is still a finding). The rule immediately found three
  published-and-uncalled items in this repo's own examples.
- **A UI change was invisible.** A component could keep its props and change what
  it shows, and nothing on the wire would notice. `.gabion/ui/` now holds a
  committed snapshot per published panel — `todo-list.txt` (the string view),
  `todo-panel.html` and `board-panel.html` (the components, rendered through
  `dioxus-ssr` from fixture data inside a `VirtualDom`, so no runtime is
  required) — re-blessed with `GABION_BLESS_UI=1` and compared otherwise. The UI
  counterpart of `openapi_snapshot.rs`. Proven by the two snapshot tests, which
  fail when the markup and the committed file disagree.
- **UI components took service handles.** `BoardPanel` and `TodoPanel` now take
  props — data from `*-api` plus a callback — so a composing screen inherits one
  signature and the wire types, not `Svc` and the store behind it. App chrome (the
  form, the toolbar, the banners) stays in the app component that owns the loading.
  They are `pub(crate)` until a second screen mounts them, at which point the
  visibility rule asks for the `pub`.
- **A hand-written HTTP handler.** `axum-outside-host`; see §3 for the limit.
  Proven by `axum_outside_a_host_is_a_finding` (a feature crate is a finding, a
  host crate importing the same path is not, and the framework's own transport is
  exempt because it *is* the machinery).
- **A missing tool read as a failing one.** `cargo-shear`/`cargo-deny` absent used
  to be reported as a failed step. `steps::run` now distinguishes "could not
  start" from "ran and failed"; a missing tool is a failure by default and a
  *named* `SKIP` under `--allow-missing-tools`, counted in the summary line.
  Proven by `dev_once_runs_the_command_and_reports_its_status` (the process
  boundary) and by the summary's own unit tests.
- **`tests/` and `examples/` were unscanned.** Now scanned by the shape rules with
  a whole-file test grant, so an `#[allow(clippy::disallowed_*)]` in an integration
  test stays legal (it is an effect home) while a TODO or a 900-line file does not.
  Proven by `tests_are_scanned_for_shape_but_granted_their_allows`.
- **A workspace with no `*-core` passed the wasm step vacuously.** `no-wasm-proof`
  is a finding now. Proven by `a_workspace_with_no_core_cannot_prove_the_wasm_wall`.
- **The surface lock recorded names, not shapes.** Each line now carries a hash of
  the operation's signature and the same-crate definitions it names
  (`checks-api::CheckService::add #3f2a1b9c`), so a method that keeps its name and
  changes its input or output type is a diff. Rendered from tokens rather than
  `Debug`, because syn's `Debug` embeds byte offsets and made the hash change when
  a file moved. Proven by `a_changed_input_type_is_a_finding` and the digest's own
  unit tests. Limit: a type defined in *another* crate is hashed by name only.
- **There was no way to scaffold an app.** `gabion new` writes a workspace whose
  `clippy.toml`, `[workspace.lints]` and edge table are generated from the same
  `Config::default()` the gate audits, and whose surface lock is written by the
  gate's own collector. `gabion feature add` grows a feature and its members.
  `gabion dev` watches sources and restarts the host. Proven by
  `crates/gabion-cli/tests/new.rs`, which scaffolds into a temp directory and runs
  the real gate over it — plus the refusal cases (occupied directory, a
  `--gabion-path` that is not a checkout, a duplicate feature).
- **No adapter agreement check.** `<ns>_conformance(svc, http, base, cases)` drives
  one case through the impl, the generated clap tree, the generated MCP tool and
  the generated client over a socket, and compares the four as JSON. The CLI now
  carries the same flat `WireError` the route sends (`CliExit::wire`), which is
  what made the comparison possible at all. Proven by
  `host-server/tests/conformance.rs` and `host-pulse-server/tests/conformance.rs`,
  including a failure case where all four must agree on the kind *and* the detail.
  A case the suite cannot drive is a rejection, not a pass.
- **No per-call context.** `gabion::ctx::Ctx` (principal, tenant, trace, a
  cancellation flag with a waker list, no runtime dependency) is required as the
  first parameter of every `#[service]` method, and each surface fills it in: HTTP
  from headers, MCP from `_meta`, the CLI from `--principal`/`--tenant`/`--trace`,
  the client by stamping the same headers the route reads. Proven by
  `host-server/tests/identity.rs` (the generated clap tree and the generated
  client→route pair both deliver the caller the caller named, and a call with no
  headers stays anonymous) and `crates/gabion/tests/cancellation.rs`.
- **No typed client.** `<Feature>Client<H: Http>` implements the *service trait*,
  so a renderer cannot tell a remote impl from a local one. `FromWire` turns a wire
  failure back into the app's error enum, and the compiler demands that impl the
  moment a client is generated. Proven by `todo-api/tests/client.rs`,
  `checks-api`/`incidents-api` client tests, and the conformance suite.
- **No MCP surface.** `gabion::mcp` speaks JSON-RPC 2.0 over one `Server`, served
  over stdio (`gabion-native`) and HTTP (`POST /mcp`). `ErrorKind::mcp_code` is
  used, so a failure is the same kind as the route's 404 and the CLI's exit 3.
  Tool schemas are inlined, because an `inputSchema` travels to a model without the
  definitions map it might reference. Proven by `crates/gabion/src/mcp/*`,
  `todo-api/tests/mcp.rs`, and `host-pulse-server/tests/mcp.rs` (both transports,
  one server).
- **Errors could not cross the wire.** `ServiceError<E>` derived `Serialize` only,
  with `kind_code` marked `#[serde(skip)]`. Now: `ErrorKind` serializes by name,
  `WireError` is flat and bidirectional, and the generated HTTP surface emits
  `WireError` for *every* failure. Proven by `crates/gabion/src/error.rs` tests and
  by `POST /api/todo.complete` on a missing id.
- **Two creates in the same second overwrote each other.** Fixed with a `Random`
  capability and a nonce in the id. Proven by
  `two_creates_in_the_same_second_do_not_collide` and
  `ids_are_reproducible_under_test_caps`.
- **`#[surface(...)]` was parsed and thrown away.** Removed, not fixed: every
  method in a `#[service]` trait reaches every surface, and an op that must not be
  remote lives outside the trait. A leftover `#[surface]`/`#[internal]` is a hard
  compile error. Proven by `crates/gabion-macros/src/tests.rs`.
- **The surface lock could be evaded by spelling** (`#[gabion::service]`). The
  collector keys on the last path segment. Proven by
  `surface_lock_sees_a_qualified_service_attribute`.
- **A `#[service]` trait outside `*-api` was invisible.** The collector scans every
  crate, and `service-outside-api` names a trait in the wrong crate. Proven by
  `service_trait_outside_the_contract_crate_is_a_finding`.
- **The generated CLI called a failed call a success.** Rendering moved to
  `gabion::cli::render_json`, which returns a `CliExit` with the `Internal` code.
  Proven by `a_success_value_that_will_not_serialize_fails_the_call`.
- **An empty workspace passed.** Now a named failure, and the run uses
  `cargo test --all-features`. Proven by `a_workspace_with_no_members_is_a_failure`.
- **There was no way to express waiting.** `Caps::sleep` and `gabion::caps::timeout`
  over `TokioSleep` natively and `MockSleep` on a virtual clock in tests. Proven by
  the timeout tests in `crates/gabion/tests/capabilities.rs` and
  `crates/gabion-native/tests/sleep.rs`.
- **A core could not call another service.** `Caps::http` over `ReqwestHttp`
  (rustls) and `MockHttp`. Proven by
  `a_core_call_over_http_reaches_the_generated_route`.
- **The spec claimed a Cargo.toml wall that did not exist.** `unsafe_code =
  "forbid"` is now set once for the workspace and `workspace-lints` audits it,
  including a member that never opts in.
- **An app could loosen the audit that audits it.** `Config::apply` takes the
  tighter of the two values. Proven by `an_app_cannot_loosen_a_required_threshold`.
- **A contract could re-export a whole module.** `contract-glob` reads the contract
  crates; clippy does not lint `pub use` globs. Proven by
  `a_glob_reexport_in_a_contract_is_a_finding`.
- **The spec's wire decision did not match the wire.** Route computed once in
  `route()`, `/api/{feature}.{method}`, shared by the router and the OpenAPI
  document, with the three committed documents regenerated.
- **Nothing emitted a schema.** `#[service]` generates `<service>_openapi()` behind
  `server` + `openapi`, and the example commits the document;
  `openapi_snapshot.rs` fails when they disagree, when an op has no path, when a
  `$ref` resolves to nothing, or when the trait's prose stops reaching the document.
- **No example had two features.** `example/features/pulse` is a second app whose
  `incidents-core` reaches `checks` only through `checks-api`, and
  `host-pulse-server/tests/live_probe.rs` runs the same core against a live socket,
  a 503, and a closed port.

## 8. Gate's own gaps

- **The Forgejo CI never runs.** `.forgejo/workflows/ci.yml` is valid and the
  commit status appears on PRs, but it sits at *Pending* indefinitely and
  `fj actions tasks` reports no tasks — no runner is registered on the instance.
  GitHub Actions runs the same one command on every push and pull request, and
  that is what enforces the wall today.
- **The conformance suite cannot compare a create.** Two calls to `check.add` are
  two watches with two ids, so the cases are the ones a repeat answers the same
  way: observations and failures. A create-shaped success is covered by the client
  tests, which script the reply instead of repeating the call. Comparing creates
  would need per-case state snapshots, which the capability model does not expose.
- **`--fix` covers two rules.** `over-broad-visibility` and the surface lock. Every
  other finding needs a decision (move this type, split this function) and is
  advice, because a tool that rewrites architecture is a tool nobody can review.
- **`unserved-contract` reads names.** It proves a host crate mentions the trait,
  not that it mounts an impl. The runtime half is the host's own test, which mounts
  and asserts on `exposed()`.
- **The wasm step builds every `*-core`**, and `no-wasm-proof` now fails a
  workspace that has none — but a workspace whose only core is a placeholder crate
  still gets a vacuous pass, and nothing checks that a core is *used*.

## 9. Cheapest fixes available

- A `Dioxus.toml` and a `wasm-bindgen` driver for one wasm host, so the browser
  story is a running page in the examples rather than in one app's `serve.sh`.
- Extending the digest in the surface lock to definitions in *other* crates, which
  means resolving `*-api` imports in the collector. The OpenAPI snapshot already
  covers a service that builds the document; this would cover the rest.
- Per-record attribution: threading `Ctx` into `Store` writes, so a store can say
  who last touched a key.
