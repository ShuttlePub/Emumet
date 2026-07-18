use crate::transfer::account::UpdateAccountDto;
use kernel::prelude::entity::{
    FieldAction, MetadataContent, MetadataLabel, ProfileDisplayName, ProfileSummary,
};
use kernel::KernelError;

pub(super) fn validate_update_account_dto(
    dto: &UpdateAccountDto,
) -> error_stack::Result<(), KernelError> {
    if let FieldAction::Set(value) = &dto.display_name {
        ProfileDisplayName::new(value.as_str()).validate()?;
    }
    if let FieldAction::Set(value) = &dto.summary {
        ProfileSummary::new(value.as_str()).validate()?;
    }
    if let Some(fields) = &dto.fields {
        for field in fields {
            MetadataLabel::new(field.label.as_str()).validate()?;
            MetadataContent::new(field.content.as_str()).validate()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::account::AccountFieldDto;

    fn dto_with(
        display_name: FieldAction<String>,
        summary: FieldAction<String>,
        fields: Option<Vec<AccountFieldDto>>,
    ) -> UpdateAccountDto {
        UpdateAccountDto {
            account_nanoid: "nanoid".to_string(),
            is_bot: FieldAction::Unchanged,
            display_name,
            summary,
            icon_url: FieldAction::Unchanged,
            banner_url: FieldAction::Unchanged,
            fields,
        }
    }

    #[test]
    fn validate_accepts_unchanged_dto() {
        let dto = dto_with(FieldAction::Unchanged, FieldAction::Unchanged, None);
        assert!(validate_update_account_dto(&dto).is_ok());
    }

    #[test]
    fn validate_accepts_values_at_limit() {
        let dto = dto_with(
            FieldAction::Set("あ".repeat(ProfileDisplayName::MAX_LENGTH)),
            FieldAction::Set("a".repeat(ProfileSummary::MAX_LENGTH)),
            Some(vec![AccountFieldDto {
                label: "l".repeat(MetadataLabel::MAX_LENGTH),
                content: "c".repeat(MetadataContent::MAX_LENGTH),
            }]),
        );
        assert!(validate_update_account_dto(&dto).is_ok());
    }

    #[test]
    fn validate_rejects_over_limit_display_name() {
        let dto = dto_with(
            FieldAction::Set("a".repeat(ProfileDisplayName::MAX_LENGTH + 1)),
            FieldAction::Unchanged,
            None,
        );
        let result = validate_update_account_dto(&dto);
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Display name must not exceed"));
    }

    #[test]
    fn validate_rejects_over_limit_summary() {
        let dto = dto_with(
            FieldAction::Unchanged,
            FieldAction::Set("a".repeat(ProfileSummary::MAX_LENGTH + 1)),
            None,
        );
        let result = validate_update_account_dto(&dto);
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Summary must not exceed"));
    }

    #[test]
    fn validate_rejects_over_limit_field_label() {
        let dto = dto_with(
            FieldAction::Unchanged,
            FieldAction::Unchanged,
            Some(vec![AccountFieldDto {
                label: "l".repeat(MetadataLabel::MAX_LENGTH + 1),
                content: "ok".to_string(),
            }]),
        );
        let result = validate_update_account_dto(&dto);
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Field label must not exceed"));
    }

    #[test]
    fn validate_rejects_over_limit_field_content() {
        let dto = dto_with(
            FieldAction::Unchanged,
            FieldAction::Unchanged,
            Some(vec![AccountFieldDto {
                label: "ok".to_string(),
                content: "c".repeat(MetadataContent::MAX_LENGTH + 1),
            }]),
        );
        let result = validate_update_account_dto(&dto);
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Field content must not exceed"));
    }
}
