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

Nothing open. Six items were here in total. The first two — the error kind
dropped at the wire, and second-resolution ids silently overwriting records —
closed earlier. A re-audit of the gate itself found four more, and each was
confirmed by running the checker against a fixture before it was fixed:

- the surface lock missed an op published as `#[gabion::service]`, because the
  collector matched the bare spelling of the attribute only;
- the surface lock missed every op in a `#[service]` trait declared outside
  `*-api`, while the macro still generated a route and a subcommand for it;
- the generated CLI printed nothing and exited 0 when a result would not
  serialize;
- a workspace with no members passed every rule without inspecting a crate.

All four are closed with fixtures, and recorded in §7.

## 2. Missing capabilities

Nothing open. `crates/gabion/src/caps.rs` now defines `Clock`, `Store`, `Random`,
`Sleep`, `Http`, the `timeout` combinator, and `Caps`. Both entries that were
here are closed with tests and recorded in §7.

What this does not deliver: the generated typed client still does not exist (§4),
so no generated code travels on `Http` yet. The capability is proven against a
real server and a real route, not against a client that is not built.

## 3. Claims ahead of code

| Claim | Reality |
|---|---|
| "four surfaces" (README headline, spec §5) | CLI and HTTP exist. MCP and the typed client do not. README says so, and spec §5 marks each generated artifact as built or not built. |
| cargo-mend-style visibility check (spec §7) | Not built. `pub` past the wall is unpoliced. |
| Generated conformance suite across all four adapters (spec §8) | Not built. `TodoServiceImpl` is exercised through clap and axum by hand, never checked for agreement. |
| `gabion dev` (spec §9) | Not built. |

Two rows that were here are closed and recorded in §7: `unsafe_code = "forbid"`
workspace-wide, the glob re-export wall, and the OpenAPI envelope.

## 4. Not built

- **`gabion new`** — no scaffolding, so every app starts by copying the example
  and hoping it copies the walls too. This is the gap that most limits the
  framework being used by anyone else.
- **MCP surface**, **generated typed client** (`InProcess` + `Http` impls).
- **Per-call `Ctx`** (principal, tenant, trace-id, cancellation) — decided, not
  implemented. Until it lands, no surface can carry a caller identity, so
  authorization is unrepresentable.

## 5. Compile-proof, not shipped

`example/hosts/host-web` and `example/hosts/host-pulse-web` build for
`wasm32-unknown-unknown` and are wired with a fixed clock and an in-memory store
(`TestCaps`, which is what the unit tests run on). There is no bundler config
(`Dioxus.toml`), no `wasm-bindgen` driver, and no way to run either in a browser.
They prove the render crates are wasm-clean; they are not browser apps. The
signed-off posture for the browser is a thin client over HTTP (spec §10), which is
also not built.

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
  in `todo-api`: those are used only inside `#[service]`-generated code, which
  cargo-shear cannot see. The tool's own escape hatch, not a deleted wall.
- Branch-count cap lives in `gabion check` (limit 10), not clippy: clippy's
  `too_many_branches` threshold is hardcoded at 42 and has no config key.

## 7. Closed (kept here so nobody reopens them)

- **Errors could not cross the wire.** `ServiceError<E>` derived `Serialize`
  only, with `kind_code` marked `#[serde(skip)]` — so the one field that maps
  to HTTP status, MCP code and exit code was dropped at every boundary, and a
  remote caller had a string to guess from. Now: `ErrorKind` serializes by
  name, `WireError` is flat and bidirectional (`from_service` /
  `into_service_error`), `ServiceError` has `Display` + `std::error::Error`, and
  the generated HTTP surface emits `WireError` for *every* failure including a
  body that will not parse. A client that knows the app's error enum recovers
  the typed error; one that does not still gets kind and message. Proven by
  `crates/gabion/src/error.rs` tests and by `POST /todo/complete` on a missing
  id.
  Deviation from the signed-off plan: the conversions are generic impls in the
  runtime rather than code the macro generates per service — same effect, less
  codegen, and nothing for an app to regenerate when the runtime changes.
