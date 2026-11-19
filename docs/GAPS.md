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

`crates/gabion/src/caps.rs` defines `Clock`, `Store`, `Random`, `Caps`. Not
defined:

- **`Sleep` / `Timeout`** — no testable way to express waiting, so any retry or
  debounce logic would have to fake it.
- **`Http`** — a core cannot call another service, so the feature wall's
  "cross-feature calls go through the generated client" has no capability to
  travel on. This is also what the wasm thin-client posture (spec §10.3) needs
  before it can exist.

## 3. Claims ahead of code

| Claim | Reality |
|---|---|
| "four surfaces" (README headline, spec §5) | CLI and HTTP exist. MCP and the typed client do not. README says so; spec §5 describes the intended shape. |
| `unsafe_code = "forbid"` workspace-wide (spec §7) | Not set anywhere. No `[workspace.lints.rust]` section exists. One line to fix; deliberately not bundled into this PR. |
| clippy `wildcard_imports` deny on contract (spec §7) | Not set. |
| "OpenAPI-ready envelope" (README) | No `schemars`, no OpenAPI export, no contract snapshot. Nothing in the repo emits a schema. |
| cargo-mend-style visibility check (spec §7) | Not built. `pub` past the wall is unpoliced. |
| Generated conformance suite across all four adapters (spec §8) | Not built. `TodoServiceImpl` is exercised through clap and axum by hand, never checked for agreement. |
| `gabion dev` (spec §9) | Not built. |

## 4. Not built

- **`gabion new`** — no scaffolding, so every app starts by copying the example
  and hoping it copies the walls too. This is the gap that most limits the
  framework being used by anyone else.
- **`gabion-codegen`** — OpenAPI export and reviewable contract snapshots.
- **MCP surface**, **generated typed client** (`InProcess` + `Http` impls).
- **Per-call `Ctx`** (principal, tenant, trace-id, cancellation) — decided, not
  implemented. Until it lands, no surface can carry a caller identity, so
  authorization is unrepresentable.

## 5. Compile-proof, not shipped

`example/hosts/host-web` builds for `wasm32-unknown-unknown` and is wired with
`MockClock` + `InMemoryStore`. There is no bundler config (`Dioxus.toml`), no
`wasm-bindgen` driver, and no way to run it in a browser. It proves the render
crate is wasm-clean; it is not a browser app. The signed-off posture for the
browser is a thin client over HTTP (spec §10), which is also not built.

## 6. Deliberate allowances (not gaps, but they look like gaps)

- `deny.toml`: `unmaintained = "workspace"`. The transitive hits
  (`RUSTSEC-2024-0370` paste, `RUSTSEC-2024-0436` proc-macro-error) arrive via
  `dioxus-desktop → image → exr/rav1e` and an app cannot upgrade them.
  Vulnerabilities and notices are still deny.
- `deny.toml`: `allow-workspace` / `allow-wildcard-paths`, with `publish = false`
  on every member. Workspace path deps have no version requirement; Cargo.lock
  is what pins them. A `"*"` from crates.io still fails.
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

- The two gate rules that used to police this (`surface-completeness`,
  `surface-unknown`) were deleted, and replaced by `surface-snapshot`: the
  reviewable artifact is the *set* of reachable ops
  (`.gabion/surfaces.lock`), not an annotation on each one. Proven by
  `surface_drift_is_a_finding` and `updating_the_lock_clears_the_finding`.

## 8. Gate's own gaps

- **The Forgejo CI never runs.** `.forgejo/workflows/ci.yml` is valid and the
  commit status appears on PRs, but it sits at *Pending* indefinitely and
  `fj actions tasks` reports no tasks — no runner is registered on the instance.
  `.github/workflows/ci.yml` is configured for the move to GitHub and is the
  same one command. Until one of the two actually executes, the gate is enforced
  locally and by whoever reviews a PR. This is the difference between a wall and
  a drawing of a wall, so it is the first thing to fix.
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
  and changes its input type drifts silently. Fixing it means hashing the input
  DTO into the lock line, which wants `schemars` (§4).

## 9. Cheapest fixes available

- Register a CI runner (§8) — until then the gate is a local promise. GitHub
  Actions is configured (`.github/workflows/ci.yml`) and just needs a repo to
  run on.
- `unsafe_code = "forbid"` in `[workspace.lints.rust]`: one line, and the spec
  already claims it.
- A `Random`/`IdGen` capability, which unblocks the id collision in 1.2.
