//! Unit tests for the contract parser. These exercise `expand` directly: the
//! `proc_macro` entry point cannot be called from a test, but everything it
//! does is delegate here.

use super::*;

fn expand_src(body: &str) -> syn::Result<proc_macro2::TokenStream> {
    expand(syn::parse_str::<ItemTrait>(body)?)
}

fn parse_attr(attr: &str) -> proc_macro2::TokenStream {
    match attr.parse() {
        Ok(t) => t,
        Err(e) => panic!("bad test attribute {attr:?}: {e}"),
    }
}

fn err_of(body: &str) -> String {
    match expand_src(body) {
        Ok(_) => String::new(),
        Err(e) => e.to_string(),
    }
}

const CLEAN: &str = "pub trait TodoService {
    async fn create(&self, ctx: &Ctx, input: CreateTodo) -> Result<Todo, TodoErr>;
    async fn list(&self, ctx: &Ctx, _input: ()) -> Result<Vec<Todo>, TodoErr>;
}";

#[test]
fn every_method_in_the_trait_reaches_every_surface() {
    let out = match expand_src(CLEAN) {
        Ok(t) => t.to_string(),
        Err(e) => panic!("clean contract must expand: {e}"),
    };
    // One HTTP handler per method, no method left behind.
    assert!(out.contains("__h_create"), "no HTTP handler for create");
    assert!(out.contains("__h_list"), "no HTTP handler for list");
}

/// A run of capitals is one word, not one word per letter: `HTTPService` becomes
/// the namespace `http`, not `h_t_t_p`. The namespace is the route prefix, the
/// MCP tool prefix, the `ns` on every frame and the stream-wall key, so the
/// spelling here decides what an HTTP client has to type.
#[test]
fn an_acronym_is_one_word_not_one_word_per_letter() {
    assert_eq!(to_snake("HTTP"), "http");
    assert_eq!(to_snake("HTTPApi"), "http_api");
    assert_eq!(to_snake("TodoId"), "todo_id");
    assert_eq!(to_snake("Todo"), "todo");
}

/// Digits are not letters, so they neither start nor end a word: `H2O` breaks
/// before the trailing `O` because that is a capital following a digit, and
/// `H2` stays `h2` because a digit does not separate anything from its prefix.
#[test]
fn digits_do_not_start_or_end_a_word() {
    assert_eq!(to_snake("H2O"), "h2_o");
    assert_eq!(to_snake("H2"), "h2");
    assert_eq!(to_snake("Todo2Id"), "todo2_id");
}

/// The whole point, at the level the issue cares about: an acronym trait must
/// generate the routes and tool names an author can actually type. `h_t_t_p.list`
/// is a wire surface nobody gets right, and the mistake stays invisible until an
/// HTTP client 404s.
#[test]
fn an_acronym_trait_names_its_routes_with_the_whole_word() {
    let text = match expand_src(
        "pub trait HTTPService {
        async fn list(&self, ctx: &Ctx, _input: ()) -> Result<Vec<Todo>, TodoErr>;
    }",
    ) {
        Ok(t) => t.to_string(),
        Err(e) => panic!("an acronym trait name is legal Rust and must expand: {e}"),
    };
    assert!(
        text.contains("/api/http.list"),
        "the route must be `/api/http.list`, got: {text}"
    );
    assert!(
        !text.contains("h_t_t_p"),
        "an acronym must never become one letter per word, got: {text}"
    );
}

#[test]
fn retired_surface_attribute_is_a_hard_error() {
    // Silently ignoring this would leave an author believing they hid a
    // method while it stayed public over HTTP.
    let msg = err_of(
        "pub trait S {
        #[surface(cli)]
        async fn a(&self, _i: ()) -> Result<(), ()>;
    }",
    );
    assert!(
        msg.contains("surface attributes are gone"),
        "expected the retired-attribute error, got: {msg}"
    );
}

#[test]
fn retired_internal_attribute_is_a_hard_error() {
    let msg = err_of(
        "pub trait S {
        #[internal]
        async fn a(&self, _i: ()) -> Result<(), ()>;
    }",
    );
    assert!(
        msg.contains("surface attributes are gone"),
        "expected the retired-attribute error, got: {msg}"
    );
}

