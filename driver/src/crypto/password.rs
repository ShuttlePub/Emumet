use error_stack::{Report, Result};
use kernel::interfaces::crypto::PasswordProvider;
use kernel::KernelError;
use std::path::Path;
use zeroize::Zeroizing;

const SECRETS_PATH: &str = "/run/secrets/master-key-password";
const FALLBACK_PATH: &str = "./master-key-password";

/// Validate file permissions (Unix only)
/// Rejects files with group or other permissions (must be 0o600 or 0o400)
#[cfg(unix)]
fn validate_file_permissions(path: &str) -> Result<(), KernelError> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(path).map_err(|e| {
        Report::new(KernelError::Internal).attach_printable(format!(
            "Failed to read file metadata for '{}': {}",
            path, e
        ))
    })?;

    let mode = metadata.permissions().mode();
    // Check if group or other have any permissions
    if mode & 0o077 != 0 {
        return Err(Report::new(KernelError::Internal).attach_printable(format!(
            "Master password file '{}' has insecure permissions: {:o} (expected 0o600 or 0o400)",
            path, mode
        )));
    }

    Ok(())
}

#[cfg(not(unix))]
fn validate_file_permissions(_path: &str) -> Result<(), KernelError> {
    // Skip permission check on non-Unix systems
    Ok(())
}

/// File-based password provider with fallback support
///
/// Tries to read from `/run/secrets/master-key-password` first,
/// then falls back to `./master-key-password` if not found.
///
/// On Unix systems, validates that the file has secure permissions (0o600 or 0o400).
#[derive(Clone)]
pub struct FilePasswordProvider {
    secrets_path: String,
    fallback_path: String,
}

impl FilePasswordProvider {
    /// Create a new provider with default paths
    pub fn new() -> Self {
        Self {
            secrets_path: SECRETS_PATH.to_string(),
            fallback_path: FALLBACK_PATH.to_string(),
        }
    }

    /// Create a provider with custom paths (useful for testing)
    pub fn with_paths<P1: AsRef<Path>, P2: AsRef<Path>>(
        secrets_path: P1,
        fallback_path: P2,
    ) -> Self {
        Self {
            secrets_path: secrets_path.as_ref().to_string_lossy().into_owned(),
            fallback_path: fallback_path.as_ref().to_string_lossy().into_owned(),
        }
    }
}

impl Default for FilePasswordProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl PasswordProvider for FilePasswordProvider {
    fn get_password(&self) -> Result<Zeroizing<Vec<u8>>, KernelError> {
        // Try secrets path first
        if Path::new(&self.secrets_path).exists() {
            validate_file_permissions(&self.secrets_path)?;
            let password = std::fs::read(&self.secrets_path).map_err(|e| {
                Report::new(KernelError::Internal).attach_printable(format!(
                    "Failed to read master password from '{}': {}",
                    self.secrets_path, e
                ))
            })?;
            if password.is_empty() {
                return Err(Report::new(KernelError::Internal)
                    .attach_printable("Master password file is empty"));
            }
            return Ok(Zeroizing::new(password));
        }

        // Fall back to local path
        if Path::new(&self.fallback_path).exists() {
            validate_file_permissions(&self.fallback_path)?;
            let password = std::fs::read(&self.fallback_path).map_err(|e| {
                Report::new(KernelError::Internal).attach_printable(format!(
                    "Failed to read master password from '{}': {}",
                    self.fallback_path, e
                ))
            })?;
            if password.is_empty() {
                return Err(Report::new(KernelError::Internal)
                    .attach_printable("Master password file is empty"));
            }
            return Ok(Zeroizing::new(password));
        }

        Err(Report::new(KernelError::Internal).attach_printable(format!(
            "Master password file not found. Tried: '{}', '{}'",
            self.secrets_path, self.fallback_path
        )))
    }
}

/// In-memory password provider for testing
#[cfg(test)]
pub struct InMemoryPasswordProvider {
    password: Vec<u8>,
}

#[cfg(test)]
impl InMemoryPasswordProvider {
    pub fn new(password: impl Into<Vec<u8>>) -> Self {
        Self {
            password: password.into(),
        }
    }
}

#[cfg(test)]
impl PasswordProvider for InMemoryPasswordProvider {
    fn get_password(&self) -> Result<Zeroizing<Vec<u8>>, KernelError> {
        Ok(Zeroizing::new(self.password.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_in_memory_provider() {
        let provider = InMemoryPasswordProvider::new(b"test-password".to_vec());
        let password = provider.get_password().unwrap();
        assert_eq!(password.as_slice(), b"test-password");
    }

    #[test]
    fn test_file_provider_with_custom_path() {
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(b"file-password").unwrap();

        let provider = FilePasswordProvider::with_paths("/nonexistent/path", temp_file.path());

        let password = provider.get_password().unwrap();
        assert_eq!(password.as_slice(), b"file-password");
    }

    #[test]
    fn test_file_provider_no_file() {
        let provider =
            FilePasswordProvider::with_paths("/nonexistent/secrets", "/nonexistent/fallback");

        let result = provider.get_password();
        assert!(result.is_err());
    }
}
