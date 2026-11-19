//! Argument parsing, kept out of main so the whole tool is testable as a lib.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::report::Summary;
use crate::Options;

/// Parse argv and run the named subcommand.
pub fn run(args: &[String]) -> (Summary, ExitCode) {
    match Args::parse(args) {
        Ok(Args::Check(opts)) => {
            let summary = crate::check(&opts);
            let code = if summary.ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
            (summary, code)
        }
        Ok(Args::Help) => (help(), ExitCode::SUCCESS),
        Err(message) => (usage_error(&message), ExitCode::FAILURE),
    }
}

enum Args {
    Check(Options),
    Help,
}

impl Args {
    fn parse(args: &[String]) -> Result<Args, String> {
        let Some(first) = args.first() else {
            return Ok(Args::Help);
        };
        match first.as_str() {
            "check" => Self::parse_check(&args[1..]),
            "-h" | "--help" | "help" => Ok(Args::Help),
            other => Err(format!("unknown subcommand `{other}`")),
        }
    }

    fn parse_check(rest: &[String]) -> Result<Args, String> {
        let mut opts = Options::new(current_dir());
        let mut i = 0;
        while i < rest.len() {
            match rest[i].as_str() {
                "--fast" => opts.fast = true,
                "--no-external" => opts.no_external = true,
                "--target" => {
                    i += 1;
                    let target = rest.get(i).ok_or("--target needs a value")?;
                    opts.wasm_target = target.clone();
                }
                "--root" => {
                    i += 1;
                    let root = rest.get(i).ok_or("--root needs a value")?;
                    opts.root = PathBuf::from(root);
                }
                other => return Err(format!("unknown flag `{other}`")),
            }
            i += 1;
        }
        Ok(Args::Check(opts))
    }
}

fn current_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn help() -> Summary {
    let text = "\
gabion — one business core, four surfaces

  gabion check [--fast] [--no-external] [--root PATH] [--target TRIPLE]

  static rules (always):
    edge table        process wall, feature wall, upward leak, host purity
    clippy.toml       required effect bans and thresholds are still present
    sources           #[surface] completeness, comment rule, silent defaults,
                      #[allow] placement, file length, TODOs

  subprocess gates:
    fmt, clippy (-D warnings), test          (cargo)
    shear, deny                              (--no-external to skip)
    wasm build of every *-core               (the wall proof)

  --fast  static rules only, no subprocesses
";
    eprint!("{text}");
    Summary {
        findings: Vec::new(),
        steps: Vec::new(),
        failed_steps: Vec::new(),
    }
}

fn usage_error(message: &str) -> Summary {
    eprintln!("gabion: {message} (try `gabion check --help`)");
    Summary {
        findings: Vec::new(),
        steps: Vec::new(),
        failed_steps: vec![format!("usage: {message}")],
    }
}
