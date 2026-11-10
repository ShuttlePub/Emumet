//! 登録保護 (D15 / E15c) の provider adapter: Turnstile Siteverify と SMTP。
//!
//! 設定は環境変数 (または secret ファイル) からだけ読み、secret は DB・ログ・エラー・API 応答に出さない。
//! 設定の不備は起動失敗にならず、スナップショットの `issue` (= `configuration_invalid`) になる。

mod config;
pub mod smtp;
pub mod turnstile;

pub use self::config::{derive_generation, secret_fingerprint, EnvSource, ProcessEnv, ValueSource};
pub use self::smtp::{SmtpEmailTransport, SmtpSettings, SmtpTlsMode};
pub use self::turnstile::{
    classify_siteverify, HttpTurnstileVerifier, SiteverifyExpectation, TurnstileNetworkSettings,
    TurnstileSettings,
};

#[cfg(test)]
mod tests;
