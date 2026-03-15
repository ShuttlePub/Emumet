use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{DependOnImageRepository, ImageRepository};
use kernel::prelude::entity::{Image, ImageBlurHash, ImageHash, ImageId, ImageUrl};
use kernel::KernelError;
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct ImageRow {
    id: i64,
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

    async fn find_by_ids(
        &self,
        executor: &mut Self::Executor,
        ids: &[ImageId],
    ) -> error_stack::Result<Vec<Image>, KernelError> {
        let con: &mut PgConnection = executor;
        let ids: Vec<i64> = ids.iter().map(|id| *id.as_ref()).collect();
        sqlx::query_as::<_, ImageRow>(
            // language=postgresql
            r#"
            SELECT id, url, hash, blurhash FROM images WHERE id = ANY($1)
            "#,
        )
        .bind(&ids)
        .fetch_all(con)
        .await
        .convert_error()
        .map(|rows| rows.into_iter().map(|row| row.into()).collect())
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
        let result = sqlx::query(
            // language=postgresql
            r#"
            DELETE FROM images WHERE id = $1
            "#,
        )
        .bind(image_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound)
                .attach_printable("Target image not found for delete"));
        }
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
    mod query {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::repository::{DependOnImageRepository, ImageRepository};
        use kernel::prelude::entity::ImageId;
        use kernel::test_utils::{unique_image_url, ImageBuilder};

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let id = ImageId::new(kernel::generate_id());
            let image = ImageBuilder::new().id(id.clone()).build();

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
        async fn find_by_ids_empty() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let result = database
                .image_repository()
                .find_by_ids(&mut transaction, &[])
                .await
                .unwrap();
            assert!(result.is_empty());
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_ids_multiple() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let image1 = ImageBuilder::new().hash("hash1").blur_hash("blur1").build();
            let image2 = ImageBuilder::new().hash("hash2").blur_hash("blur2").build();

            database
                .image_repository()
                .create(&mut transaction, &image1)
                .await
                .unwrap();
            database
                .image_repository()
                .create(&mut transaction, &image2)
                .await
                .unwrap();

            let result = database
                .image_repository()
                .find_by_ids(
                    &mut transaction,
                    &[image1.id().clone(), image2.id().clone()],
                )
                .await
                .unwrap();
            assert_eq!(result.len(), 2);
            assert!(result.contains(&image1));
            assert!(result.contains(&image2));

            database
                .image_repository()
                .delete(&mut transaction, image1.id())
                .await
                .unwrap();
            database
                .image_repository()
                .delete(&mut transaction, image2.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_ids_with_nonexistent() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let image = ImageBuilder::new().build();

            database
                .image_repository()
                .create(&mut transaction, &image)
                .await
                .unwrap();

            let nonexistent_id = ImageId::new(kernel::generate_id());
            let result = database
                .image_repository()
                .find_by_ids(&mut transaction, &[image.id().clone(), nonexistent_id])
                .await
                .unwrap();
            assert_eq!(result.len(), 1);
            assert!(result.contains(&image));

            database
                .image_repository()
                .delete(&mut transaction, image.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_url() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let url = unique_image_url();
            let image = ImageBuilder::new().url(url.as_ref()).build();

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
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::repository::{DependOnImageRepository, ImageRepository};
        use kernel::test_utils::ImageBuilder;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn create() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let image = ImageBuilder::new().build();

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
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let image = ImageBuilder::new().build();

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
    }
}
