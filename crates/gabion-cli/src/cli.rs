//! Argument parsing, kept out of main so the whole tool is testable as a lib.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use crate::report::Summary;
use crate::Options;

/// Parse argv and run the named subcommand.
pub fn run(args: &[String]) -> (Summary, ExitCode) {
    match Args::parse(args) {
        Ok(Args::Check(opts)) => {
            let summary = crate::check(&opts);
            let code = exit_for(&summary);
            (summary, code)
        }
        Ok(Args::New(opts)) => {
            let summary = crate::commands::new_app(&opts);
            let code = exit_for(&summary);
            (summary, code)
        }
        Ok(Args::FeatureAdd(opts)) => {
            let summary = crate::commands::add_feature(&opts);
            let code = exit_for(&summary);
            (summary, code)
        }
        Ok(Args::FeatureRemove(opts)) => {
            let summary = crate::commands::remove_feature(&opts);
            let code = exit_for(&summary);
            (summary, code)
        }
        Ok(Args::Dev(opts)) => {
            let summary = crate::commands::dev(&opts);
            let code = exit_for(&summary);
            (summary, code)
        }
        Ok(Args::Help) => (help(), ExitCode::SUCCESS),
        Ok(Args::Version) => (version(), ExitCode::SUCCESS),
        Err(message) => (usage_error(&message), ExitCode::FAILURE),
    }
}

fn exit_for(summary: &Summary) -> ExitCode {
    if summary.ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// What `gabion new` parsed.
pub struct NewOptions {
    pub name: String,
    pub dir: PathBuf,
    pub gabion_path: Option<PathBuf>,
}

/// What `gabion feature add` parsed.
pub struct FeatureOptions {
    pub name: String,
    pub root: PathBuf,
}

pub enum Args {
    Check(Options),
    New(NewOptions),
    FeatureAdd(FeatureOptions),
    FeatureRemove(FeatureOptions),
    Dev(crate::dev::Options),
    Help,
    Version,
}

impl Args {
    fn parse(args: &[String]) -> Result<Args, String> {
        let Some(first) = args.first() else {
            return Ok(Args::Help);
        };
        match first.as_str() {
            "check" => Self::parse_check(&args[1..]),
            "new" => Self::parse_new(&args[1..]),
            "feature" => Self::parse_feature(&args[1..]),
            "dev" => Self::parse_dev(&args[1..]),
            "-h" | "--help" | "help" => Ok(Args::Help),
            "-V" | "--version" | "version" => Ok(Args::Version),
            other => Err(format!("unknown subcommand `{other}`")),
        }
    }

    fn parse_check(rest: &[String]) -> Result<Args, String> {
        let mut opts = Options::new(current_dir());
        let mut i = 0;
        while i < rest.len() {
            let flag = rest[i].clone();
            apply_check_flag(&mut opts, rest, &mut i, &flag)?;
            i += 1;
        }
        Ok(Args::Check(opts))
    }

    fn parse_new(rest: &[String]) -> Result<Args, String> {
        let mut name = None;
        let mut dir = PathBuf::from(".");
        let mut gabion_path = None;
        let mut i = 0;
        while i < rest.len() {
            match rest[i].as_str() {
                "--dir" => dir = PathBuf::from(value_for(rest, &mut i, "--dir")?),
                "--gabion-path" => {
                    gabion_path = Some(PathBuf::from(value_for(rest, &mut i, "--gabion-path")?));
                }
                flag if flag.starts_with('-') => return Err(format!("unknown flag `{flag}`")),
                value => name = Some(value.to_string()),
            }
            i += 1;
        }
        let name = name.ok_or("`gabion new` needs a name: gabion new myapp")?;
        if !is_crate_name(&name) {
            return Err(format!(
                "`{name}` is not a crate name: lower-case letters, digits, `-` or `_`"
            ));
        }
        Ok(Args::New(NewOptions {
            name,
            dir,
            gabion_path,
        }))
    }

    fn parse_feature(rest: &[String]) -> Result<Args, String> {
        let Some(sub) = rest.first() else {
            return Err(
                "`gabion feature` needs a subcommand: add <name> | remove <name>".to_string(),
            );
        };
        if sub != "add" && sub != "remove" {
            return Err(format!("unknown `feature` subcommand `{sub}`"));
        }
        let removing = sub == "remove";
        let name = rest
            .get(1)
            .ok_or("`gabion feature add` needs a name: gabion feature add billing")?;
        if name.starts_with('-') {
            return Err(format!("unknown flag `{name}`"));
        }
        let mut root = current_dir();
        let mut i = 2;
        while i < rest.len() {
            if rest[i] == "--root" {
                root = PathBuf::from(value_for(rest, &mut i, "--root")?);
            } else {
                return Err(format!("unknown flag `{}`", rest[i]));
            }
            i += 1;
        }
        let options = FeatureOptions {
            name: name.clone(),
            root,
        };
        if removing {
            return Ok(Args::FeatureRemove(options));
        }
        Ok(Args::FeatureAdd(options))
    }

    fn parse_dev(rest: &[String]) -> Result<Args, String> {
        let mut opts = crate::dev::Options::new(current_dir());
        let mut i = 0;
        while i < rest.len() {
            let flag = rest[i].clone();
            apply_dev_flag(&mut opts, rest, &mut i, &flag)?;
            i += 1;
        }
        Ok(Args::Dev(opts))
    }
}

/// One `gabion check` flag.
fn apply_check_flag(
    opts: &mut Options,
    rest: &[String],
    i: &mut usize,
    flag: &str,
) -> Result<(), String> {
    match flag {
        "--fast" => opts.fast = true,
        "--no-external" => opts.no_external = true,
        "--update-surfaces" => opts.update_surfaces = true,
        "--fix" => opts.fix = true,
        "--allow-missing-tools" => opts.allow_missing_tools = true,
        "--target" => opts.wasm_target = value_for(rest, i, "--target")?.to_string(),
        "--root" => opts.root = PathBuf::from(value_for(rest, i, "--root")?),
        other => return Err(format!("unknown flag `{other}`")),
    }
    Ok(())
}

/// One `gabion dev` flag. Split out so the parser reads as a loop over flags and
/// the flag table is one dispatch, which is also where a typo is caught.
fn apply_dev_flag(
    opts: &mut crate::dev::Options,
    rest: &[String],
    i: &mut usize,
    flag: &str,
) -> Result<(), String> {
    match flag {
        "--once" => opts.once = true,
        "--run" => set_option(&mut opts.run, rest, i, "--run")?,
        "--host" => set_option(&mut opts.host, rest, i, "--host")?,
        "--root" => opts.root = PathBuf::from(value_for(rest, i, "--root")?),
        "--interval" => set_interval(opts, rest, i)?,
        other => return Err(format!("unknown flag `{other}`")),
    }
    Ok(())
}

/// `--interval MS`.
fn set_interval(
    opts: &mut crate::dev::Options,
    rest: &[String],
    i: &mut usize,
) -> Result<(), String> {
    opts.interval = interval_of(value_for(rest, i, "--interval")?)?;
    Ok(())
}

/// Fill a string option from its flag's value.
fn set_option(
    slot: &mut Option<String>,
    rest: &[String],
    i: &mut usize,
    flag: &str,
) -> Result<(), String> {
    *slot = Some(value_for(rest, i, flag)?.to_string());
    Ok(())
}

/// The value of a flag, or which flag was missing one. One helper, so every
/// option parses its argument the same way and the branch budget goes to the
/// dispatch rather than to four copies of the same `?`.
fn value_for<'a>(rest: &'a [String], i: &mut usize, flag: &str) -> Result<&'a str, String> {
    *i += 1;
    rest.get(*i)
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// A crate name: lower-case letters, digits, `-` or `_`.
fn is_crate_name(name: &str) -> bool {
    const PUNCTUATION: [char; 2] = ['-', '_'];
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || PUNCTUATION.contains(&c))
}

