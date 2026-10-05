//! Capture of tracing output at trace level, spans included, for the tests that look for a secret in it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::format::FmtSpan;

/// A buffer that a tracing subscriber writes every event and span event into.
#[derive(Clone, Default)]
pub struct LogCapture {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl LogCapture {
    /// Installs a subscriber for the current thread, until the guard drops.
    ///
    /// The subscriber enables every level, and prints the fields of a span with its enter, exit and close events.
    /// A test that awaits runs on a current-thread runtime, so every future it polls reports to this subscriber.
    pub fn install() -> (Self, tracing::subscriber::DefaultGuard) {
        let capture = Self::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_span_events(FmtSpan::FULL)
            .with_ansi(false)
            .with_writer(capture.clone())
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        (capture, guard)
    }

    /// Everything written so far.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.buffer.lock().unwrap()).into_owned()
    }

    /// Panics when the capture holds any of `forbidden`, showing the first line that holds it.
    pub fn assert_holds_none_of(&self, forbidden: &[&str]) {
        let logged = self.text();
        for text in forbidden {
            if let Some(line) = logged.lines().find(|line| line.contains(text)) {
                panic!("the log holds `{text}`: {line}");
            }
        }
    }
}

impl io::Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buffer.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogCapture {
    type Writer = LogCapture;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
