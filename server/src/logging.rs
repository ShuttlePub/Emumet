//! Process logging set-up.
//!
//! stdout is the log of record. A rolling `./logs/debug.log.<date>` file is written ONLY when
//! `EMUMET_FILE_LOG_ENABLED=true` is set (development convenience); by default no file appender
//! and no `./logs` directory exist, so a container can run on a read-only root filesystem.
//!
//! `EMUMET_FILE_LOG_ENABLED` is read from the process environment only, never from `.env`: the
//! logger is initialised before the first `dotenvy` call loads `.env`, and reading it through
//! `dotenvy` would change when `.env` is loaded (and make `RUST_LOG` / `WORKER_ID` in `.env`
//! start to apply). `RUST_LOG` keeps its existing behaviour.

use error_stack::{Report, ResultExt};
use kernel::KernelError;
use std::env::VarError;
use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

pub(crate) const FILE_LOG_ENV: &str = "EMUMET_FILE_LOG_ENABLED";
const LOG_DIRECTORY: &str = "./logs/";
const LOG_FILE_PREFIX: &str = "debug.log";

/// Unset or `false`: off. `true`: on. Anything else (empty, `TRUE`, `1`, non-UTF-8, ...) is a
/// configuration error rather than a silent default.
pub(crate) fn parse_file_log_setting(value: Result<String, VarError>) -> Result<bool, String> {
    match value.as_deref() {
        Err(VarError::NotPresent) | Ok("false") => Ok(false),
        Ok("true") => Ok(true),
        Ok(_) | Err(VarError::NotUnicode(_)) => Err(format!(
            "{FILE_LOG_ENV} must be exactly `true` or `false` when set"
        )),
    }
}

/// Keeps the file writer's background thread alive (and flushes it) until dropped.
pub(crate) struct LogGuard {
    _file: Option<WorkerGuard>,
}

pub(crate) fn init() -> error_stack::Result<LogGuard, KernelError> {
    let file_enabled = parse_file_log_setting(std::env::var(FILE_LOG_ENV))
        .map_err(|message| Report::new(KernelError::Validation).attach_printable(message))?;

    let (file_layer, guard) = if file_enabled {
        let appender = tracing_appender::rolling::RollingFileAppender::builder()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix(LOG_FILE_PREFIX)
            .build(Path::new(LOG_DIRECTORY))
            .change_context_lazy(|| KernelError::Internal)
            .attach_printable_lazy(|| {
                format!("{FILE_LOG_ENV}=true but {LOG_DIRECTORY} is not writable")
            })?;
        let (writer, guard) = tracing_appender::non_blocking(appender);
        let layer = tracing_subscriber::fmt::Layer::default()
            .with_writer(writer)
            .with_ansi(false)
            .with_filter(tracing_subscriber::filter::LevelFilter::DEBUG);
        (Some(layer), Some(guard))
    } else {
        (None, None)
    };

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_filter(tracing_subscriber::EnvFilter::new(
                    std::env::var("RUST_LOG").unwrap_or_else(|_| {
                        "driver=debug,server=debug,tower_http=debug,hyper=debug,sqlx=debug".into()
                    }),
                ))
                .with_filter(tracing_subscriber::filter::LevelFilter::DEBUG),
        )
        .with(file_layer)
        .init();

    Ok(LogGuard { _file: guard })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn setting(value: Option<&str>) -> Result<bool, String> {
        parse_file_log_setting(value.map(str::to_string).ok_or(VarError::NotPresent))
    }

    #[test]
    fn unset_and_false_disable_the_file_log() {
        assert_eq!(setting(None), Ok(false));
        assert_eq!(setting(Some("false")), Ok(false));
    }

    #[test]
    fn only_the_exact_word_true_enables_it() {
        assert_eq!(setting(Some("true")), Ok(true));
    }

    #[test]
    fn every_other_value_is_a_configuration_error() {
        for value in [
            "", " ", "TRUE", "True", "1", "0", "yes", "no", "false ", " true", "off",
        ] {
            let error = setting(Some(value)).expect_err(value);
            assert!(error.contains(FILE_LOG_ENV), "{value}: {error}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let invalid = OsString::from_vec(vec![0xff, 0xfe]);
            assert!(parse_file_log_setting(Err(VarError::NotUnicode(invalid))).is_err());
        }
    }

    /// The error names the variable only, never echoes the offending value.
    #[test]
    fn the_error_does_not_echo_the_value() {
        let error = setting(Some("s3cr3t-looking-value")).unwrap_err();
        assert!(!error.contains("s3cr3t-looking-value"));
    }
}
