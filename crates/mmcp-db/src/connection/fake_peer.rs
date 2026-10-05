//! A loopback listener that stands in for a Postgres server which ends every connection.
//!
//! A connect to it ends with an error from the driver, not with a refused port that the pool retries until its timeout.
//! A test that searches the error chain for a secret then searches a chain that exists.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// Length of the SSL request that a client sends before it negotiates TLS.
const SSL_REQUEST_LENGTH: usize = 8;

/// The protocol code that the SSL request carries where a startup message carries its version.
const SSL_REQUEST_CODE: u32 = 80_877_103;

/// The size of the length field of a message, which counts itself.
const LENGTH_FIELD_SIZE: usize = size_of::<u32>();

/// The size of the code field of an authentication message.
const CODE_FIELD_SIZE: usize = size_of::<u32>();

/// What an authentication message counts besides its payload: the length field and the code field.
const AUTHENTICATION_HEADER_LENGTH: usize = LENGTH_FIELD_SIZE + CODE_FIELD_SIZE;

/// The tag of an authentication message of the server.
const AUTHENTICATION_TAG: u8 = b'R';

/// The code of the message that ends the authentication successfully.
const AUTHENTICATION_OK: u32 = 0;

/// The tag, the length (the length field and one status byte) and the idle status of `ReadyForQuery`.
const READY_FOR_QUERY_TAG: u8 = b'Z';
const READY_FOR_QUERY_LENGTH: u32 = 5;
const TRANSACTION_STATUS_IDLE: u8 = b'I';

/// The code of the message that offers SASL mechanisms.
const AUTHENTICATION_SASL: u32 = 10;

/// The code of the message that carries the server-first-message of a SASL exchange.
const AUTHENTICATION_SASL_CONTINUE: u32 = 11;

/// The one mechanism the peer offers, with the terminator of the list.
const SCRAM_MECHANISM_LIST: &[u8] = b"SCRAM-SHA-256\0\0";

/// The salt and the iteration count of the server-first-message.
const SCRAM_SALT_AND_ITERATIONS: &str = "s=c2FsdA==,i=4096";

/// The attribute that carries the client nonce in the client-first-message.
const NONCE_ATTRIBUTE: &str = "r=";

/// What the SASLprep failure of the driver holds when it fails on a credential of the SCRAM peer: its text, the error name, the character U+1F600 and its escaped Debug forms.
/// A log that holds none of them holds none of the failure.
pub const SASLPREP_FAILURE_MARKERS: [&str; 5] = [
    "Failed to saslprep",
    "ProhibitedCharacter",
    "\u{1F600}",
    "\\u{1f600}",
    "\\u{7}",
];

/// What the peer does with a connection before it closes it.
#[derive(Debug, Clone, Copy)]
pub enum Behavior {
    /// Closes at once, so a client that sent an SSL request or a startup message reads the end of the stream.
    Closes,
    /// Reads the SSL request, answers `S`, then closes.
    /// The driver then builds its TLS configuration, which is where it raises the errors about client certificates and keys.
    AnswersSslRequestThenCloses,
    /// Declines TLS, asks for SCRAM authentication, answers the client-first-message, then closes.
    /// The driver applies SASLprep to the user name before it sends the first message and to the password after it reads the answer.
    RequestsScram,
    /// Accepts the first connection with trust authentication and closes it after the start-up, then asks for SCRAM on every later one.
    /// A pool that finds its idle connection closed opens a new one, which meets SCRAM, as it does when the server changes its method between two connections.
    TrustsFirstThenRequestsScram,
}

/// A listener on a free loopback port, served until it drops.
pub struct FakePeer {
    port: u16,
    task: JoinHandle<()>,
}

impl FakePeer {
    /// Binds a free loopback port and serves it on the current runtime.
    pub async fn start(behavior: Behavior) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let mut accepted = 0_usize;
            while let Ok((stream, _)) = listener.accept().await {
                let first = accepted == 0;
                accepted += 1;
                tokio::spawn(async move {
                    // The client ends the exchange when it fails, so a read or a write error here is expected.
                    let _ = serve(stream, behavior, first).await;
                });
            }
        });
        Self { port, task }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for FakePeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Serves one connection, which is the first one of the listener when `first` is set.
async fn serve(mut stream: TcpStream, behavior: Behavior, first: bool) -> io::Result<()> {
    match behavior {
        Behavior::Closes => {}
        Behavior::AnswersSslRequestThenCloses => {
            let mut request = [0_u8; SSL_REQUEST_LENGTH];
            stream.read_exact(&mut request).await?;
            stream.write_all(b"S").await?;
        }
        Behavior::RequestsScram => serve_scram(&mut stream).await?,
        Behavior::TrustsFirstThenRequestsScram if first => serve_trust(&mut stream).await?,
        Behavior::TrustsFirstThenRequestsScram => serve_scram(&mut stream).await?,
    }
    Ok(())
}

/// Declines every SSL request, then returns once the start-up message is read.
async fn read_startup(stream: &mut TcpStream) -> io::Result<()> {
    while read_untagged_message(stream).await? == SSL_REQUEST_CODE {
        stream.write_all(b"N").await?;
    }
    Ok(())
}

/// Answers the start-up with `AuthenticationOk` and `ReadyForQuery`.
async fn serve_trust(stream: &mut TcpStream) -> io::Result<()> {
    read_startup(stream).await?;
    write_authentication(stream, AUTHENTICATION_OK, &[]).await?;
    stream.write_u8(READY_FOR_QUERY_TAG).await?;
    stream.write_u32(READY_FOR_QUERY_LENGTH).await?;
    stream.write_u8(TRANSACTION_STATUS_IDLE).await
}

/// Reads a message that has no tag, and returns its code.
async fn read_untagged_message(stream: &mut TcpStream) -> io::Result<u32> {
    let length = stream.read_u32().await?;
    let mut body = vec![0_u8; length as usize - LENGTH_FIELD_SIZE];
    stream.read_exact(&mut body).await?;
    Ok(u32::from_be_bytes(
        body[..CODE_FIELD_SIZE].try_into().unwrap(),
    ))
}

/// Writes a tagged message made of `code` and `payload`.
async fn write_authentication(stream: &mut TcpStream, code: u32, payload: &[u8]) -> io::Result<()> {
    let length = u32::try_from(AUTHENTICATION_HEADER_LENGTH + payload.len()).unwrap();
    stream.write_u8(AUTHENTICATION_TAG).await?;
    stream.write_u32(length).await?;
    stream.write_u32(code).await?;
    stream.write_all(payload).await
}

async fn serve_scram(stream: &mut TcpStream) -> io::Result<()> {
    read_startup(stream).await?;
    write_authentication(stream, AUTHENTICATION_SASL, SCRAM_MECHANISM_LIST).await?;

    // The SASLInitialResponse: a tag, a length, the mechanism, then the length and the text of the client-first-message.
    let _tag = stream.read_u8().await?;
    let length = stream.read_u32().await?;
    let mut body = vec![0_u8; length as usize - LENGTH_FIELD_SIZE];
    stream.read_exact(&mut body).await?;
    let text = String::from_utf8_lossy(&body);
    let nonce = text
        .split(',')
        .find_map(|attribute| attribute.strip_prefix(NONCE_ATTRIBUTE))
        .unwrap_or_default();
    let server_first = format!("{NONCE_ATTRIBUTE}{nonce}server,{SCRAM_SALT_AND_ITERATIONS}");
    write_authentication(
        stream,
        AUTHENTICATION_SASL_CONTINUE,
        server_first.as_bytes(),
    )
    .await
}
