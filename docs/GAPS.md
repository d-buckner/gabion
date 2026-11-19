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

### 1.1 Errors cannot cross the wire

`crates/gabion/src/error.rs:44` — `#[derive(Debug, Serialize)]` with
`#[serde(skip)] pub kind_code: ErrorKind`. There is no `Deserialize`, no
`Display`, no `impl std::error::Error`.

Consequence: `ErrorKind` — the thing that maps to HTTP status, MCP error code
and CLI exit code — is dropped on serialization and can never be recovered at
the client. A remote caller sees a string and guesses. The spec's "one error
enum, one mapping, everywhere" is true in-process and false across the network.

Fix (signed off): a flat `WireError` with `ErrorKind` serialized, plus generated
`From` impls in both directions. In-proc error stays rich.

### 1.2 Two creates in the same second silently overwrite each other

`example/features/todo/todo-core/src/lib.rs:35` — `let id = format!("t{}", now.0)`,
where `now` is whole seconds. Two `create` calls in one second produce the same
id, and the second `set()` replaces the first. No error, no trace.

This is the same failure mode as the `unwrap_or_default()` data loss fixed in
#2: silent overwrite, discovered by a missing record.

Fix needs a capability that does not exist yet — see 2.1. A monotonic counter or
random suffix sourced from `Caps` is the only pure option; `std::time` is banned
in core for good reason and `Clock` is deliberately coarse.

## 2. Missing capabilities (these block 1.2 and more)

`crates/gabion/src/caps.rs` defines `Clock`, `Store`, `Caps`. Not defined:

- **`Random` / `IdGen`** — required for any pure id generation (1.4).
- **`Sleep` / `Timeout`** — no testable way to express waiting.
- **`Http`** — a core cannot call another service, so the feature wall's
  "cross-feature calls go through the generated client" has no capability to
  travel on.

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
- The two gate rules that used to police this (`surface-completeness`,
  `surface-unknown`) were deleted, and replaced by `surface-snapshot`: the
  reviewable artifact is the *set* of reachable ops
  (`.gabion/surfaces.lock`), not an annotation on each one. Proven by
  `surface_drift_is_a_finding` and `updating_the_lock_clears_the_finding`.

## 8. Gate's own gaps

- **CI does not actually run.** `.forgejo/workflows/ci.yml` is valid and the
  commit status appears on PRs, but it sits at *Pending* indefinitely and
  `fj actions tasks` reports no tasks — no runner is registered on the instance.
  Until one is, the gate is enforced locally and by whoever reviews a PR, not by
  the server. This is the difference between a wall and a drawing of a wall, so
  it is the first thing to fix.
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

- Register a CI runner (§8) — until then the gate is a local promise.
- `unsafe_code = "forbid"` in `[workspace.lints.rust]`: one line, and the spec
  already claims it.
- A `Random`/`IdGen` capability, which unblocks the id collision in 1.2.
