#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration test: the server serves the built web UI on its own origin.

use std::net::SocketAddr;
use std::path::Path;

use reqwest::{Method, Response};

mod common;

const INDEX_HTML: &str = "<!doctype html><title>mmcp web UI</title>";
const IMMUTABLE_ASSET_PATH: &str = "/_app/immutable/app.js";
const IMMUTABLE_ASSET_BODY: &str = "export {};";
const SECRET_OUTSIDE_THE_WEBUI_DIR: &str = "outside-the-served-directory";

/// Every API prefix a request must never answer with the web UI under.
const API_PREFIXES: [&str; 4] = ["/mcp", "/auth", "/sync", "/git"];

/// Writes a minimal built web UI into `dir`.
fn write_webui(dir: &Path) {
    std::fs::create_dir_all(dir.join("_app/immutable")).unwrap();
    std::fs::write(dir.join("index.html"), INDEX_HTML).unwrap();
    std::fs::write(dir.join("_app/immutable/app.js"), IMMUTABLE_ASSET_BODY).unwrap();
}

/// Starts a server serving the web UI written under a fresh `webui` directory.
/// A `secret.txt` sits beside that directory, outside what the server may serve.
async fn start_server_with_webui() -> SocketAddr {
    let root = tempfile::tempdir().unwrap().keep();
    let webui_dir = root.join("webui");
    write_webui(&webui_dir);
    std::fs::write(root.join("secret.txt"), SECRET_OUTSIDE_THE_WEBUI_DIR).unwrap();
    common::start_server_with_config(
        common::TestServerConfigBuilder::new(root.join("repos"))
            .webui_dir(webui_dir)
            .build(),
    )
    .await
}

async fn start_server_without_webui() -> SocketAddr {
    common::start_server_with_config(
        common::TestServerConfigBuilder::new(tempfile::tempdir().unwrap().keep()).build(),
    )
    .await
}

fn header<'a>(resp: &'a Response, name: &str) -> Option<&'a str> {
    resp.headers().get(name).map(|v| v.to_str().unwrap())
}

async fn request(addr: SocketAddr, method: Method, path: &str) -> Response {
    reqwest::Client::new()
        .request(method, format!("http://{addr}{path}"))
        .send()
        .await
        .unwrap()
}

fn is_html(resp: &Response) -> bool {
    header(resp, "content-type").is_some_and(|content_type| content_type.starts_with("text/html"))
}

#[tokio::test]
async fn root_serves_the_index_with_revalidation_and_security_headers() {
    let addr = start_server_with_webui().await;
    let resp = request(addr, Method::GET, "/").await;
    assert_eq!(resp.status(), 200);
    assert!(is_html(&resp));
    assert_eq!(header(&resp, "cache-control"), Some("no-cache"));
    assert_eq!(header(&resp, "x-content-type-options"), Some("nosniff"));
    assert_eq!(header(&resp, "referrer-policy"), Some("no-referrer"));
    assert_eq!(
        header(&resp, "content-security-policy"),
        Some("frame-ancestors 'none'")
    );
    assert_eq!(resp.text().await.unwrap(), INDEX_HTML);
}

#[tokio::test]
async fn client_side_routes_fall_back_to_the_index_with_the_same_headers() {
    let addr = start_server_with_webui().await;
    for path in ["/login", "/groups/0190b2a0", "/no/such/page"] {
        let resp = request(addr, Method::GET, path).await;
        assert_eq!(resp.status(), 200, "{path}");
        assert_eq!(header(&resp, "cache-control"), Some("no-cache"), "{path}");
        assert_eq!(
            header(&resp, "content-security-policy"),
            Some("frame-ancestors 'none'"),
            "{path}"
        );
        assert_eq!(resp.text().await.unwrap(), INDEX_HTML, "{path}");
    }
}

