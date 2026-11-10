//! 登録ゲート (D15 registration-readiness-gate) の control plane ドメイン。
//!
//! - `state`: 永続化される状態と観測入力の型
//! - `machine`: 状態表を実装した **純粋** 状態機械 (I/O なし。判定・遷移・通知イベント生成)
//! - `record`: 監査記録・通知イベントなど永続化レコードの型
//!
//! 実 Kratos の遮断・実 provider の検証はここには含まない (E15b〜E15d)。

mod machine;
mod record;
mod state;

pub use self::machine::*;
pub use self::record::*;
pub use self::state::*;
