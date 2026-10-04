//! Static serving of the built web UI, a single-page application.

use std::path::Path;

use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::Response,
};
use tower_http::services::{ServeDir, ServeFile};

/// Entry document of the single-page application, also the answer to every path that names no file.
const INDEX_FILE_NAME: &str = "index.html";

/// Path prefix of the content-hashed assets the web UI build emits.
const IMMUTABLE_ASSET_PATH_PREFIX: &str = "/_app/immutable/";

/// One year, the longest lifetime caches honour; the content hash in the file name makes it safe.
const IMMUTABLE_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// The entry document names the current hashed assets, so a cache must revalidate it.
const REVALIDATE_CACHE_CONTROL: &str = "no-cache";

const NO_SNIFF: &str = "nosniff";

const NO_REFERRER: &str = "no-referrer";

/// The login page lives on the API origin, so no site may frame it.
const FRAME_ANCESTORS_NONE: &str = "frame-ancestors 'none'";

const HTML_MEDIA_TYPE_PREFIX: &str = "text/html";

/// Service answering every request the API does not own with the web UI in `webui_dir`.
///
/// `GET` and `HEAD` serve the file at the request path, or [`INDEX_FILE_NAME`]
/// when none exists, so client-side routes survive a reload; other methods answer 405.
/// Every file under `webui_dir` is public, and a path that would leave the directory
/// is never resolved.
///
/// `None` when `webui_dir` holds no [`INDEX_FILE_NAME`], leaving the API alone.
pub fn service(webui_dir: &Path) -> Option<Router> {
    let index_file = webui_dir.join(INDEX_FILE_NAME);
    if !index_file.is_file() {
        tracing::warn!(webui_dir = %webui_dir.display(), missing_file = INDEX_FILE_NAME);
        return None;
    }
    tracing::info!(webui_dir = %webui_dir.display());
    let files = ServeDir::new(webui_dir).fallback(ServeFile::new(index_file));
    Some(
        Router::new()
            .fallback_service(files)
            .layer(middleware::from_fn(add_static_headers)),
    )
}

/// Sets the cache and hardening headers of a static response.
async fn add_static_headers(request: Request, next: Next) -> Response {
    let is_immutable_asset_path = request
        .uri()
        .path()
        .starts_with(IMMUTABLE_ASSET_PATH_PREFIX);
    let mut response = next.run(request).await;
    let is_html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|content_type| content_type.to_str().ok())
        .is_some_and(|content_type| content_type.starts_with(HTML_MEDIA_TYPE_PREFIX));
    let is_served = response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static(NO_SNIFF),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static(NO_REFERRER),
    );
    if is_html {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(REVALIDATE_CACHE_CONTROL),
        );
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(FRAME_ANCESTORS_NONE),
        );
    } else if is_immutable_asset_path && is_served {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(IMMUTABLE_CACHE_CONTROL),
        );
    }
    response
}
