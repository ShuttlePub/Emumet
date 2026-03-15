mod account;
mod auth_account;
mod auth_host;
mod event;
mod follow;
mod image;
mod metadata;
mod profile;
mod remote_account;

pub use self::account::*;
pub use self::auth_account::*;
pub use self::auth_host::*;
pub use self::event::*;
pub use self::follow::*;
pub use self::image::*;
pub use self::metadata::*;
pub use self::profile::*;
pub use self::remote_account::*;

use crate::entity::{AccountName, AuthHostUrl, ImageUrl, RemoteAccountAcct, RemoteAccountUrl};
use std::sync::atomic::{AtomicUsize, Ordering};

pub const DEFAULT_ACCOUNT_NAME: &str = "alice";
pub const DEFAULT_PRIVATE_KEY: &str =
    "-----BEGIN RSA PRIVATE KEY-----\ntest-key-data\n-----END RSA PRIVATE KEY-----";
pub const DEFAULT_PUBLIC_KEY: &str =
    "-----BEGIN PUBLIC KEY-----\ntest-key-data\n-----END PUBLIC KEY-----";
pub const DEFAULT_DISPLAY_NAME: &str = "Alice Wonderland";
pub const DEFAULT_SUMMARY: &str = "Hello! I'm a test user on ShuttlePub.";
pub const DEFAULT_METADATA_LABEL: &str = "Website";
pub const DEFAULT_METADATA_CONTENT: &str = "https://example.com";
pub const DEFAULT_CLIENT_ID: &str = "550e8400-e29b-41d4-a716-446655440000";
pub const DEFAULT_IMAGE_HASH: &str = "sha256:e3b0c44298fc1c149afbf4c8996fb924";
pub const DEFAULT_BLUR_HASH: &str = "LEHV6nWB2yk8pyo0adR*.7kCMdnj";

static UNIQUE_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn next_unique() -> usize {
    UNIQUE_COUNTER.fetch_add(1, Ordering::SeqCst)
}

pub fn unique_account_name() -> AccountName {
    AccountName::new(format!("user-{}", next_unique()))
}

pub fn unique_image_url() -> ImageUrl {
    ImageUrl::new(format!("https://img.example.com/{}", next_unique()))
}

pub fn unique_auth_host_url() -> AuthHostUrl {
    AuthHostUrl::new(format!("https://auth-{}.example.com", next_unique()))
}

pub fn unique_remote_acct() -> (RemoteAccountAcct, RemoteAccountUrl) {
    let n = next_unique();
    (
        RemoteAccountAcct::new(format!("remote-{}@example.com", n)),
        RemoteAccountUrl::new(format!("https://remote.example.com/users/{}", n)),
    )
}
