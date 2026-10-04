#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration test: the CORS allowlist is inert by default and exact when set.

use std::net::SocketAddr;

use reqwest::{Method, Response};

mod common;

const ALLOWED_ORIGIN: &str = "http://127.0.0.1:3000";
const OTHER_ORIGIN: &str = "http://evil.example";

async fn start_server(allowed_origins: &[&str]) -> SocketAddr {
    common::start_server_with_config(
        common::TestServerConfigBuilder::new(tempfile::tempdir().unwrap().keep())
            .cors_allowed_origins(allowed_origins)
            .build(),
    )
    .await
}

async fn get_health_from(addr: SocketAddr, origin: &str) -> Response {
    reqwest::Client::new()
        .get(format!("http://{addr}/health"))
        .header("origin", origin)
        .send()
        .await
        .unwrap()
}

async fn preflight_login_from(addr: SocketAddr, origin: &str) -> Response {
    reqwest::Client::new()
        .request(Method::OPTIONS, format!("http://{addr}/auth/login"))
        .header("origin", origin)
        .header("access-control-request-method", "POST")
        .header(
            "access-control-request-headers",
            "authorization,content-type",
        )
        .send()
        .await
        .unwrap()
}

fn header<'a>(resp: &'a Response, name: &str) -> Option<&'a str> {
    resp.headers().get(name).map(|v| v.to_str().unwrap())
}

#[tokio::test]
async fn no_cors_headers_are_sent_when_no_origin_is_allowed() {
    let addr = start_server(&[]).await;

    let resp = get_health_from(addr, ALLOWED_ORIGIN).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(header(&resp, "access-control-allow-origin"), None);

    let resp = preflight_login_from(addr, ALLOWED_ORIGIN).await;
    assert_eq!(header(&resp, "access-control-allow-origin"), None);
}

#[tokio::test]
async fn a_listed_origin_is_answered_with_its_own_allow_origin() {
    let addr = start_server(&[ALLOWED_ORIGIN]).await;
    let resp = get_health_from(addr, ALLOWED_ORIGIN).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        header(&resp, "access-control-allow-origin"),
        Some(ALLOWED_ORIGIN)
    );
    assert_eq!(header(&resp, "access-control-allow-credentials"), None);
}

#[tokio::test]
async fn an_unlisted_origin_gets_no_allow_origin() {
    let addr = start_server(&[ALLOWED_ORIGIN]).await;
    let resp = get_health_from(addr, OTHER_ORIGIN).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(header(&resp, "access-control-allow-origin"), None);

    let resp = preflight_login_from(addr, OTHER_ORIGIN).await;
    assert_eq!(header(&resp, "access-control-allow-origin"), None);
}

#[tokio::test]
async fn a_listed_origin_preflight_allows_the_bearer_call_without_credentials() {
    let addr = start_server(&[ALLOWED_ORIGIN]).await;
    let resp = preflight_login_from(addr, ALLOWED_ORIGIN).await;
    assert!(resp.status().is_success());
    assert_eq!(
        header(&resp, "access-control-allow-origin"),
        Some(ALLOWED_ORIGIN)
    );
    let allowed_methods = header(&resp, "access-control-allow-methods").unwrap();
    for method in ["GET", "POST", "PATCH", "DELETE", "OPTIONS"] {
        assert!(
            allowed_methods.contains(method),
            "{method} in {allowed_methods}"
        );
    }
    let allowed_headers = header(&resp, "access-control-allow-headers")
        .unwrap()
        .to_ascii_lowercase();
    for name in ["authorization", "content-type", "accept"] {
        assert!(
            allowed_headers.contains(name),
            "{name} in {allowed_headers}"
        );
    }
    assert_eq!(header(&resp, "access-control-allow-credentials"), None);
}
