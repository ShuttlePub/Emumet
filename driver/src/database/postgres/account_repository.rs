use super::account_event_store::PostgresAccountEventStore;
use crate::database::{PostgresConnection, PostgresDatabase};
use error_stack::Report;
use kernel::interfaces::event_store::AccountEventStore;
use kernel::interfaces::repository::{AggregateRepository, DependOnAccountRepository, Rehydrated};
use kernel::prelude::entity::{
    Account, AccountEvent, AccountId, CommandEnvelope, EventEnvelope, EventId,
};
use kernel::KernelError;

pub struct PostgresAccountRepository;

impl AggregateRepository<Account> for PostgresAccountRepository {
    type Connection = PostgresConnection;
    type Id = AccountId;

    async fn load(
        &self,
        executor: &mut Self::Connection,
        id: &Self::Id,
    ) -> error_stack::Result<Rehydrated<Account>, KernelError> {
        let events = PostgresAccountEventStore
            .find_by_id(executor, &EventId::from(id.clone()), None)
            .await?;
        Rehydrated::from_events(events)?.ok_or_else(|| {
            Report::new(KernelError::NotFound)
                .attach_printable(format!("No events found for account: {}", id.as_ref()))
        })
    }

    async fn save(
        &self,
        executor: &mut Self::Connection,
        command: CommandEnvelope<AccountEvent, Account>,
    ) -> error_stack::Result<EventEnvelope<AccountEvent, Account>, KernelError> {
        PostgresAccountEventStore
            .persist_and_transform(executor, command)
            .await
    }
}

impl DependOnAccountRepository for PostgresDatabase {
    type AccountRepository = PostgresAccountRepository;

    fn account_repository(&self) -> &Self::AccountRepository {
        &PostgresAccountRepository
    }
}

#[cfg(test)]
mod test {
    use crate::database::PostgresDatabase;
    use kernel::interfaces::database::DatabaseConnection;
    use kernel::interfaces::event::EventApplier;
    use kernel::interfaces::event_store::{AccountEventStore, DependOnAccountEventStore};
    use kernel::interfaces::repository::{AggregateRepository, DependOnAccountRepository};
    use kernel::prelude::entity::{
        Account, AccountEvent, AccountId, AccountIsBot, AccountName, AuthAccountId,
        CommandEnvelope, EventEnvelope, EventId, EventVersion, ExpectedVersion, Nanoid,
    };
    use kernel::KernelError;
    use time::OffsetDateTime;

    fn script_events(nanoid: &Nanoid<Account>) -> Vec<AccountEvent> {
        let at = OffsetDateTime::now_utc();
        vec![
            AccountEvent::Created {
                name: AccountName::new("equivalence"),
                is_bot: AccountIsBot::new(false),
                nanoid: nanoid.clone(),
                auth_account_id: AuthAccountId::default(),
            },
            AccountEvent::Updated {
                is_bot: AccountIsBot::new(true),
            },
            AccountEvent::Suspended {
                reason: "spam".to_string(),
                suspended_at: at,
                expires_at: None,
            },
            AccountEvent::Unsuspended,
            AccountEvent::Banned {
                reason: "violation".to_string(),
                banned_at: at,
            },
        ]
    }

    async fn run_old_path(
        db: &PostgresDatabase,
        executor: &mut <PostgresDatabase as DatabaseConnection>::Connection,
        account_id: &AccountId,
        events: &[AccountEvent],
    ) -> (Vec<EventEnvelope<AccountEvent, Account>>, Account) {
        let mut expected = ExpectedVersion::Nothing;
        let mut envelopes = Vec::new();
        for event in events {
            let command = CommandEnvelope::new(
                EventId::from(account_id.clone()),
                event.name(),
                event.clone(),
                Some(expected),
            );
            let envelope = db
                .account_event_store()
                .persist_and_transform(executor, command)
                .await
                .unwrap();
            expected = ExpectedVersion::At(EventVersion::new(*envelope.version.as_ref()));
            envelopes.push(envelope);
        }
        let mut account: Option<Account> = None;
        for envelope in envelopes.clone() {
            Account::apply(&mut account, envelope).unwrap();
        }
        (envelopes, account.unwrap())
    }

