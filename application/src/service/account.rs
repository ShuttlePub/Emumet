use crate::service::auth_account::get_auth_account;
use crate::transfer::account::AccountDto;
use crate::transfer::auth_account::AuthAccountInfo;
use crate::transfer::pagination::{apply_pagination, Pagination};
use adapter::crypto::{DependOnSigningKeyGenerator, SigningKeyGenerator};
use error_stack::Report;
use kernel::interfaces::crypto::{DependOnPasswordProvider, PasswordProvider};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::modify::{
    AccountModifier, DependOnAccountModifier, DependOnAuthAccountModifier,
    DependOnAuthHostModifier, DependOnEventModifier, EventModifier,
};
use kernel::interfaces::query::{
    AccountQuery, DependOnAccountQuery, DependOnAuthAccountQuery, DependOnAuthHostQuery,
    DependOnEventQuery, EventQuery,
};
use kernel::interfaces::signal::Signal;
use kernel::prelude::entity::{
    Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
    AuthAccountId, CreatedAt, EventId, EventVersion, Nanoid,
};
use kernel::KernelError;
use serde_json;
use std::future::Future;

pub trait GetAccountService:
    'static
    + Sync
    + Send
    + DependOnAccountQuery
    + DependOnAuthAccountQuery
    + DependOnAuthAccountModifier
    + DependOnAuthHostQuery
    + DependOnAuthHostModifier
    + DependOnEventModifier
    + DependOnEventQuery
{
    fn get_all_accounts(
        &self,
        signal: &impl Signal<AuthAccountId>,
        account: AuthAccountInfo,
        Pagination {
            direction,
            cursor,
            limit,
        }: Pagination<String>,
    ) -> impl Future<Output = error_stack::Result<Option<Vec<AccountDto>>, KernelError>> {
        async move {
            let auth_account = get_auth_account(self, signal, account).await?;
            let mut transaction = self.database_connection().begin_transaction().await?;
            let accounts = self
                .account_query()
                .find_by_auth_id(&mut transaction, auth_account.id())
                .await?;
            let cursor = if let Some(cursor) = cursor {
                let id: Nanoid<Account> = Nanoid::new(cursor);
                self.account_query()
                    .find_by_nanoid(&mut transaction, &id)
                    .await?
            } else {
                None
            };
            let accounts = apply_pagination(accounts, limit, cursor, direction);
            Ok(Some(accounts.into_iter().map(AccountDto::from).collect()))
        }
    }

    fn get_account_by_id(
        &self,
        signal: &impl Signal<AuthAccountId>,
        auth_info: AuthAccountInfo,
        account_id: String,
    ) -> impl Future<Output = error_stack::Result<AccountDto, KernelError>> {
        async move {
            let auth_account = get_auth_account(self, signal, auth_info).await?;
            let mut transaction = self.database_connection().begin_transaction().await?;

            // Nanoidからアカウントを検索
            let nanoid = Nanoid::<Account>::new(account_id);
            let account = self
                .account_query()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            // 権限チェック (認証アカウントに紐づくアカウントのみアクセス可能)
            let auth_account_id = auth_account.id();
            let accounts = self
                .account_query()
                .find_by_auth_id(&mut transaction, auth_account_id)
                .await?;

            let found = accounts.iter().any(|a| a.id() == account.id());
            if !found {
                return Err(Report::new(KernelError::PermissionDenied)
                    .attach_printable("This account does not belong to the authenticated user"));
            }

            Ok(AccountDto::from(account))
        }
    }
}

impl<T> GetAccountService for T where
    T: 'static
        + DependOnAccountQuery
        + DependOnAuthAccountQuery
        + DependOnAuthAccountModifier
        + DependOnAuthHostQuery
        + DependOnAuthHostModifier
        + DependOnEventModifier
        + DependOnEventQuery
{
}