#[tokio::test]
async fn hashed_assets_are_cached_for_a_year_as_immutable() {
    let addr = start_server_with_webui().await;
    let resp = request(addr, Method::GET, IMMUTABLE_ASSET_PATH).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        header(&resp, "cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert_eq!(header(&resp, "x-content-type-options"), Some("nosniff"));
    assert_eq!(header(&resp, "referrer-policy"), Some("no-referrer"));
    assert_eq!(header(&resp, "content-security-policy"), None);
    assert_eq!(resp.text().await.unwrap(), IMMUTABLE_ASSET_BODY);
}

#[tokio::test]
async fn unknown_paths_under_every_api_prefix_answer_a_non_html_404() {
    let addr = start_server_with_webui().await;
    for prefix in API_PREFIXES {
        for path in [
            prefix.to_string(),
            format!("{prefix}/"),
            format!("{prefix}/no-such-route"),
            format!("{prefix}/no-such-route/deeper"),
        ] {
            for method in [Method::GET, Method::POST] {
                let resp = request(addr, method.clone(), &path).await;
                assert_eq!(resp.status(), 404, "{method} {path}");
                assert!(!is_html(&resp), "{method} {path} answered HTML");
                let body = resp.text().await.unwrap();
                assert!(
                    !body.contains(INDEX_HTML),
                    "{method} {path} answered the index"
                );
            }
        }
    }
}

#[tokio::test]
async fn wrong_method_on_a_registered_api_path_answers_405_not_the_index() {
    let addr = start_server_with_webui().await;
    for (method, path) in [
        (Method::GET, "/auth/login"),
        (Method::POST, "/health"),
        (Method::POST, "/sync/manifest"),
        (Method::GET, "/mcp/tool"),
    ] {
        let resp = request(addr, method.clone(), path).await;
        assert_eq!(resp.status(), 405, "{method} {path}");
        assert!(!is_html(&resp), "{method} {path} answered HTML");
    }
}

#[tokio::test]
async fn registered_api_routes_still_answer_with_the_web_ui_enabled() {
    let addr = start_server_with_webui().await;
    let resp = request(addr, Method::GET, "/health").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn non_read_methods_outside_the_api_answer_405() {
    let addr = start_server_with_webui().await;
    for method in [Method::POST, Method::PUT, Method::DELETE] {
        let resp = request(addr, method.clone(), "/").await;
        assert_eq!(resp.status(), 405, "{method} /");
    }
}

#[tokio::test]
async fn a_path_escaping_the_served_directory_never_reveals_the_file_outside() {
    let addr = start_server_with_webui().await;
    for path in [
        "/..%2fsecret.txt",
        "/%2e%2e%2fsecret.txt",
        "/_app/..%2f..%2fsecret.txt",
        "/%2fetc%2fpasswd",
    ] {
        let resp = request(addr, Method::GET, path).await;
        let body = resp.text().await.unwrap();
        assert!(!body.contains(SECRET_OUTSIDE_THE_WEBUI_DIR), "{path}");
    }
}

#[tokio::test]
async fn without_a_web_ui_directory_only_the_api_answers() {
    let addr = start_server_without_webui().await;
    for path in ["/", "/login", "/_app/immutable/app.js"] {
        let resp = request(addr, Method::GET, path).await;
        assert_eq!(resp.status(), 404, "{path}");
        assert!(!is_html(&resp), "{path}");
    }
    let resp = request(addr, Method::GET, "/health").await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn a_web_ui_directory_without_an_index_serves_the_api_only() {
    let root = tempfile::tempdir().unwrap().keep();
    let webui_dir = root.join("empty-webui");
    std::fs::create_dir_all(&webui_dir).unwrap();
    std::fs::write(webui_dir.join("stray.txt"), "must stay private").unwrap();
    let addr = common::start_server_with_config(
        common::TestServerConfigBuilder::new(root.join("repos"))
            .webui_dir(webui_dir)
            .build(),
    )
    .await;

    for path in ["/", "/login", "/stray.txt"] {
        let resp = request(addr, Method::GET, path).await;
        assert_eq!(resp.status(), 404, "{path}");
    }
    let resp = request(addr, Method::GET, "/health").await;
    assert_eq!(resp.status(), 200);
}
