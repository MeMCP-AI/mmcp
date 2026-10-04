//! Serving a router with the peer address of every connection.

use std::net::SocketAddr;

use axum::Router;
use tokio::net::TcpListener;

/// Serve `router` on `listener` until the listener fails.
///
/// Every request carries the peer address of its connection as `ConnectInfo<SocketAddr>`.
/// The client address of a rate limited route is resolved from it.
/// The binary and every test server serve through this one function, so none of them can serve without client addresses.
///
/// # Errors
/// The I/O error of the listener, as [`axum::serve`] reports it.
pub async fn serve_router(listener: TcpListener, router: Router) -> std::io::Result<()> {
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use axum::extract::ConnectInfo;
    use axum::routing::get;

    use super::*;

    /// A request served through [`serve_router`] exposes the peer address the client connected from.
    #[tokio::test]
    async fn serve_router_provides_client_addresses() {
        let router = Router::new().route(
            "/peer",
            get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move { peer.ip().to_string() }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(serve_router(listener, router));

        let body = reqwest::get(format!("http://{address}/peer"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        server.abort();

        assert_eq!(body, "127.0.0.1");
    }
}
