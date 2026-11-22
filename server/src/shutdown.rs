//! SIGTERM / SIGINT handling for the server process.
//!
//! `docker stop`, systemd and Kubernetes send SIGTERM; a terminal sends SIGINT. Both take the same
//! graceful path: stop new relay claims and serving together, then drain concurrently within
//! the configured serving-phase deadline.
//!
//! The signal streams are created at the very top of `main`, before logging, the JWKS fetch, the
//! database connection and the migrations. From then on a signal is recorded by tokio (it replaces
//! the default "terminate immediately" action) and is observed whenever the streams are polled
//! next, so a stop request that arrives during a long start-up step is not lost. Nothing here
//! starts a task: `main` polls the streams itself at the points it chooses.

use std::future::poll_fn;
use std::task::{Context, Poll, Waker};

pub(crate) struct ShutdownSignals {
    #[cfg(unix)]
    sigterm: tokio::signal::unix::Signal,
    #[cfg(unix)]
    sigint: tokio::signal::unix::Signal,
    #[cfg(not(unix))]
    ctrl_c: std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>>,
    requested: bool,
}

impl ShutdownSignals {
    /// Failing to install a handler is a start-up error: the process would otherwise be killed
    /// abruptly by the first SIGTERM.
    #[cfg(unix)]
    pub(crate) fn install() -> std::io::Result<Self> {
        use tokio::signal::unix::{signal, SignalKind};
        Ok(Self {
            sigterm: signal(SignalKind::terminate())?,
            sigint: signal(SignalKind::interrupt())?,
            requested: false,
        })
    }

    #[cfg(not(unix))]
    pub(crate) fn install() -> std::io::Result<Self> {
        Ok(Self {
            ctrl_c: Box::pin(tokio::signal::ctrl_c()),
            requested: false,
        })
    }

    fn poll_requested(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.requested {
            return Poll::Ready(());
        }
        #[cfg(unix)]
        let seen = self.sigterm.poll_recv(cx).is_ready() || self.sigint.poll_recv(cx).is_ready();
        #[cfg(not(unix))]
        let seen = self.ctrl_c.as_mut().poll(cx).is_ready();
        if seen {
            self.requested = true;
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    /// Resolves once SIGTERM or SIGINT has been received (also if it was received earlier).
    pub(crate) async fn requested(&mut self) {
        poll_fn(|cx| self.poll_requested(cx)).await
    }

    /// Non-blocking check, used after the uninterruptible initialisation steps.
    pub(crate) fn is_requested(&mut self) -> bool {
        self.poll_requested(&mut Context::from_waker(Waker::noop()))
            .is_ready()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::signal::unix::{signal, SignalKind};

    /// Signals are process-wide: the handlers installed by `ShutdownSignals` (and the extra
    /// `Signal`s the test keeps alive) stop the default "terminate" action.
    fn raise(name: &str) {
        let status = std::process::Command::new("kill")
            .arg(format!("-{name}"))
            .arg(std::process::id().to_string())
            .status()
            .expect("kill is available");
        assert!(status.success());
    }

    /// One test for both signals so that the process-global scenarios cannot interleave.
    #[tokio::test]
    async fn sigterm_and_sigint_are_latched_and_observed_later() {
        let _keep_term = signal(SignalKind::terminate()).unwrap();
        let _keep_int = signal(SignalKind::interrupt()).unwrap();

        let mut signals = ShutdownSignals::install().unwrap();
        assert!(!signals.is_requested(), "nothing was sent yet");
        assert!(
            tokio::time::timeout(Duration::from_millis(50), signals.requested())
                .await
                .is_err(),
            "requested() must stay pending without a signal"
        );

        // SIGTERM arrives while nobody is polling: it is observed afterwards and stays observed.
        raise("TERM");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(signals.is_requested());
        assert!(signals.is_requested(), "the request is latched");
        tokio::time::timeout(Duration::from_secs(1), signals.requested())
            .await
            .expect("requested() resolves immediately once latched");

        // SIGINT takes the same path.
        let mut signals = ShutdownSignals::install().unwrap();
        raise("INT");
        tokio::time::timeout(Duration::from_secs(5), signals.requested())
            .await
            .expect("SIGINT resolves requested()");
        assert!(signals.is_requested());
    }
}
