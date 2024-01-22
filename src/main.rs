#![allow(dead_code)]

use std::{net::SocketAddr, str::FromStr};

use auth::{AuthLayer, KeyPool};
use forward_request::ForwardRequestLayer;
use http::{
    header::{AUTHORIZATION, HOST},
    Uri,
};
use hyper::{Client, Request, Server};
use hyper_tls::HttpsConnector;
use read_request_body::ReadRequestLayer;
use rename_header::RenameHeaderLayer;
use request_id::MakeIntRequestId;
use retry::{ExponentialBackoff, WithBackoff};
use tower::{make::Shared, retry::RetryLayer, util::MapRequestLayer, BoxError, ServiceBuilder};
use tower_http::{
    trace::{DefaultMakeSpan, DefaultOnRequest, TraceLayer},
    ServiceBuilderExt,
};
use tracing::Level;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod auth;
mod forward_request;
mod read_request_body;
mod rename_header;
mod request_id;
mod retry;
mod rng;

const X_BALENA_AUTHORIZATION: &str = "x-balena-authorization";
const BALENA_API_KEY: &str = "BALENA_API_KEY";

// Balena does not like host header
fn without_host_header<B>(mut req: Request<B>) -> Request<B> {
    req.headers_mut().remove(HOST);
    req
}

// fn debug_request<B: std::fmt::Debug>(req: Request<B>) -> Request<B> {
//     tracing::log::trace!("{:?}", req);
//     req
// }

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "proxy=trace,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(DefaultMakeSpan::new().include_headers(true))
        .on_request(DefaultOnRequest::new().level(Level::INFO));

    // let trace_layer = init_tracing();

    let balena_api_key =
        std::env::var(BALENA_API_KEY).unwrap_or_else(|err| panic!("{}: {}", err, BALENA_API_KEY));
    let keys = KeyPool::from(balena_api_key.split(',').collect::<Vec<&str>>());
    let retry_policy = WithBackoff::new(3, ExponentialBackoff::default());
    let forward_uri = Uri::from_str("https://api.balena-cloud.com/v6").unwrap();

    // Use tower's `ServiceBuilder` API to build a stack of tower middleware
    // wrapping our request handler.
    let service = ServiceBuilder::new()
        .set_x_request_id(MakeIntRequestId::default())
        // next layer reads streaming request body before we proceed,
        // we need it to get retry layer work as it clones request.
        .layer(ReadRequestLayer::new())
        .layer(trace_layer)
        .layer(RenameHeaderLayer::new(
            X_BALENA_AUTHORIZATION,
            AUTHORIZATION,
        ))
        .layer(MapRequestLayer::new(without_host_header)) // Balena does not like host header
        .layer(ForwardRequestLayer::new(forward_uri))
        // .layer(MapRequestBodyLayer::new(BufBody::new))
        .layer(RetryLayer::new(retry_policy)) // retry request if failed
        // assign balena api key if missing, rotate key on 429, remove key on 401
        .layer(AuthLayer::new(keys))
        // .layer(MapRequestLayer::new(debug_request)) // print request
        .propagate_x_request_id()
        .service(Client::builder().build(HttpsConnector::new()));

    // And run our service using `hyper`
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    Server::bind(&addr)
        .serve(Shared::new(service))
        .await
        .expect("server error");

    Ok(())
}
