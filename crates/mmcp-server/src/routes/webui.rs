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

use crate::routes::defaults::{
    HTML_MEDIA_TYPE_PREFIX, WEBUI_FRAME_ANCESTORS_NONE, WEBUI_IMMUTABLE_ASSET_PATH_PREFIX,
    WEBUI_IMMUTABLE_CACHE_CONTROL, WEBUI_INDEX_FILE_NAME, WEBUI_NO_REFERRER, WEBUI_NO_SNIFF,
    WEBUI_REVALIDATE_CACHE_CONTROL,
};

/// Service answering every request the API does not own with the web UI in `webui_dir`.
///
/// `GET` and `HEAD` serve the file at the request path, or the index document when none exists.
/// Client-side routes therefore survive a reload.
/// Other methods answer 405.
/// Every file under `webui_dir` is public.
/// A path that would leave the directory is never resolved.
///
/// `None` when `webui_dir` holds no index document, leaving the API alone.
pub fn service(webui_dir: &Path) -> Option<Router> {
    let index_file = webui_dir.join(WEBUI_INDEX_FILE_NAME);
    if !index_file.is_file() {
        tracing::warn!(webui_dir = %webui_dir.display(), missing_file = WEBUI_INDEX_FILE_NAME);
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
        .starts_with(WEBUI_IMMUTABLE_ASSET_PATH_PREFIX);
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
        HeaderValue::from_static(WEBUI_NO_SNIFF),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static(WEBUI_NO_REFERRER),
    );
    if is_html {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(WEBUI_REVALIDATE_CACHE_CONTROL),
        );
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(WEBUI_FRAME_ANCESTORS_NONE),
        );
    } else if is_immutable_asset_path && is_served {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(WEBUI_IMMUTABLE_CACHE_CONTROL),
        );
    }
    response
}
