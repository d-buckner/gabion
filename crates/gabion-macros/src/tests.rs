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
fn a_streaming_return_is_a_hard_error() {
    // Every surface assumes one value: schemars schemas it, the client parses one
    // body, the CLI renders one. Failing here beats an E0038 from generated code.
    let msg = err_of(
        "pub trait S {
        async fn watch(&self, ctx: &Ctx) -> Result<impl Stream<Item = u8>, E>;
    }",
    );
    assert!(
        msg.contains("streaming returns are not supported"),
        "expected the streaming error, got: {msg}"
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
