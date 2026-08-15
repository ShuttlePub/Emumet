use crate::transfer::account::AccountFieldDto;
use adapter::processor::metadata::{
    CreateMetadataParam, DependOnMetadataCommandProcessor, MetadataCommandProcessor,
    UpdateMetadataParam,
};
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::read_model::MetadataProjection;
use kernel::interfaces::repository::{AggregateRepository, DependOnMetadataRepository};
use kernel::prelude::entity::{
    AccountId, EventVersion, Metadata, MetadataContent, MetadataId, MetadataLabel, Nanoid,
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

fn plan_field_updates(
    existing: &[MetadataProjection],
    submitted: &[AccountFieldDto],
) -> Vec<FieldUpdate> {
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
    executor: &mut <<T as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    account_id: &AccountId,
    existing: &[MetadataProjection],
    submitted: &[AccountFieldDto],
) -> error_stack::Result<(), KernelError>
where
    T: DependOnMetadataCommandProcessor + DependOnMetadataRepository + ?Sized,
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
            }
            FieldUpdate::Delete { metadata_id } => {
                let (_, current_version) = rehydrate_metadata(deps, executor, &metadata_id).await?;
                deps.metadata_command_processor()
                    .delete(executor, metadata_id.clone(), current_version)
                    .await?;
            }
            FieldUpdate::Create { label, content } => {
                deps.metadata_command_processor()
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
            }
        }
    }
    Ok(())
}

async fn rehydrate_metadata<T>(
    deps: &T,
    executor: &mut <<T as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    metadata_id: &MetadataId,
) -> error_stack::Result<(Metadata, EventVersion<Metadata>), KernelError>
where
    T: DependOnMetadataRepository + ?Sized,
{
    let rehydrated = deps
        .metadata_repository()
        .load(executor, metadata_id)
        .await?;
    Ok(rehydrated.into_parts())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::test_utils::MetadataBuilder;

    #[test]
    fn field_diff_pairs_by_index_and_updates_only_changed_pairs() {
        let existing = vec![
            MetadataProjection::from(
                MetadataBuilder::new()
                    .label("Website")
                    .content("old")
                    .build(),
            ),
            MetadataProjection::from(
                MetadataBuilder::new()
                    .label("GitHub")
                    .content("same")
                    .build(),
            ),
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
            MetadataProjection::from(MetadataBuilder::new().build()),
            MetadataProjection::from(MetadataBuilder::new().build()),
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
        let existing = vec![MetadataProjection::from(MetadataBuilder::new().build())];
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