pub trait CreateAccountService:
    'static
    + Sync
    + Send
    + DependOnAccountQuery
    + DependOnAccountModifier
    + DependOnAuthAccountQuery
    + DependOnAuthAccountModifier
    + DependOnAuthHostQuery
    + DependOnAuthHostModifier
    + DependOnEventModifier
    + DependOnEventQuery
    + DependOnPasswordProvider
    + DependOnSigningKeyGenerator
{
    fn create_account(
        &self,
        signal: &impl Signal<AuthAccountId>,
        auth_info: AuthAccountInfo,
        name: String,
        is_bot: bool,
    ) -> impl Future<Output = error_stack::Result<AccountDto, KernelError>> {
        async move {
            // 認証アカウントを確認
            let auth_account = get_auth_account(self, signal, auth_info).await?;
            let mut transaction = self.database_connection().begin_transaction().await?;

            // アカウントID生成
            let account_id = AccountId::default();

            // 鍵ペア生成 (DI経由で取得したPasswordProviderとSigningKeyGeneratorを使用)
            let master_password = self.password_provider().get_password()?;
            let key_pair = self.signing_key_generator().generate(&master_password)?;

            // 暗号化された秘密鍵をJSON文字列として保存
            let encrypted_private_key_json = serde_json::to_string(&key_pair.encrypted_private_key)
                .map_err(|e| {
                    Report::new(KernelError::Internal)
                        .attach_printable(format!("Failed to serialize encrypted private key: {e}"))
                })?;

            let private_key = AccountPrivateKey::new(encrypted_private_key_json);
            let public_key = AccountPublicKey::new(key_pair.public_key_pem);

            // アカウント名とbot状態設定
            let account_name = AccountName::new(name);
            let account_is_bot = AccountIsBot::new(is_bot);

            // NanoIDの生成
            let nanoid = Nanoid::<Account>::default();

            // 直接エンティティ構築
            let created_at = CreatedAt::now();
            let version = EventVersion::default();
            let account = Account::new(
                account_id.clone(),
                account_name,
                private_key,
                public_key,
                account_is_bot,
                None,
                version,
                nanoid,
                created_at,
            );

            // accountsテーブルにINSERT
            self.account_modifier()
                .create(&mut transaction, &account)
                .await?;

            // auth_emumet_accountsにINSERT
            self.account_modifier()
                .link_auth_account(&mut transaction, &account_id, auth_account.id())
                .await?;

            Ok(AccountDto::from(account))
        }
    }
}

impl<T> CreateAccountService for T where
    T: 'static
        + DependOnAccountQuery
        + DependOnAccountModifier
        + DependOnAuthAccountQuery
        + DependOnAuthAccountModifier
        + DependOnAuthHostQuery
        + DependOnAuthHostModifier
        + DependOnEventModifier
        + DependOnEventQuery
        + DependOnPasswordProvider
        + DependOnSigningKeyGenerator
{
}

pub trait UpdateAccountService:
    'static
    + DependOnAccountQuery
    + DependOnAccountModifier
    + DependOnEventQuery
    + DependOnEventModifier
{
    fn update_account(
        &self,
        account_id: AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;
            let account = self
                .account_query()
                .find_by_id(&mut transaction, &account_id)
                .await?;
            if let Some(account) = account {
                let event_id = EventId::from(account_id.clone());
                let events = self
                    .event_query()
                    .find_by_id(&mut transaction, &event_id, Some(account.version()))
                    .await?;
                if events.is_empty() {
                    return Ok(());
                }
                let mut account = Some(account);
                for event in events {
                    Account::apply(&mut account, event)?;
                }
                if let Some(account) = account {
                    self.account_modifier()
                        .update(&mut transaction, &account)
                        .await?;
                } else {
                    self.account_modifier()
                        .delete(&mut transaction, &account_id)
                        .await?;
                }
                Ok(())
            } else {
                Err(Report::new(KernelError::Internal)
                    .attach_printable(format!("Failed to get target account: {account_id:?}")))
            }
        }
    }
}

impl<T> UpdateAccountService for T where
    T: 'static
        + DependOnAccountQuery
        + DependOnAccountModifier
        + DependOnEventQuery
        + DependOnEventModifier
{
}

pub trait DeleteAccountService:
    'static
    + Sync
    + Send
    + DependOnAccountQuery
    + DependOnAuthAccountQuery
    + DependOnAuthAccountModifier
    + DependOnAuthHostQuery
    + DependOnAuthHostModifier
    + DependOnEventModifier
    + DependOnEventQuery
{
    fn delete_account<S>(
        &self,
        signal: &S,
        auth_info: AuthAccountInfo,
        account_id: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>>
    where
        S: Signal<AuthAccountId> + Signal<AccountId> + Send + Sync + 'static,
    {
        async move {
            // 認証アカウントを確認
            let auth_account = get_auth_account(self, signal, auth_info).await?;
            let mut transaction = self.database_connection().begin_transaction().await?;

            // Nanoidからアカウントを検索
            let nanoid = Nanoid::<Account>::new(account_id);
            let account = self
                .account_query()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            // 権限チェック (認証アカウントに紐づくアカウントのみ削除可能)
            let auth_account_id = auth_account.id().clone();
            let accounts = self
                .account_query()
                .find_by_auth_id(&mut transaction, &auth_account_id)
                .await?;

            let found = accounts.iter().any(|a| a.id() == account.id());
            if !found {
                return Err(Report::new(KernelError::PermissionDenied)
                    .attach_printable("This account does not belong to the authenticated user"));
            }

            // 削除イベントの生成と保存
            let account_id = account.id().clone();
            let delete_command = Account::delete(account_id.clone());

            self.event_modifier()
                .persist_and_transform(&mut transaction, delete_command)
                .await?;
            signal.emit(account_id).await?;

            Ok(())
        }
    }
}

impl<T> DeleteAccountService for T where
    T: 'static
        + DependOnAccountQuery
        + DependOnAuthAccountQuery
        + DependOnAuthAccountModifier
        + DependOnAuthHostQuery
        + DependOnAuthHostModifier
        + DependOnEventModifier
        + DependOnEventQuery
        + UpdateAccountService
{
}
