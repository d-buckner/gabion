//! Configuration: `[workspace.metadata.gabion]` in the app's root Cargo.toml,
//! with defaults that match what `gabion new` generates.
//!
//! The edge table lives here rather than in code so an app can extend it (a new
//! crate class, an extra effect home) without patching the framework — but the
//! *required* effect bans and clippy thresholds are defaults that the checker
//! only ever tightens, never relaxes.

use std::collections::BTreeMap;
use std::path::Path;

/// A crate's role in the wall. Derived from the crate name, never hand-declared.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Api,
    Core,
    Render,
    Shared,
    Host,
    Framework,
    /// Where an app's own effects live: a pty, a serial port, a C library. The
    /// name gabion gives it is `-native`, or a line in
    /// `[workspace.metadata.gabion] effect-homes`.
    EffectHome,
    /// A workspace member gabion cannot classify — which is itself a failure,
    /// because an unclassified crate slips past the whole edge table.
    Unclassified,
}

impl Class {
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Class::Api => "api",
            Class::Core => "core",
            Class::Render => "render",
            Class::Shared => "shared",
            Class::Host => "host",
            Class::EffectHome => "effect-home",
            Class::Framework => "framework",
            Class::Unclassified => "unclassified",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Class {
        match s {
            "api" => Class::Api,
            "core" => Class::Core,
            "render" => Class::Render,
            "shared" => Class::Shared,
            "host" => Class::Host,
            "effect-home" => Class::EffectHome,
            "framework" => Class::Framework,
            _ => Class::Unclassified,
        }
    }
}

/// Crate-name suffix -> class, in the order a name is matched.
const SUFFIX_CLASSES: &[(&str, Class)] = &[
    ("-api", Class::Api),
    ("-core", Class::Core),
    ("-render", Class::Render),
    ("-shared", Class::Shared),
];

/// Where a host lives when its name does not say so.
const HOST_DIRS: &[&str] = &["hosts/", "/hosts/"];

fn is_host_name(name: &str) -> bool {
    name.starts_with("host-") || name.ends_with("-host")
}

pub struct Config {
    /// Framework crates (the gabion runtime itself) — never app features.
    pub framework_crates: Vec<String>,
    /// Extra crates allowed to host effects, beyond `*-native` and hosts.
    pub effect_homes: Vec<String>,
    /// class -> classes it may depend on.
    pub edges: BTreeMap<&'static str, Vec<String>>,
    pub max_file_lines: usize,
    /// Branches per function, counted from the AST (clippy's own gate is 42).
    pub max_branches: usize,
    /// Paths that must stay banned in clippy.toml.
    pub required_bans: Vec<String>,
    /// clippy.toml threshold key -> the loosest value still allowed.
    pub max_thresholds: BTreeMap<String, i64>,
    /// `[workspace.lints.rust]` lint -> the loosest level still allowed. The
    /// rustc-side wall the spec claims (unsafe code).
    pub required_rust_lints: BTreeMap<String, String>,
    /// `[workspace.lints.clippy]` lint -> the loosest level still allowed.
    pub required_clippy_lints: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            framework_crates: default_framework(),
            effect_homes: Vec::new(),
            edges: default_edges(),
            max_file_lines: 400,
            max_branches: 10,
            required_bans: default_required_bans(),
            max_thresholds: default_thresholds(),
            required_rust_lints: default_required_rust_lints(),
            required_clippy_lints: default_required_clippy_lints(),
        }
    }
}

fn default_framework() -> Vec<String> {
    ["gabion", "gabion-macros", "gabion-native", "gabion-cli"]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

fn default_edges() -> BTreeMap<&'static str, Vec<String>> {
    let mut m: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let v = |names: &[&str]| names.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
    // The spec's allowed-edges table, made machine-readable.
    m.insert("api", v(&["framework", "shared", "api"]));
    m.insert("core", v(&["framework", "shared", "api"]));
    // Render may depend on render: a feature's UI is a library of components, and
    // composing two features on one screen is the normal case. What keeps that
    // from becoming "reach into another feature's internals" is the visibility
    // rule, which treats `pub` in a render crate as published surface.
    m.insert("render", v(&["framework", "shared", "api", "render"]));
    m.insert("shared", v(&["framework"]));
    m.insert(
        "host",
        v(&[
            "framework",
            "shared",
            "api",
            "core",
            "render",
            "effect-home",
        ]),
    );
    // An effect home is where the banned crates belong, so it reads the runtime
    // and the contracts but never another feature's logic or UI.
    m.insert("effect-home", v(&["framework", "shared", "api"]));
    m.insert("framework", v(&["framework"]));
    m.insert("unclassified", Vec::new());
    m
}