- **Two creates in the same second overwrote each other.** Ids were
  `format!("t{}", now.0)` at whole-second resolution: the second `set()` replaced
  the first, no error, discovered by a missing record. Fixed with a `Random`
  capability (`Caps::random`) and a nonce in the id — `SystemRandom` (getrandom)
  in the native effect home, deterministic `MockRandom` in tests, so id
  behaviour is reproducible. Proven by `two_creates_in_the_same_second_do_not_collide`
  and `ids_are_reproducible_under_test_caps`.
- **`#[surface(...)]` was parsed and thrown away**, so `#[surface(cli)]` still
  emitted an HTTP route, while the gate validated surface names the macro did
  not honor. **Removed, not fixed.** There are no per-method surface
  attributes: every method in a `#[service]` trait reaches every surface, and
  an op that must not be remote lives outside the trait (plain trait in `*-api`
  for cross-feature in-proc, inherent method for feature-local). Proven by
  `crates/gabion-macros/src/tests.rs`.
- **A missing attribute meant exposed.** With no attribute to forget there is
  nothing to default on. `#[service(http)]` and any leftover `#[surface]` /
  `#[internal]` are now hard compile errors rather than silent no-ops — an
  author who believes they restricted a surface must be told they did not.
- **The surface lock could be evaded by spelling.** The collector matched an
  attribute path equal to `service`, so `#[gabion::service]` generated the full
  set of surfaces while the op stayed out of `.gabion/surfaces.lock`. A locked
  surface that depends on spelling is not a boundary. The collector now keys on
  the last path segment. Proven by `surface_lock_sees_a_qualified_service_attribute`.
- **A `#[service]` trait outside `*-api` was invisible.** The collector scanned
  `*-api` crates only, but `#[service]` generates a route, a subcommand and a
  client method wherever it appears. A trait in `*-core` published ops that no
  review ever saw. The collector now scans every crate, and `service-outside-api`
  names a trait declared in the wrong crate. Proven by
  `service_trait_outside_the_contract_crate_is_a_finding`. The lock still records
  names, not input types; that gap stays open in §8.
- **The generated CLI called a failed call a success.** The clap dispatch mapped
  a successful result with `to_string_pretty(..).unwrap_or_default()`, so a value
  whose `Serialize` impl fails produced empty stdout and exit code 0 — the same
  output as a call that succeeded with nothing to show. Rendering moved to
  `gabion::cli::render_json`, which returns a `CliExit` with the `Internal` exit
  code. Proven by `a_success_value_that_will_not_serialize_fails_the_call`, which
  drives the generated dispatch, not the template string.
- **An empty workspace passed.** `gabion check` ran its rules over the crate
  graph from `cargo metadata`. With no members the graph is empty, every rule has
  nothing to check, and the run prints PASS. An empty graph is now a named
  failure. Proven by `a_workspace_with_no_members_is_a_failure`. The same run now
  uses `cargo test --all-features`, so a feature-gated test target cannot be
  skipped into a vacuous green.

- **There was no way to express waiting.** Core code had no timer: `std::thread::sleep`
  blocks a worker and does not exist on wasm, so retry or debounce logic had to be
  faked in every test that touched it. `Caps::sleep` is a `Sleep` capability with
  two methods: `sleep(millis)` to wait, and `timer(millis)` to bound a wait. A
  deadline is a separate method because a timer that advances a virtual clock
  would fire every timeout the moment it was polled. `gabion::caps::timeout` races
  a future against that timer.
  Native: `TokioSleep` over `tokio::time`. Tests: `MockSleep` over a `MockTime`
  virtual clock shared with `MockClock`, which moves only when code asks to wait.
  Proven by `a_wait_moves_the_virtual_clock_and_costs_no_wall_time`,
  `timeout_cuts_off_a_wait_that_runs_past_the_deadline` and
  `timeout_keeps_a_result_that_arrives_first` in `crates/gabion/tests/capabilities.rs`,
  and by the same two timeout tests in `crates/gabion-native/tests/sleep.rs`, run
  against the real tokio timer on its paused clock.
  Deliberate limit: under the test clock a timeout only fires if the awaited code
  sleeps past the deadline, because virtual time moves only on a wait. That is the
  rule tokio's own virtual time follows, and it is documented at `timeout`.