    async fn run_new_path(
        db: &PostgresDatabase,
        executor: &mut <PostgresDatabase as DatabaseConnection>::Connection,
        account_id: &AccountId,
        events: &[AccountEvent],
    ) -> Vec<EventEnvelope<AccountEvent, Account>> {
        let mut expected = ExpectedVersion::Nothing;
        let mut envelopes = Vec::new();
        for event in events {
            let command = CommandEnvelope::new(
                EventId::from(account_id.clone()),
                event.name(),
                event.clone(),
                Some(expected),
            );
            let envelope = db
                .account_repository()
                .save(executor, command)
                .await
                .unwrap();
            expected = ExpectedVersion::At(EventVersion::new(*envelope.version.as_ref()));
            envelopes.push(envelope);
        }
        envelopes
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn save_and_load_are_equivalent_to_event_store_path() {
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.unwrap();
        let mut conn = db.connection().await.unwrap();
        let nanoid = Nanoid::default();
        let events = script_events(&nanoid);

        let old_id = AccountId::default();
        let old_envelopes = run_old_path(&db, &mut conn, &old_id, &events).await;
        let new_id = AccountId::default();
        let new_envelopes = run_new_path(&db, &mut conn, &new_id, &events).await;

        assert_eq!(old_envelopes.0.len(), new_envelopes.len());
        for (old, new) in old_envelopes.0.iter().zip(new_envelopes.iter()) {
            assert_eq!(&old.event, &new.event);
        }
        for envelopes in [&old_envelopes.0, &new_envelopes] {
            assert!(envelopes
                .windows(2)
                .all(|pair| pair[0].version < pair[1].version));
        }

        let stored_old = db
            .account_event_store()
            .find_by_id(&mut conn, &EventId::from(old_id), None)
            .await
            .unwrap();
        let stored_new = db
            .account_event_store()
            .find_by_id(&mut conn, &EventId::from(new_id.clone()), None)
            .await
            .unwrap();
        assert_eq!(stored_old.len(), events.len());
        assert_eq!(stored_new.len(), events.len());
        for (stored, envelope) in stored_old.iter().zip(old_envelopes.0.iter()) {
            assert_eq!(&stored.event, &envelope.event);
            assert_eq!(stored.version, envelope.version);
        }
        for (old, new) in stored_old.iter().zip(stored_new.iter()) {
            assert_eq!(&old.event, &new.event);
        }
        for (stored, envelope) in stored_new.iter().zip(new_envelopes.iter()) {
            assert_eq!(&stored.event, &envelope.event);
            assert_eq!(stored.version, envelope.version);
        }

        let rehydrated = db
            .account_repository()
            .load(&mut conn, &new_id)
            .await
            .unwrap();
        let old_account = old_envelopes.1;
        let new_account = rehydrated.aggregate();
        assert_eq!(new_account.name(), old_account.name());
        assert_eq!(new_account.is_bot(), old_account.is_bot());
        assert_eq!(new_account.status(), old_account.status());
        assert_eq!(new_account.deleted_at(), old_account.deleted_at());
        assert_eq!(new_account.nanoid(), old_account.nanoid());
        assert_eq!(rehydrated.version(), new_account.version());
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn load_unknown_account_returns_not_found() {
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.unwrap();
        let mut conn = db.connection().await.unwrap();
        let result = db
            .account_repository()
            .load(&mut conn, &AccountId::default())
            .await;
        assert!(result.is_err_and(|e| e.current_context() == &KernelError::NotFound));
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn double_create_conflicts_on_both_paths() {
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.unwrap();
        let mut conn = db.connection().await.unwrap();
        let nanoid = Nanoid::default();
        let events = script_events(&nanoid);

        let old_id = AccountId::default();
        let first = run_old_path(&db, &mut conn, &old_id, &events[..1]).await;
        assert_eq!(first.0.len(), 1);
        let old_again = db
            .account_event_store()
            .persist_and_transform(
                &mut conn,
                CommandEnvelope::new(
                    EventId::from(old_id),
                    events[0].name(),
                    events[0].clone(),
                    Some(ExpectedVersion::Nothing),
                ),
            )
            .await;
        assert!(
            old_again.is_err_and(|e| e.current_context() == &KernelError::Concurrency),
            "old path must reject a duplicate create"
        );

        let new_id = AccountId::default();
        run_new_path(&db, &mut conn, &new_id, &events[..1]).await;
        let new_again = db
            .account_repository()
            .save(
                &mut conn,
                CommandEnvelope::new(
                    EventId::from(new_id),
                    events[0].name(),
                    events[0].clone(),
                    Some(ExpectedVersion::Nothing),
                ),
            )
            .await;
        assert!(
            new_again.is_err_and(|e| e.current_context() == &KernelError::Concurrency),
            "new path must reject a duplicate create"
        );
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn stale_expected_version_conflicts_on_both_paths() {
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.unwrap();
        let mut conn = db.connection().await.unwrap();
        let nanoid = Nanoid::default();
        let events = script_events(&nanoid);

        for use_new_path in [false, true] {
            let account_id = AccountId::default();
            let first_version = if use_new_path {
                run_new_path(&db, &mut conn, &account_id, &events[..2]).await
            } else {
                run_old_path(&db, &mut conn, &account_id, &events[..2])
                    .await
                    .0
            }
            .first()
            .unwrap()
            .version
            .clone();
            let stale = CommandEnvelope::new(
                EventId::from(account_id),
                events[1].name(),
                events[1].clone(),
                Some(ExpectedVersion::At(first_version)),
            );
            let result = if use_new_path {
                db.account_repository().save(&mut conn, stale).await
            } else {
                db.account_event_store()
                    .persist_and_transform(&mut conn, stale)
                    .await
            };
            assert!(
                result.is_err_and(|e| e.current_context() == &KernelError::Concurrency),
                "stale expected version must conflict (new_path={use_new_path})"
            );
        }
    }
}