/// `--interval` takes whole milliseconds.
fn interval_of(value: &str) -> Result<Duration, String> {
    value
        .parse::<u64>()
        .map(Duration::from_millis)
        .map_err(|_| "--interval takes milliseconds".to_string())
}

fn current_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn help() -> Summary {
    let text = "\
gabion — one business core, every surface

  gabion new <name> [--dir PATH] [--gabion-path PATH]
  gabion feature add <name> [--root PATH]
  gabion feature remove <name> [--root PATH]
  gabion dev [--once] [--run CMD] [--host PKG] [--interval MS] [--root PATH]
  gabion check [--fast] [--no-external] [--update-surfaces] [--fix]
               [--allow-missing-tools] [--root PATH] [--target TRIPLE]
  gabion --help | --version

  static rules (always):
    edge table        process wall, feature wall, upward leak, host purity
    core deps         a *-core may not name a runtime, server, client or store
    clippy.toml       required effect bans and thresholds are still present
    workspace lints   unsafe forbidden, clippy denies, every member opted in
    surfaces          .gabion/surfaces.lock matches the reachable method set
    sources           comment rule, silent defaults, #[allow] placement,
                      over-broad visibility, file length, branch caps, TODOs

  subprocess gates:
    fmt, clippy (-D warnings), test          (cargo)
    shear, deny                              (--no-external to skip)
    wasm build of every *-core

  --fast              static rules only, no subprocesses
  --update-surfaces   rewrite the surface lock (the 'I meant to publish that' gesture)
  --fix               apply the mechanical findings (pub -> pub(crate), the lock)
                      and re-run; what is left needs a decision
  --allow-missing-tools  report a missing cargo-shear/cargo-deny as a named SKIP
                      instead of a failure. Never silent: the summary counts them.
";
    eprint!("{text}");
    Summary::silent()
}

/// The `--version` line, on stdout: this is the one line a script or a bug
/// report captures, where the help text is a human reading a terminal.
fn version() -> Summary {
    println!("{}", version_line());
    Summary::silent()
}

/// The version string, split out so a test can assert on the exact text without
/// capturing the process's own stdout. The binary is `gabion` even though the
/// crate is `gabion-cli`, so the line names the command a person typed.
#[must_use]
pub fn version_line() -> String {
    format!("gabion {}", env!("CARGO_PKG_VERSION"))
}

fn usage_error(message: &str) -> Summary {
    eprintln!("gabion: {message} (try `gabion --help`)");
    Summary {
        skipped: Vec::new(),
        notes: Vec::new(),
        findings: Vec::new(),
        steps: Vec::new(),
        failed_steps: vec![format!("usage: {message}")],
    }
}
