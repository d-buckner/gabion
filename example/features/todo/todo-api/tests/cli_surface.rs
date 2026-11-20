//! The generated CLI surface, run as generated code rather than as a template
//! string. gabion's own source rules do not scan `tests/`, and the `cli`
//! feature is what makes the generated module exist at all.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use gabion::prelude::*;

/// A success value whose `Serialize` impl always fails. The generated dispatch
/// used to swallow that error: empty stdout and exit code 0, which reads as a
/// successful call to every caller, script, and agent in the chain.
#[derive(JsonSchema)]
pub struct Broken;

impl gabion::serde::Serialize for Broken {
    fn serialize<S: gabion::serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(gabion::serde::ser::Error::custom(
            "this value cannot be written",
        ))
    }
}

#[service]
pub trait ProbeService {
    async fn read(&self, ctx: &Ctx, _input: ()) -> Result<Broken, ServiceError<()>>;
}

pub struct Probe;

#[async_trait]
impl ProbeService for Probe {
    async fn read(&self, _ctx: &Ctx, _input: ()) -> Result<Broken, ServiceError<()>> {
        Ok(Broken)
    }
}

#[test]
fn a_success_value_that_will_not_serialize_fails_the_call() {
    let matches = __probe_cli::probe_cli_command().get_matches_from(["probe", "read"]);
    let outcome =
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(__probe_cli::probe_cli_dispatch(
                &Probe,
                &Ctx::anonymous(),
                &matches,
            ));
    match outcome {
        Ok(text) => panic!("a value that cannot serialize must not look like success: {text:?}"),
        Err(e) => {
            assert_eq!(e.code, gabion::error::ErrorKind::Internal.exit_code());
            assert!(
                e.message.contains("serialize"),
                "the message must name the failure: {}",
                e.message
            );
        }
    }
}
