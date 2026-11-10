mod endpoint;
mod sender;
pub mod signature;
#[cfg(test)]
mod tests;

pub(crate) use endpoint::{is_forbidden_domain, is_forbidden_ip};
pub use endpoint::{ENV_SECRET, ENV_SECRET_FILE, ENV_URL, MIN_SECRET_BYTES};
pub(crate) use sender::parse_retry_after;
pub use sender::{
    HostResolver, HttpWebhookSender, SystemResolver, WebhookNetworkSettings,
    HEADER_DELIVERY_ATTEMPT, HEADER_EVENT_ID, HEADER_EVENT_TYPE, HEADER_SIGNATURE,
    HEADER_TIMESTAMP,
};
