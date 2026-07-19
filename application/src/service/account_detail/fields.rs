use crate::service::metadata::rehydrate_metadata;
use crate::transfer::account::AccountFieldDto;
use adapter::processor::metadata::{
    CreateMetadataParam, DependOnMetadataCommandProcessor, MetadataCommandProcessor,
    UpdateMetadataParam,
};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::event_store::DependOnMetadataEventStore;
use kernel::interfaces::read_model::{DependOnMetadataReadModel, MetadataReadModel};
use kernel::prelude::entity::{
    AccountId, Metadata, MetadataContent, MetadataId, MetadataLabel, Nanoid,
};
use kernel::KernelError;

#[derive(Debug, Eq, PartialEq)]
enum FieldUpdate {
    Update {
        metadata_id: MetadataId,
        label: String,
        content: String,
    },
    Delete {
        metadata_id: MetadataId,
    },
    Create {
        label: String,
        content: String,
    },
}

fn plan_field_updates(existing: &[Metadata], submitted: &[AccountFieldDto]) -> Vec<FieldUpdate> {
    let paired = existing.len().min(submitted.len());
    let mut operations = Vec::new();
    for index in 0..paired {
        let current = &existing[index];
        let next = &submitted[index];
        if current.label().as_ref() != &next.label || current.content().as_ref() != &next.content {
            operations.push(FieldUpdate::Update {
                metadata_id: current.id().clone(),
                label: next.label.clone(),
                content: next.content.clone(),
            });
        }
    }
    operations.extend(existing[paired..].iter().map(|field| FieldUpdate::Delete {
        metadata_id: field.id().clone(),
    }));
    operations.extend(submitted[paired..].iter().map(|field| FieldUpdate::Create {
        label: field.label.clone(),
        content: field.content.clone(),
    }));
    operations
}

pub(super) async fn apply_field_updates<T>(
    deps: &T,
    executor: &mut <<T as kernel::interfaces::database::DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
    account_id: &AccountId,
    existing: &[Metadata],
    submitted: &[AccountFieldDto],
) -> error_stack::Result<(), KernelError>
where
    T: DependOnMetadataCommandProcessor
        + DependOnMetadataEventStore
        + DependOnMetadataReadModel
        + ?Sized,
{
    for operation in plan_field_updates(existing, submitted) {
        match operation {
            FieldUpdate::Update {
                metadata_id,
                label,
                content,
            } => {
                let (_, current_version) = rehydrate_metadata(deps, executor, &metadata_id).await?;
                deps.metadata_command_processor()
                    .update(
                        executor,
                        UpdateMetadataParam {
                            metadata_id: metadata_id.clone(),
                            label: MetadataLabel::new(label),
                            content: MetadataContent::new(content),
                            current_version,
                        },
                    )
                    .await?;
                let (metadata, _) = rehydrate_metadata(deps, executor, &metadata_id).await?;
                deps.metadata_read_model()
                    .update(executor, &metadata)
                    .await?;
            }
            FieldUpdate::Delete { metadata_id } => {
                let (_, current_version) = rehydrate_metadata(deps, executor, &metadata_id).await?;
                deps.metadata_command_processor()
                    .delete(executor, metadata_id.clone(), current_version)
                    .await?;
                deps.metadata_read_model()
                    .delete(executor, &metadata_id)
                    .await?;
            }
            FieldUpdate::Create { label, content } => {
                let metadata = deps
                    .metadata_command_processor()
                    .create(
                        executor,
                        CreateMetadataParam {
                            account_id: account_id.clone(),
                            label: MetadataLabel::new(label),
                            content: MetadataContent::new(content),
                            nano_id: Nanoid::<Metadata>::default(),
                        },
                    )
                    .await?;
                deps.metadata_read_model()
                    .create(executor, &metadata)
                    .await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::test_utils::MetadataBuilder;

    #[test]
    fn field_diff_pairs_by_index_and_updates_only_changed_pairs() {
        let existing = vec![
            MetadataBuilder::new()
                .label("Website")
                .content("old")
                .build(),
            MetadataBuilder::new()
                .label("GitHub")
                .content("same")
                .build(),
        ];
        let submitted = vec![
            AccountFieldDto {
                label: "Website".into(),
                content: "new".into(),
            },
            AccountFieldDto {
                label: "GitHub".into(),
                content: "same".into(),
            },
        ];
        let operations = plan_field_updates(&existing, &submitted);
        assert_eq!(operations.len(), 1);
        assert!(
            matches!(&operations[0], FieldUpdate::Update { label, content, .. } if label == "Website" && content == "new")
        );
    }

    #[test]
    fn field_diff_deletes_existing_items_left_after_pairing() {
        let existing = vec![
            MetadataBuilder::new().build(),
            MetadataBuilder::new().build(),
        ];
        let submitted = vec![AccountFieldDto {
            label: "Website".into(),
            content: "content".into(),
        }];
        assert!(matches!(
            plan_field_updates(&existing, &submitted).last(),
            Some(FieldUpdate::Delete { .. })
        ));
    }

    #[test]
    fn field_diff_creates_submitted_items_left_after_pairing() {
        let existing = vec![MetadataBuilder::new().build()];
        let submitted = vec![
            AccountFieldDto {
                label: "Website".into(),
                content: "content".into(),
            },
            AccountFieldDto {
                label: "GitHub".into(),
                content: "github.example".into(),
            },
        ];
        assert!(matches!(
            plan_field_updates(&existing, &submitted).last(),
            Some(FieldUpdate::Create { label, content }) if label == "GitHub" && content == "github.example"
        ));
    }
}