- **A core could not call another service.** The feature wall says cross-feature
  calls go through the generated client, and there was no capability for that call
  to travel on. `Caps::http` is an `Http` trait over `HttpRequest`, `HttpResponse`
  and `HttpError`: a method, a URL, headers, bytes. Native: `ReqwestHttp` with
  rustls, so the single-binary promise holds and no system openssl is needed.
  Tests: `MockHttp` replays scripted routes and records what was sent; a call
  nobody scripted is an error rather than an empty 200. A non-2xx status is a
  response, not a transport error, so the caller picks the `ErrorKind`.
  Proven by `a_core_call_over_http_reaches_the_generated_route` and
  `a_missing_route_comes_back_as_a_status_the_caller_can_read`, which call generic
  `Caps` code over a real socket against the generated axum router, and by
  `mock_http_replays_the_script_and_records_the_request`.
  Deviation from the plan in §2's wording: `Timeout` is a combinator over `Sleep`,
  not a sixth capability. A deadline needs the same clock as the wait it bounds,
  and two capabilities that must agree on one clock is one capability with a
  second method.

- The two gate rules that used to police this (`surface-completeness`,
  `surface-unknown`) were deleted, and replaced by `surface-snapshot`: the
  reviewable artifact is the *set* of reachable ops
  (`.gabion/surfaces.lock`), not an annotation on each one. Proven by
  `surface_drift_is_a_finding` and `updating_the_lock_clears_the_finding`.

- **The spec claimed a Cargo.toml wall that did not exist.** `unsafe_code =
  "forbid"` workspace-wide was in spec §7, and there was no `[workspace.lints]`
  section anywhere to set it. Now the workspace sets it, every member opts into
  it, and the new `workspace-lints` rule audits the block the way `clippy-config`
  audits clippy.toml: a missing section, a level lowered to `warn`, and a member
  that never opts in are each named findings. Proven by
  `a_workspace_with_no_lints_block_is_a_finding`, `a_loosened_workspace_lint_is_a_finding`,
  and `a_member_that_skips_the_workspace_lints_is_a_finding`, and by an `unsafe`
  block in `crates/gabion` failing `cargo clippy`. Cargo itself rejects a member
  that opts into a block the workspace never declared, so the two ways to be
  missing always travel together.
- **A contract could re-export a whole module.** Spec §7 said clippy
  `wildcard_imports` deny on contract would stop glob re-exports across the wall.
  clippy does not lint `pub use` globs — measured on rust 1.99, `use
  crate::inner::*` is a finding and `pub use inner::*` is silent. The re-export is
  the case that matters, because the re-export is the one that publishes. The new
  `contract-glob` rule reads the contract crates and names a `pub use` whose tree
  contains a glob. Proven by `a_glob_reexport_in_a_contract_is_a_finding`, and the
  glob sits in the `every_rule_bites` fixture so the rule cannot go quiet.
- **An app could loosen the audit that audits it.** README says an app may tighten
  the clippy thresholds through `max-thresholds`, and that the gate only ever
  tightens them. `Config::apply` took the app's value outright, so
  `max-thresholds = { cognitive-complexity-threshold = 999 }` in an app's own
  Cargo.toml raised the limit `clippy-config` enforces: the delete-the-wall diff
  taken in two steps. The gate keeps the tighter of the two now. Proven by
  `an_app_cannot_loosen_a_required_threshold`.

