use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::repository::{DependOnImageRepository, ImageRepository};
use kernel::prelude::entity::{Image, ImageBlurHash, ImageHash, ImageId, ImageUrl};
use kernel::KernelError;
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct ImageRow {
    id: Uuid,
    url: String,
    hash: String,
    blurhash: String,
}

impl From<ImageRow> for Image {
    fn from(row: ImageRow) -> Self {
        Self::new(
            ImageId::new(row.id),
            ImageUrl::new(row.url),
            ImageHash::new(row.hash),
            ImageBlurHash::new(row.blurhash),
        )
    }
}

pub struct PostgresImageRepository;

impl ImageRepository for PostgresImageRepository {
    type Executor = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &ImageId,
    ) -> error_stack::Result<Option<Image>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, ImageRow>(
            // language=postgresql
            r#"
            SELECT id, url, hash, blurhash FROM images WHERE id = $1
            "#,
        )
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(|row| row.into()))
    }

    async fn find_by_url(
        &self,
        executor: &mut Self::Executor,
        url: &ImageUrl,
    ) -> error_stack::Result<Option<Image>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, ImageRow>(
            // language=postgresql
            r#"
            SELECT id, url, hash, blurhash FROM images WHERE url = $1
            "#,
        )
        .bind(url.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(|row| row.into()))
    }

    async fn create(
        &self,
        executor: &mut Self::Executor,
        image: &Image,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            // language=postgresql
            r#"
            INSERT INTO images (id, url, hash, blurhash) VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(image.id().as_ref())
        .bind(image.url().as_ref())
        .bind(image.hash().as_ref())
        .bind(image.blur_hash().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn delete(
        &self,
        executor: &mut Self::Executor,
        image_id: &ImageId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            // language=postgresql
            r#"
            DELETE FROM images WHERE id = $1
            "#,
        )
        .bind(image_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

impl DependOnImageRepository for PostgresDatabase {
    type ImageRepository = PostgresImageRepository;

    fn image_repository(&self) -> &Self::ImageRepository {
        &PostgresImageRepository
    }
}

#[cfg(test)]
mod test {
    use kernel::prelude::entity::ImageUrl;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn url() -> ImageUrl {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        ImageUrl::new(format!(
            "https://example.com/{}",
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ))
    }

    mod query {
        use crate::database::postgres::image::test::url;
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::repository::{DependOnImageRepository, ImageRepository};
        use kernel::prelude::entity::{Image, ImageBlurHash, ImageHash, ImageId};
        use uuid::Uuid;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let id = ImageId::new(Uuid::now_v7());
            let url = url();
            let image = Image::new(
                id.clone(),
                url,
                ImageHash::new("hash".to_string()),
                ImageBlurHash::new("blur_hash".to_string()),
            );

            database
                .image_repository()
                .create(&mut transaction, &image)
                .await
                .unwrap();
            let result = database
                .image_repository()
                .find_by_id(&mut transaction, &id)
                .await
                .unwrap();
            assert_eq!(result, Some(image));
            database
                .image_repository()
                .delete(&mut transaction, &id)
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_url() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let id = ImageId::new(Uuid::now_v7());
            let url = url();
            let image = Image::new(
                id,
                url.clone(),
                ImageHash::new("hash".to_string()),
                ImageBlurHash::new("blur_hash".to_string()),
            );

            database
                .image_repository()
                .create(&mut transaction, &image)
                .await
                .unwrap();
            let result = database
                .image_repository()
                .find_by_url(&mut transaction, &url)
                .await
                .unwrap();
            assert_eq!(result, Some(image.clone()));
            database
                .image_repository()
                .delete(&mut transaction, image.id())
                .await
                .unwrap();
        }
    }

    mod modifier {
        use crate::database::postgres::image::test::url;
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::repository::{DependOnImageRepository, ImageRepository};
        use kernel::prelude::entity::{Image, ImageBlurHash, ImageHash, ImageId};
        use uuid::Uuid;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn create() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let id = ImageId::new(Uuid::now_v7());
            let url = url();
            let image = Image::new(
                id,
                url,
                ImageHash::new("hash".to_string()),
                ImageBlurHash::new("blur_hash".to_string()),
            );

            database
                .image_repository()
                .create(&mut transaction, &image)
                .await
                .unwrap();
            database
                .image_repository()
                .delete(&mut transaction, image.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn delete() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let id = ImageId::new(Uuid::now_v7());
            let url = url();
            let image = Image::new(
                id.clone(),
                url,
                ImageHash::new("hash".to_string()),
                ImageBlurHash::new("blur_hash".to_string()),
            );

            database
                .image_repository()
                .create(&mut transaction, &image)
                .await
                .unwrap();
            database
                .image_repository()
                .delete(&mut transaction, &id)
                .await
                .unwrap();
        }
    }
}
