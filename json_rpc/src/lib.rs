//! # casper-json-rpc
//!
//! A library suitable for use as the framework for a JSON-RPC server.
//!
//! # Usage
//!
//! Normally usage will involve two steps:
//!   * construct a set of request handlers using a [`RequestHandlersBuilder`]
//!   * call [`casper_json_rpc::route`](route) to construct a boxed warp filter ready to be passed
//!     to [`warp::service`](https://docs.rs/warp/latest/warp/fn.service.html) for example
//!
//! # Example
//!
//! ```no_run
//! use casper_json_rpc::{Error, Params, RequestHandlersBuilder};
//! use std::{convert::Infallible, sync::Arc};
//!
//! # #[allow(unused)]
//! async fn get(params: Option<Params>) -> Result<String, Error> {
//!     // * parse params or return `ReservedErrorCode::InvalidParams` error
//!     // * handle request and return result
//!     Ok("got it".to_string())
//! }
//!
//! # #[allow(unused)]
//! async fn put(params: Option<Params>, other_input: &str) -> Result<String, Error> {
//!     Ok(other_input.to_string())
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     // Register handlers for methods "get" and "put".
//!     let mut handlers = RequestHandlersBuilder::new();
//!     handlers.register_handler("get", Arc::new(get));
//!     let put_handler = move |params| async move { put(params, "other input").await };
//!     handlers.register_handler("put", Arc::new(put_handler));
//!     let handlers = handlers.build();
//!
//!     // Get the new route.
//!     let path = "rpc";
//!     let max_body_bytes = 1024;
//!     let allow_unknown_fields = false;
//!     let route = casper_json_rpc::route(path, max_body_bytes, handlers, allow_unknown_fields, None);
//!
//!     // Convert it into a `Service` and run it.
//!     let make_svc = hyper::service::make_service_fn(move |_| {
//!         let svc = warp::service(route.clone());
//!         async move { Ok::<_, Infallible>(svc.clone()) }
//!     });
//!
//!     hyper::Server::bind(&([127, 0, 0, 1], 3030).into())
//!         .serve(make_svc)
//!         .await
//!         .unwrap();
//! }
//! ```
//!
//! # Errors
//!
//! To return a JSON-RPC response indicating an error, use [`Error::new`].  Most error conditions
//! which require returning a reserved error are already handled in the provided warp filters.  The
//! only exception is [`ReservedErrorCode::InvalidParams`] which should be returned by any RPC
//! handler which deems the provided `params: Option<Params>` to be invalid for any reason.
//!
//! Generally a set of custom error codes should be provided.  These should all implement
//! [`ErrorCodeT`].

#![doc(html_root_url = "https://docs.rs/casper-json-rpc/1.1.0")]
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/casper-network/casper-node/master/images/CasperLabs_Logo_Favicon_RGB_50px.png",
    html_logo_url = "https://raw.githubusercontent.com/casper-network/casper-node/master/images/CasperLabs_Logo_Symbol_RGB.png",
    test(attr(deny(warnings)))
)]
#![warn(
    missing_docs,
    trivial_casts,
    trivial_numeric_casts,
    unused_qualifications
)]

mod error;
pub mod filters;
mod rejections;
mod request;
mod request_handlers;
mod response;

use http::{header::CONTENT_TYPE, Method};
use warp::{filters::BoxedFilter, Filter, Reply};

pub use error::{Error, ErrorCodeT, ReservedErrorCode};
pub use request::Params;
pub use request_handlers::{RequestHandlers, RequestHandlersBuilder};
pub use response::Response;

const JSON_RPC_VERSION: &str = "2.0";

/// Specifies the CORS origin
#[derive(Debug)]
pub enum CorsOrigin {
    /// Any (*) origin is allowed.
    Any,
    /// Only the specified origin is allowed.
    Specified(String),
}

impl CorsOrigin {
    /// Converts the [`CorsOrigin`] into a CORS [`Builder`](warp::cors::Builder).
    #[inline]
    pub fn to_cors_builder(&self) -> warp::cors::Builder {
        match self {
            CorsOrigin::Any => warp::cors().allow_any_origin(),
            CorsOrigin::Specified(origin) => warp::cors().allow_origin(origin.as_str()),
        }
    }

    /// Parses a [`CorsOrigin`] from a given configuration string.
    ///
    /// The input string will be parsed as follows:
    ///
    /// * `""` (empty string): No CORS Origin (i.e. returns [`None`]).
    /// * `"*"`: [`CorsOrigin::Any`].
    /// * otherwise, returns `CorsOrigin::Specified(raw)`.
    #[inline]
    pub fn from_str<T: ToString + AsRef<str>>(raw: T) -> Option<Self> {
        match raw.as_ref() {
            "" => None,
            "*" => Some(CorsOrigin::Any),
            _ => Some(CorsOrigin::Specified(raw.to_string())),
        }
    }
}

/// Constructs a set of warp filters suitable for use in a JSON-RPC server.
///
/// `path` specifies the exact HTTP path for JSON-RPC requests, e.g. "rpc" will match requests on
/// exactly "/rpc", and not "/rpc/other".
///
/// `max_body_bytes` sets an upper limit for the number of bytes in the HTTP request body.  For
/// further details, see
/// [`warp::filters::body::content_length_limit`](https://docs.rs/warp/latest/warp/filters/body/fn.content_length_limit.html).
///
/// `handlers` is the map of functions to which incoming requests will be dispatched.  These are
/// keyed by the JSON-RPC request's "method".
///
/// If `allow_unknown_fields` is `false`, requests with unknown fields will cause the server to
/// respond with an error.
///
/// If `cors_header` is `Some`, it is used to add a [a warp CORS
/// filter](https://docs.rs/warp/latest/warp/filters/cors/index.html) which
///
///   * allows any origin or specified origin
///   * allows "content-type" as a header
///   * allows the method "POST"
///
/// For further details, see the docs for the [`filters`] functions.
pub fn route<P: AsRef<str>>(
    path: P,
    max_body_bytes: u32,
    handlers: RequestHandlers,
    allow_unknown_fields: bool,
    cors_header: Option<&CorsOrigin>,
) -> BoxedFilter<(Box<dyn Reply>,)> {
    let base = filters::base_filter(path, max_body_bytes)
        .and(filters::main_filter(handlers, allow_unknown_fields))
        .recover(filters::handle_rejection);

    if let Some(cors_origin) = cors_header {
        let cors = cors_origin
            .to_cors_builder()
            .allow_header(CONTENT_TYPE)
            .allow_method(Method::POST)
            .build();
        base.with(cors).map(box_reply).boxed()
    } else {
        base.map(box_reply).boxed()
    }
}

/// Boxes a reply of a warp filter.
///
/// Can be combined with [`Filter::boxed`] through [`Filter::map`] to erase the type on filters:
///
/// ```rust
/// use warp::{Filter, filters::BoxedFilter, http::Response, reply::Reply};
///# use casper_json_rpc::box_reply;
///
/// let filter: BoxedFilter<(Box<dyn Reply>,)> = warp::any()
///                .map(|| Response::builder().body("hello world"))
///                .map(box_reply).boxed();
///# drop(filter);
/// ```
#[inline(always)]
pub fn box_reply<T: Reply + 'static>(reply: T) -> Box<dyn Reply> {
    let boxed: Box<dyn Reply> = Box::new(reply);
    boxed
}
