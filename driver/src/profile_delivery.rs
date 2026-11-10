//! Profile 配送 (P3o) の外向き HTTP: 署名済み activity の inbox への POST と、remote actor の inbox 解決。
//!
//! どちらも同じ宛先検査 (https のみ・userinfo 不可・内部宛先拒否・DNS 解決結果の再検査・接続固定・
//! redirect 拒否・timeout) を通る。http・loopback を許可する構築は `cfg(test)` / `test-mode`
//! ビルドにだけ存在し、設定や環境変数からは選べない。

mod resolver;
mod sender;
mod target;

#[cfg(any(test, feature = "test-mode"))]
pub use resolver::inject_test_resolved_actor;
pub use resolver::HttpRemoteActorResolver;
pub use sender::HttpProfileDeliverySender;
pub use target::ProfileDeliveryNetworkSettings;

#[cfg(test)]
mod tests;
