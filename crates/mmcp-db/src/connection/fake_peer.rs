//! A loopback listener that stands in for a Postgres server which ends every connection.
//!
//! A connect to it ends with an error from the driver, not with a refused port that the pool retries until its timeout.
//! A test that searches the error chain for a secret then searches a chain that exists.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Length of the SSL request that a client sends before it negotiates TLS.
const SSL_REQUEST_LENGTH: usize = 8;

/// What the peer does with a connection before it closes it.
#[derive(Debug, Clone, Copy)]
pub(super) enum Behavior {
    /// Closes at once, so a client that sent an SSL request or a startup message reads the end of the stream.
    Closes,
    /// Reads the SSL request, answers `S`, then closes.
    /// The driver then builds its TLS configuration, which is where it raises the errors about client certificates and keys.
    AnswersSslRequestThenCloses,
}

/// A listener on a free loopback port, served until it drops.
pub(super) struct FakePeer {
    port: u16,
    task: JoinHandle<()>,
}

impl FakePeer {
    /// Binds a free loopback port and serves it on the current runtime.
    pub(super) async fn start(behavior: Behavior) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    if matches!(behavior, Behavior::AnswersSslRequestThenCloses) {
                        let mut request = [0_u8; SSL_REQUEST_LENGTH];
                        if stream.read_exact(&mut request).await.is_ok() {
                            let _ = stream.write_all(b"S").await;
                        }
                    }
                    drop(stream);
                });
            }
        });
        Self { port, task }
    }

    pub(super) fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for FakePeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
