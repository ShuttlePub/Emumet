use crate::entity::{
    Account, AccountEvent, AccountId, AccountIsBot, AccountName, AccountPrivateKey,
    AccountPublicKey, AuthAccount, AuthAccountClientId, AuthAccountEvent, AuthAccountId,
    AuthHostId, CommandEnvelope, Metadata, MetadataContent, MetadataEvent, MetadataId,
    MetadataLabel, Nanoid, Profile, ProfileEvent, ProfileId,
};

use super::{
    DEFAULT_ACCOUNT_NAME, DEFAULT_CLIENT_ID, DEFAULT_METADATA_CONTENT, DEFAULT_METADATA_LABEL,
    DEFAULT_PRIVATE_KEY, DEFAULT_PUBLIC_KEY,
};

pub fn account_create_command(account_id: AccountId) -> CommandEnvelope<AccountEvent, Account> {
    crate::ensure_generator_initialized();
    Account::create(
        account_id,
        AccountName::new(DEFAULT_ACCOUNT_NAME),
        AccountPrivateKey::new(DEFAULT_PRIVATE_KEY),
        AccountPublicKey::new(DEFAULT_PUBLIC_KEY),
        AccountIsBot::new(false),
        Nanoid::default(),
        AuthAccountId::default(),
    )
}

pub fn profile_create_command(profile_id: ProfileId) -> CommandEnvelope<ProfileEvent, Profile> {
    crate::ensure_generator_initialized();
    Profile::create(
        profile_id,
        AccountId::default(),
        None,
        None,
        None,
        None,
        Nanoid::default(),
    )
}

pub fn metadata_create_command(
    metadata_id: MetadataId,
) -> CommandEnvelope<MetadataEvent, Metadata> {
    crate::ensure_generator_initialized();
    Metadata::create(
        metadata_id,
        AccountId::default(),
        MetadataLabel::new(DEFAULT_METADATA_LABEL),
        MetadataContent::new(DEFAULT_METADATA_CONTENT),
        Nanoid::default(),
    )
}

pub fn auth_account_create_command(
    id: AuthAccountId,
) -> CommandEnvelope<AuthAccountEvent, AuthAccount> {
    crate::ensure_generator_initialized();
    AuthAccount::create(
        id,
        AuthHostId::default(),
        AuthAccountClientId::new(DEFAULT_CLIENT_ID),
    )
}
