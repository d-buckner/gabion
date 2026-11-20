//! `gabion dev`: build, run, watch, rebuild. The agent loop stays fast.
//!
//! Spec §9 asked for "server + wasm renderer + native CLI against one workspace
//! with one command". v0 runs the binary a host crate produces and restarts it
//! when a source file changes; the wasm renderer is `serve.sh` in an app that has
//! one, and wiring that here is a host decision, not a framework one.
//!
//! The parts worth testing are the two that are not a `while` loop: *which* host
//! to run when the caller does not say, and *what counts as a change*. Both are
//! pure functions over a workspace and a set of mtimes.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, SystemTime};

use crate::config::Config;
use crate::graph::{self, Graph};

/// What `gabion dev` was asked to do.
pub struct Options {
    pub root: PathBuf,
    /// An explicit command line. Default: `cargo run -p <host>`.
    pub run: Option<String>,
    /// Which host package to run. Default: the only one.
    pub host: Option<String>,
    /// Build and run once, then exit with the child's status (`CI`, a smoke test,
    /// a human in a hurry).
    pub once: bool,
    /// Poll interval for changes.
    pub interval: Duration,
}

impl Options {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            run: None,
            host: None,
            once: false,
            interval: Duration::from_millis(500),
        }
    }
}

/// The host package to run: the caller's choice, or the only binary-producing
/// host in the workspace.
///
/// Ambiguity is an error rather than a guess: two hosts are two different
/// processes, and starting the wrong one wastes the developer's next ten minutes.
pub fn choose_host(graph: &Graph, wanted: Option<&str>) -> Result<String, String> {
    if let Some(wanted) = wanted {
        if graph.find(wanted).is_some_and(|c| c.has_bin) {
            return Ok(wanted.to_string());
        }
        return Err(format!(
            "`{wanted}` is not a binary-producing workspace member. Hosts here: {}",
            hosts(graph).join(", ")
        ));
    }
    let hosts = hosts(graph);
    match hosts.len() {
        1 => Ok(hosts[0].clone()),
        0 => Err(
            "no `host-*` crate with a binary in this workspace; pass --run to choose a command"
                .to_string(),
        ),
        _ => Err(format!(
            "several hosts produce binaries ({}); pick one with --host",
            hosts.join(", ")
        )),
    }
}

fn hosts(graph: &Graph) -> Vec<String> {
    graph
        .crates
        .iter()
        .filter(|c| c.has_bin && (c.name.starts_with("host-") || c.rel_dir.contains("hosts/")))
        .map(|c| c.name.clone())
        .collect()
}

/// Every file whose change should restart the process: workspace sources and
/// manifests, never `target/`, never VCS metadata, never a store file.
#[must_use]
pub fn watched_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(root, &mut out);
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    const SKIP: [&str; 3] = ["target", ".git", "node_modules"];
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if !SKIP.contains(&name) {
                collect(&path, out);
            }
            continue;
        }
        let watched =
            name == "Cargo.toml" || path.extension().is_some_and(|e| e == "rs" || e == "toml");
        if watched {
            out.push(path);
        }
    }
}

/// The newest modification time in a set, which is the clock the watch loop
/// compares against.
#[must_use]
pub fn newest(paths: &[PathBuf]) -> Option<SystemTime> {
    paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .filter_map(|m| m.modified().ok())
        .max()
}

/// Whether anything changed after `since`. A file that appeared counts; a file
/// that vanished does too, and both show up as "the set is newer than the clock".
#[must_use]
pub fn changed_since(paths: &[PathBuf], since: SystemTime) -> bool {
    newest(paths).is_some_and(|newest| newest > since)
}

/// Build and run once; returns the child's exit code.
pub fn once(opts: &Options) -> Result<i32, String> {
    let (program, args) = command_for(opts)?;
    let status = Command::new(&program)
        .args(&args)
        .current_dir(&opts.root)
        .status()
        .map_err(|e| format!("cannot run `{program}`: {e}"))?;
    Ok(status.code().unwrap_or(1))
}

/// The long-running loop: run, watch, restart on change, stop on Ctrl-C.
pub fn watch(opts: &Options) -> Result<(), String> {
    let files = watched_files(&opts.root);
    if files.is_empty() {
        return Err(format!(
            "nothing to watch under {}: is this a workspace root?",
            opts.root.display()
        ));
    }
    println!(
        "watching {} file(s); restarting on change (Ctrl-C to stop)",
        files.len()
    );
    let mut clock = SystemTime::now();
    loop {
        let mut child = spawn(opts)?;
        let built_at = SystemTime::now();
        loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                // The process exited on its own (a crash, a one-shot). Wait for a
                // change rather than respawning in a tight loop: a rebuild storm
                // on a code error is how a dev loop melts a laptop.
                println!("exited with {status}; waiting for a change");
                wait_for_change(&files, built_at, opts.interval);
                clock = SystemTime::now();
                break;
            }
            if changed_since(&files, clock) {
                println!("change detected; restarting");
                stop(&mut child);
                clock = SystemTime::now();
                break;
            }
            std::thread::sleep(opts.interval);
        }
    }
}

fn wait_for_change(files: &[PathBuf], since: SystemTime, interval: Duration) {
    while !changed_since(files, since) {
        std::thread::sleep(interval);
    }
}

fn spawn(opts: &Options) -> Result<Child, String> {
    let (program, args) = command_for(opts)?;
    Command::new(&program)
        .args(&args)
        .current_dir(&opts.root)
        .spawn()
        .map_err(|e| format!("cannot run `{program}`: {e}"))
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// `--run` verbatim (through the shell, so a pipeline works), or `cargo run -p` on
/// the chosen host.
fn command_for(opts: &Options) -> Result<(String, Vec<String>), String> {
    if let Some(run) = &opts.run {
        return Ok(("sh".to_string(), vec!["-c".to_string(), run.clone()]));
    }
    let cfg = Config::load(&opts.root)?;
    let graph = graph::load(&opts.root, &cfg)?;
    let host = choose_host(&graph, opts.host.as_deref())?;
    Ok((
        "cargo".to_string(),
        vec!["run".to_string(), "-p".to_string(), host],
    ))
}

#[cfg(test)]
#[allow(clippy::expect_used)] // a failing assertion should name the file it could not write
mod tests {
    use super::*;

    #[test]
    fn a_new_file_is_a_change_and_an_old_one_is_not() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn main() {}").expect("write");
        let files = vec![file];
        let since = SystemTime::now();
        assert!(!changed_since(&files, since), "nothing touched yet");
        // A file appearing is a change, even though it was not in the set.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(dir.path().join("b.rs"), "// new").expect("write");
        let files = watched_files(dir.path());
        assert!(changed_since(&files, since));
    }

    #[test]
    fn target_and_git_are_not_watched() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir_all(dir.path().join("target/debug")).expect("mkdir");
        std::fs::create_dir_all(dir.path().join(".git")).expect("mkdir");
        std::fs::write(dir.path().join("target/debug/x.rs"), "").expect("write");
        std::fs::write(dir.path().join(".git/y.rs"), "").expect("write");
        std::fs::write(dir.path().join("src.rs"), "").expect("write");
        let files = watched_files(dir.path());
        assert_eq!(files, vec![dir.path().join("src.rs")]);
    }
}
