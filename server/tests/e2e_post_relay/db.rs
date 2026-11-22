//! Direct database access of the test process (assertions, fault injection, time injection).
use driver::database::isolated_db::IsolatedDatabase;
use std::future::Future;
use std::time::{Duration, Instant};

pub struct Db {
    pub iso: IsolatedDatabase,
}

impl Db {
    pub async fn create() -> Self {
        super::stack::ensure_database_url();
        Self {
            iso: IsolatedDatabase::create().await,
        }
    }
    pub fn url(&self) -> String {
        let mut u = url::Url::parse(&std::env::var("DATABASE_URL").unwrap()).unwrap();
        u.set_path(self.iso.pool.connect_options().get_database().unwrap());
        u.to_string()
    }
    pub async fn i64(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql)
            .fetch_one(&self.iso.pool)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"))
    }
    pub async fn exec(&self, sql: &str) {
        sqlx::query(sql)
            .execute(&self.iso.pool)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    pub async fn count(&self, table: &str) -> i64 {
        self.i64(&format!("SELECT count(*) FROM {table}")).await
    }
    pub async fn profile_id(&self, nanoid: &str) -> i64 {
        sqlx::query_scalar("SELECT id FROM profiles WHERE nanoid=$1")
            .bind(nanoid)
            .fetch_one(&self.iso.pool)
            .await
            .unwrap()
    }
    /// Fixture shortcut: the API registers the link against a public-IP-literal HTTPS origin (the
    /// normal build refuses loopback); delivery to a loopback mock needs the stored link to
    /// point at it. Only the origin column changes; id, generation and ownership are untouched.
    pub async fn point_link_at(&self, link_id: &str, origin: &str) {
        sqlx::query("UPDATE profile_links SET origin=$2 WHERE id=$1")
            .bind(link_id)
            .bind(origin)
            .execute(&self.iso.pool)
            .await
            .unwrap();
    }
}

pub async fn eventually<F, Fut>(what: &str, bound: Duration, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let end = Instant::now() + bound;
    loop {
        if check().await {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}