#[test]
fn service_takes_no_arguments() {
    assert!(reject_service_args(&proc_macro2::TokenStream::new()).is_ok());
    let err = reject_service_args(&parse_attr("http"));
    assert!(
        err.is_err(),
        "#[service(http)] must be rejected, not ignored"
    );
}

#[test]
fn one_input_argument_is_enforced() {
    let msg = err_of(
        "pub trait S {
        async fn a(&self, ctx: &Ctx, one: A, two: B) -> Result<(), ()>;
    }",
    );
    assert!(
        msg.contains("exactly one input argument"),
        "expected the arity error, got: {msg}"
    );
}

#[test]
fn a_method_without_a_context_is_a_hard_error() {
    // This is the whole point of the parameter: a method whose surface cannot
    // name its caller is a method that cannot be authorized.
    let msg = err_of(
        "pub trait S {
        async fn a(&self, input: A) -> Result<(), ()>;
    }",
    );
    assert!(
        msg.contains("takes `ctx: &Ctx` first"),
        "expected the missing-ctx error, got: {msg}"
    );
}

#[test]
fn a_qualified_context_type_is_accepted() {
    // `&gabion::Ctx` and `&crate::Ctx` are the same convention; keying on the
    // bare spelling would be a wall that depends on how someone imported it.
    let out = expand_src(
        "pub trait S {
        async fn a(&self, ctx: &::gabion::Ctx) -> Result<(), ()>;
    }",
    );
    assert!(out.is_ok(), "a qualified Ctx must expand");
}

#[test]
fn a_unit_input_is_still_allowed_after_the_context() {
    let out = expand_src(
        "pub trait S {
        async fn a(&self, ctx: &Ctx, _input: ()) -> Result<(), ()>;
    }",
    );
    assert!(out.is_ok());
}

#[test]
fn a_streaming_return_without_a_policy_is_a_hard_error() {
    // Every surface assumes one value: schemars schemas it, the client parses one
    // body, the CLI renders one. And there is no default policy to fall back on,
    // because the policy selects the loop the host runs. Failing here beats an
    // E0038 from generated code and beats picking a flow rule nobody chose.
    let msg = err_of(
        "pub trait S {
        async fn watch(&self, ctx: &Ctx) -> Result<impl Stream<Item = u8>, E>;
    }",
    );
    assert!(
        msg.contains("an unmarked stream return is refused"),
        "expected the unmarked-stream refusal, got: {msg}"
    );
    assert!(
        msg.contains("Each<T>") && msg.contains("Latest<T>"),
        "the refusal must name the two markers that exist, got: {msg}"
    );
}

/// A `BoxStream` is dyn-compatible and spells nothing the token scan already knew,
/// so without this it reaches the value generators and the author gets a schemars
/// failure for a decision they never made.
#[test]
fn a_stream_marker_that_is_not_each_or_latest_is_a_hard_error() {
    let msg = err_of(
        "pub trait S {
        async fn watch(&self, ctx: &Ctx) -> Result<BoxStream<'static, Frame>, E>;
    }",
    );
    assert!(
        msg.contains("an unmarked stream return is refused"),
        "expected the unmarked-stream refusal, got: {msg}"
    );
}

/// A marker with nothing inside it has no item type to decode an `item` frame into.
#[test]
fn a_marker_with_no_item_type_is_a_hard_error() {
    let msg = err_of(
        "pub trait S {
        async fn watch(&self, ctx: &Ctx) -> Result<Each, E>;
    }",
    );
    assert!(
        msg.contains("take the item type"),
        "expected the malformed-marker refusal, got: {msg}"
    );
}