- **Nothing emitted a schema.** README promised an OpenAPI-ready envelope and no
  code in the repo produced a schema. `#[service]` now generates
  `<service>_openapi()` under the `server` and `openapi` features: one OpenAPI 3.1
  document per service, built from the types the axum handlers already
  deserialize, with the method's doc comment as the operation summary and
  `WireError` as the one error response. The route string is computed once and
  shared with the router, and the document is gated on the router feature, so it
  cannot describe a route that is not served. The example commits its document to
  `.gabion/openapi/todo.json`, and `todo-api/tests/openapi_snapshot.rs` fails when
  the code and the committed document disagree, fails when an op has no path or a
  `$ref` resolves to nothing, and fails if the trait's prose stops reaching the
  document. Regenerate with
  `GABION_BLESS_OPENAPI=1 cargo test -p todo-api --test openapi_snapshot`.
  Deliberate limits: one schemars contract (deserialize) describes both directions,
  so a DTO whose serialized shape differs from its deserialized shape needs its own
  type; and the document is opt-in per api crate, because requiring `JsonSchema` on
  every DTO is a demand an app may not want to take on.

- **No example had two features.** Everything above claimed a feature wall and a
  cross-feature call, and the repo had one feature, so nothing exercised either.
  `example/features/pulse` is a second app: `checks` probes URLs, `incidents`
  opens and closes outages from probe history, and `incidents-core`'s only window
  on those probes is `checks-api`. `checks-core` is not a dependency of
  `incidents-core` at all — `gabion check` reads dev-dependencies too, so a test
  could not smuggle the implementation in — and the incidents tests drive a stub
  that implements the contract and nothing else.
  `example/hosts/host-pulse-server/tests/live_probe.rs` runs the same core
  against a live socket, a route that returns 503, and a port nobody listens on.
  Adding the two services made `surface-snapshot` fail until the eight new ops
  were reviewed and committed to `.gabion/surfaces.lock`.
  This is also the first code in the repo that waits: the probe ladder is three
  tries with a doubling wait and a deadline, and
  `checks-core/tests/probing.rs` asserts the backoff was 250ms then 500ms on the
  virtual clock, so the wait is proven rather than assumed.
  Two papercuts it exposed and fixed: `TestCaps` had no way to carry a scripted
  `MockHttp` (now `with_http` and `http()`), and `CliExit` had no `Debug`, so a
  test could not unwrap a generated CLI dispatch.

## 8. Gate's own gaps

- **The Forgejo CI never runs.** `.forgejo/workflows/ci.yml` is valid and the
  commit status appears on PRs, but it sits at *Pending* indefinitely and
  `fj actions tasks` reports no tasks — no runner is registered on the instance.
  GitHub Actions runs the same one command on every push and pull request, and
  that is what enforces the wall today.
- `cargo shear` and `cargo deny` are assumed installed; CI installs them, a fresh
  laptop does not have them. `gabion check` reports the step as failed rather
  than "tool missing", which is misleading.
- No `--fix` for mechanical findings.
- Source rules scan `src/` only. `tests/` and `examples/` are unchecked by
  design, which is why the framework's own integration tests carry effect-home
  allows.
- The wasm step builds every `*-core` crate, so an app with no `*-core` crate yet
  gets a vacuous pass on the wall proof.
- The surface lock is a set of names, not a schema: a method that keeps its name
  and changes its input type drifts silently there. The OpenAPI snapshot covers
  that shape for a service that builds the document, and only there — a service
  that leaves the `openapi` feature off is back to names alone. Closing it in the
  lock itself means hashing the input schema into the lock line.

## 9. Cheapest fixes available

- The visibility check spec §7 claims (`pub` past the wall), which is the last
  wall in that table with nothing behind it.
- A `Dioxus.toml` and a `wasm-bindgen` driver for one wasm host, which would turn
  §5 from a compile proof into a running page.
