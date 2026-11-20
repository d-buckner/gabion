//! The prose a scaffolded app starts with. Split from the crate templates because
//! it is documentation, not shape: it changes when the framework's story changes.

pub(crate) const README: &str = r#"# %NAME%

A gabion app. One business core (`%FEATURE%`), three crates for it, two hosts.

```sh
cargo run -p %NAME%-host-server    # http://127.0.0.1:3456
cargo run -p %NAME%-host-native -- todo list
cargo run -p gabion-cli -- check   # the gate: one command, one exit code
```

## The shape

```
features/%FEATURE%/     %FEATURE%-api | %FEATURE%-core | %FEATURE%-render
hosts/                  %NAME%-host-server (axum) | %NAME%-host-native (CLI)
```

`%FEATURE%-render` may name `%FEATURE%-api` and nothing from
`%FEATURE%-core`: the UI cannot reach the business logic except through the
contract the host hands it. Adding a method to the `#[service]` trait publishes it
on every surface at once, which is why `gabion check` locks the reachable set in
`.gabion/surfaces.lock` — a new op is a diff someone reviews.

Add another feature with `gabion feature add <name>`.
"#;
