//! Test-only capture of everything `tracing` would write.
//!
//! The captured text uses the same `tracing_subscriber::fmt` formatter as production (all targets,
//! TRACE and below, span open/close events included), so a field, an error report or a `Debug`
//! rendering that would reach the log file also reaches the capture.
//!
//! One process-wide subscriber is installed on first use and routes each event to the capture
//! registered for the **current thread**. A per-thread `set_default` subscriber is not enough: with
//! tests running in parallel, other threads evaluate (and cache) callsite interest while this thread's
//! subscriber does not exist yet, and dynamic callsites such as sqlx's statement logging then stay
//! disabled for the capturing test. Use the capture from a `#[tokio::test]` (current-thread runtime,
//! the default flavor) so every task the test spawns runs on the capturing thread. Events emitted by
//! other OS threads (for example the `wiremock` server thread, which models the remote receiver) are
//! intentionally not captured.

use std::cell::RefCell;
use std::io::Write;
use std::sync::{Arc, Mutex, Once};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::MakeWriter;

type Buffer = Arc<Mutex<Vec<u8>>>;

thread_local! {
    static CURRENT: RefCell<Option<Buffer>> = const { RefCell::new(None) };
}

struct Router;

impl Write for Router {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        CURRENT.with(|current| {
            if let Some(buffer) = current.borrow().as_ref() {
                buffer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(buf);
            }
        });
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Router {
    type Writer = Router;

    fn make_writer(&'a self) -> Self::Writer {
        Router
    }
}

fn install_global_subscriber() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(Router)
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_target(true)
            .with_span_events(FmtSpan::FULL)
            .finish();
        tracing::subscriber::set_global_default(subscriber).expect(
            "the log capture needs to own the global tracing subscriber of the test process",
        );
    });
}

/// Captures tracing output of the current thread until dropped.
pub struct LogCapture {
    buffer: Buffer,
}

impl LogCapture {
    pub fn start() -> Self {
        install_global_subscriber();
        let buffer = Buffer::default();
        CURRENT.with(|current| *current.borrow_mut() = Some(buffer.clone()));
        Self { buffer }
    }

    /// Everything captured so far.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.buffer.lock().unwrap_or_else(|e| e.into_inner())).to_string()
    }

    /// Self-check: the capture really sees events. A leak test that captured nothing proves nothing.
    pub fn assert_sees(&self, marker: &str) {
        let text = self.text();
        assert!(
            text.contains(marker),
            "the log capture did not observe `{marker}`; captured {} bytes: {}",
            text.len(),
            text.chars().take(600).collect::<String>()
        );
    }

    /// Fails when any of `forbidden` (`(label, value)`) appears in the captured output, compared
    /// as-is and ASCII-case-insensitively. Empty values are rejected to avoid vacuous checks.
    pub fn assert_absent(&self, forbidden: &[(&str, &str)]) {
        let text = self.text();
        let lower = text.to_ascii_lowercase();
        for (label, value) in forbidden {
            assert!(!value.is_empty(), "forbidden value `{label}` is empty");
            assert!(
                !text.contains(value) && !lower.contains(&value.to_ascii_lowercase()),
                "`{label}` leaked into the tracing output:\n{text}"
            );
        }
    }
}

impl Drop for LogCapture {
    fn drop(&mut self) {
        CURRENT.with(|current| *current.borrow_mut() = None);
    }
}