/// The marker is what makes a listen op a listen op, and it is a named type in the
/// signature, so `.gabion/surfaces.lock` hashes it. A policy carried by an
/// attribute could flip and change the loop every surface runs without moving the
/// lock, which is the whole reason the policy is a type.
#[test]
fn a_policy_marker_makes_the_op_a_listen_op_on_every_surface() {
    let out = expand_src(
        "pub trait S {
        async fn watch(&self, ctx: &Ctx) -> Result<Each<Frame>, E>;
    }",
    );
    let text = match out {
        Ok(t) => t.to_string(),
        Err(e) => panic!("a marked stream return must expand: {e}"),
    };
    // The stream adapter exists, and it reports the policy the signature chose.
    assert!(text.contains("__s_ws"), "no stream adapter was generated");
    assert!(
        text.contains(":: gabion :: ws :: Policy :: Each"),
        "the adapter must report `Each`, not infer one"
    );
    // The op is absent from the ordinary HTTP routes: one value per response is
    // not a shape a stream fits.
    assert!(
        !text.contains("__h_watch"),
        "a listen op must not get an HTTP route"
    );
}

/// A feature with no listen op generates no stream adapter at all, so it mounts no
/// stream surface and the host gains no `/api/ws` route. That is the
/// "byte-identical to today" half of docs/streaming.md §5, made structural.
#[test]
fn a_feature_that_never_streams_generates_no_stream_adapter() {
    let text = match expand_src(CLEAN) {
        Ok(t) => t.to_string(),
        Err(e) => panic!("a clean contract must expand: {e}"),
    };
    assert!(
        !text.contains("__todo_ws"),
        "a feature with no listen op must not gain a stream adapter"
    );
}

#[test]
fn a_type_that_merely_contains_the_word_stream_is_fine() {
    let out = expand_src(
        "pub trait S {
        async fn read(&self, ctx: &Ctx) -> Result<StreamConfig, E>;
    }",
    );
    assert!(out.is_ok(), "a type named StreamConfig is not a stream");
}

/// A feature that streams moves its ordinary ops onto the socket once a stream is
/// open (docs/streaming.md §5). The generated client is what does the moving, so
/// the branch has to be in the code it writes — not in a runtime knob an app could
/// forget to turn on.
#[test]
fn a_streaming_feature_routes_its_ordinary_ops_through_the_socket() {
    let streamed = expand_src(
        "pub trait S {
        async fn ping(&self, ctx: &Ctx) -> Result<Frame, E>;
        async fn watch(&self, ctx: &Ctx) -> Result<Each<Frame>, E>;
    }",
    );
    let text = match streamed {
        Ok(t) => t.to_string(),
        Err(e) => panic!("a marked stream return must expand: {e}"),
    };
    assert!(
        text.contains("has_open_stream"),
        "an ordinary op of a streaming feature must check the connection first"
    );

    // A feature that never streams must not grow the check at all: its client is
    // byte-identical to the one written before streams existed.
    let plain = expand_src(
        "pub trait S {
        async fn ping(&self, ctx: &Ctx) -> Result<Frame, E>;
    }",
    );
    let plain = match plain {
        Ok(t) => t.to_string(),
        Err(e) => panic!("a clean contract must expand: {e}"),
    };
    assert!(
        !plain.contains("has_open_stream"),
        "a feature with no listen op must keep the plain POST client"
    );
}

/// `Surfaces.routes` is what a host logs as the HTTP surface it serves, so it has
/// to be the routes the router actually registers. A listen op has no
/// `POST /api/<ns>.<op>` — it is served on `GET /api/ws` and named in the OpenAPI
/// document's `x-gabion-streams` — so listing it here would be a report of success
/// over a route nobody serves.
#[test]
fn a_listen_op_is_not_advertised_as_a_post_route() {
    let out = expand_src(
        "pub trait S {
        async fn ping(&self, ctx: &Ctx) -> Result<Frame, E>;
        async fn watch(&self, ctx: &Ctx) -> Result<Each<Frame>, E>;
    }",
    );
    let text = match out {
        Ok(t) => t.to_string(),
        Err(e) => panic!("a mixed contract must expand: {e}"),
    };
    assert!(
        text.contains("\"/api/s.ping\""),
        "the value op's route must be advertised"
    );
    assert!(
        !text.contains("/api/s.watch"),
        "a listen op has no POST route, so it must not be advertised as one"
    );
    // The op is not gone: it is a stream, and the document says so.
    assert!(
        text.contains("doc . stream"),
        "the listen op must still reach x-gabion-streams"
    );
}