fn default_required_bans() -> Vec<String> {
    [
        "std::time::SystemTime",
        "std::time::Instant",
        "std::thread::JoinHandle",
        "std::fs::File",
        "std::net::TcpStream",
        "std::time::SystemTime::now",
        "std::thread::sleep",
        "std::thread::spawn",
        "std::env::var",
        "std::process::id",
        "std::fs::read_to_string",
        "std::fs::write",
        "std::fs::metadata",
        "std::fs::copy",
        "std::fs::canonicalize",
        "std::fs::OpenOptions::open",
        "std::net::TcpListener",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

fn default_thresholds() -> BTreeMap<String, i64> {
    let mut m = BTreeMap::new();
    m.insert("cognitive-complexity-threshold".to_string(), 15);
    m.insert("too-many-lines-threshold".to_string(), 80);
    m.insert("too-many-arguments-threshold".to_string(), 6);
    m.insert("type-complexity-threshold".to_string(), 150);
    m
}

/// The rustc wall the spec promises: no unsafe anywhere, and the ban is set once
/// for the whole workspace rather than per crate.
fn default_required_rust_lints() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("unsafe_code".to_string(), "forbid".to_string());
    m
}

/// The clippy half of the same block. `disallowed_*` is what makes the effect
/// ban in clippy.toml bite.
fn default_required_clippy_lints() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for (lint, level) in [
        ("unwrap_used", "deny"),
        ("expect_used", "deny"),
        ("disallowed_types", "deny"),
        ("disallowed_methods", "deny"),
    ] {
        m.insert(lint.to_string(), level.to_string());
    }
    m
}

impl Config {
    /// Load `[workspace.metadata.gabion]` from the workspace root manifest,
    /// falling back to defaults for anything the app does not override.
    pub fn load(root: &Path) -> Result<Self, String> {
        let manifest = root.join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest)
            .map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
        let value: toml::Value = text
            .parse()
            .map_err(|e| format!("cannot parse {}: {e}", manifest.display()))?;
        let mut cfg = Config::default();
        let Some(gabion) = value
            .get("workspace")
            .and_then(|w| w.get("metadata"))
            .and_then(|m| m.get("gabion"))
        else {
            return Ok(cfg);
        };
        cfg.apply(gabion);
        Ok(cfg)
    }

    fn apply(&mut self, gabion: &toml::Value) {
        if let Some(list) = gabion
            .get("framework-crates")
            .and_then(toml::Value::as_array)
        {
            self.framework_crates = list
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
        if let Some(list) = gabion.get("effect-homes").and_then(toml::Value::as_array) {
            self.effect_homes = list
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
        if let Some(n) = gabion
            .get("max-file-lines")
            .and_then(toml::Value::as_integer)
        {
            self.max_file_lines = n as usize;
        }
        if let Some(n) = gabion.get("max-branches").and_then(toml::Value::as_integer) {
            self.max_branches = n as usize;
        }
        if let Some(table) = gabion.get("edges").and_then(toml::Value::as_table) {
            for (class, allowed) in table {
                if let Some(list) = allowed.as_array() {
                    let names: Vec<String> = list
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    self.edges.insert(Class::parse(class).key(), names);
                }
            }
        }
        if let Some(table) = gabion.get("max-thresholds").and_then(toml::Value::as_table) {
            for (key, v) in table {
                if let Some(n) = v.as_integer() {
                    // Only ever tighten. A key that could raise a limit would
                    // loosen the audit that reads this same map.
                    let max = self.max_thresholds.get(key).copied().unwrap_or(i64::MAX);
                    self.max_thresholds.insert(key.clone(), n.min(max));
                }
            }
        }
    }

    #[must_use]
    pub fn may_depend_on(&self, from: Class, to: Class) -> bool {
        self.edges
            .get(from.key())
            .is_some_and(|allowed| allowed.iter().any(|a| a == to.key()))
    }

    #[must_use]
    pub fn classify(&self, name: &str, rel_dir: &str) -> Class {
        if self.framework_crates.iter().any(|f| f == name) {
            return Class::Framework;
        }
        // The list is the app naming its own effect home, so it outranks a suffix.
        if self.effect_homes.iter().any(|h| h == name) {
            return Class::EffectHome;
        }
        if name == "shared" {
            return Class::Shared;
        }
        if let Some((_, class)) = SUFFIX_CLASSES.iter().find(|(s, _)| name.ends_with(s)) {
            return *class;
        }
        if is_host_name(name) || HOST_DIRS.iter().any(|d| rel_dir.contains(d)) {
            return Class::Host;
        }
        // After the host test on purpose: `host-native` is a host written in
        // native code, `dh-native` is a home for effects.
        if name.ends_with("-native") {
            return Class::EffectHome;
        }
        Class::Unclassified
    }

    #[must_use]
    pub fn is_effect_home(&self, name: &str, class: Class) -> bool {
        class == Class::Host
            || class == Class::EffectHome
            || name.ends_with("-native")
            || self.effect_homes.iter().any(|h| h == name)
    }
}
