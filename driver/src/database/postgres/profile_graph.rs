//! Profile social graph の各 repository が共有する列の変換と条件組み立て。

use error_stack::Report;
use kernel::prelude::entity::{GraphActorUrl, ProfileGraphActor, ProfileId};
use kernel::KernelError;
use sqlx::{Postgres, QueryBuilder};

/// 関係の当事者を `(profile_id, actor_url)` の列値に分解する。どちらか一方だけが `Some`。
pub(super) fn split_actor(actor: &ProfileGraphActor) -> (Option<i64>, Option<&str>) {
    match actor {
        ProfileGraphActor::Local(profile_id) => (Some(*profile_id.as_ref()), None),
        ProfileGraphActor::Remote(url) => (None, Some(url.as_str())),
    }
}

pub(super) fn actor_from_columns(
    profile_id: Option<i64>,
    actor_url: Option<String>,
) -> error_stack::Result<ProfileGraphActor, KernelError> {
    match (profile_id, actor_url) {
        (Some(profile_id), None) => Ok(ProfileGraphActor::Local(ProfileId::new(profile_id))),
        (None, Some(url)) => Ok(ProfileGraphActor::Remote(GraphActorUrl::new(url)?)),
        (profile_id, actor_url) => Err(Report::new(KernelError::Internal).attach_printable(
            format!("invalid actor columns. profile_id: {profile_id:?}, actor_url: {actor_url:?}"),
        )),
    }
}

/// `<prefix>_profile_id = $n` または `<prefix>_actor_url = $n` を追加する。
/// `prefix` は呼び出し側のリテラルで、利用者の入力は bind される。
pub(super) fn push_actor_eq(
    builder: &mut QueryBuilder<'_, Postgres>,
    prefix: &str,
    actor: &ProfileGraphActor,
) {
    match split_actor(actor) {
        (Some(profile_id), None) => {
            builder
                .push(format!("{prefix}_profile_id = "))
                .push_bind(profile_id);
        }
        (None, Some(url)) => {
            builder
                .push(format!("{prefix}_actor_url = "))
                .push_bind(url.to_owned());
        }
        _ => unreachable!("split_actor returns exactly one column"),
    }
}
