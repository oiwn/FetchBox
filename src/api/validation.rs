use std::collections::HashSet;
use thiserror::Error;

use super::models::Manifest;

#[derive(Debug, Error)]
pub enum ManifestValidationError {
    #[error("manifest_version must be 'v1'")]
    UnsupportedVersion,
    #[error("metadata must be an object")]
    InvalidMetadata,
    #[error("resources must contain between 1 and 1000 entries")]
    InvalidResourceCount,
    #[error("resource name '{0}' exceeds 128 characters")]
    ResourceNameTooLong(String),
    #[error("resource names must be unique")]
    DuplicateResourceNames,
    #[error("resource '{0}' must include an http/https url")]
    InvalidResourceUrl(String),
    #[error("resource '{0}' headers exceed limit of 10")]
    HeaderLimitExceeded(String),
    #[error("resource '{0}' header value '{1}' exceeds 1024 bytes")]
    HeaderValueTooLarge(String, String),
    #[error("tags for resource '{0}' exceed limit of 10")]
    TagLimitExceeded(String),
    #[error("tag value for resource '{0}' key '{1}' exceeds 1024 bytes")]
    TagValueTooLarge(String, String),
    #[error("attributes must be an object when present")]
    InvalidAttributes,
}

pub fn validate_manifest(manifest: &Manifest) -> Result<(), ManifestValidationError> {
    if manifest.manifest_version != "v1" {
        return Err(ManifestValidationError::UnsupportedVersion);
    }

    if !manifest.metadata.is_object() {
        return Err(ManifestValidationError::InvalidMetadata);
    }

    if !(1..=1000).contains(&manifest.resources.len()) {
        return Err(ManifestValidationError::InvalidResourceCount);
    }

    if let Some(attributes) = &manifest.attributes {
        if !attributes.is_object() {
            return Err(ManifestValidationError::InvalidAttributes);
        }
    }

    let mut seen = HashSet::new();
    for resource in &manifest.resources {
        if resource.name.len() > 128 {
            return Err(ManifestValidationError::ResourceNameTooLong(
                resource.name.clone(),
            ));
        }

        if !seen.insert(resource.name.clone()) {
            return Err(ManifestValidationError::DuplicateResourceNames);
        }

        if !resource.url.starts_with("http://") && !resource.url.starts_with("https://") {
            return Err(ManifestValidationError::InvalidResourceUrl(
                resource.name.clone(),
            ));
        }

        if resource.headers.len() > 10 {
            return Err(ManifestValidationError::HeaderLimitExceeded(
                resource.name.clone(),
            ));
        }

        for (key, value) in &resource.headers {
            if value.len() > 1024 {
                return Err(ManifestValidationError::HeaderValueTooLarge(
                    resource.name.clone(),
                    key.clone(),
                ));
            }

            if value.contains('\0') {
                return Err(ManifestValidationError::HeaderValueTooLarge(
                    resource.name.clone(),
                    key.clone(),
                ));
            }
        }

        if resource.tags.len() > 10 {
            return Err(ManifestValidationError::TagLimitExceeded(
                resource.name.clone(),
            ));
        }

        for (key, value) in &resource.tags {
            if value.len() > 1024 {
                return Err(ManifestValidationError::TagValueTooLarge(
                    resource.name.clone(),
                    key.clone(),
                ));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::Resource;
    use crate::handlers::types::HeadersMap;
    use serde_json::{Map, Value};

    #[test]
    fn validate_manifest_accepts_valid_payload() {
        let manifest = sample_manifest();
        assert!(validate_manifest(&manifest).is_ok());
    }

    #[test]
    fn validate_manifest_rejects_bad_version() {
        let mut manifest = sample_manifest();
        manifest.manifest_version = "v2".to_string();

        let err = validate_manifest(&manifest).unwrap_err();
        assert!(matches!(err, ManifestValidationError::UnsupportedVersion));
    }

    #[test]
    fn validate_manifest_limits_resource_count() {
        let mut manifest = sample_manifest();
        manifest.resources = vec![];

        let err = validate_manifest(&manifest).unwrap_err();
        assert!(matches!(err, ManifestValidationError::InvalidResourceCount));
    }

    fn sample_manifest() -> Manifest {
        Manifest {
            manifest_version: "v1".to_string(),
            metadata: Value::Object(Map::new()),
            resources: vec![Resource {
                name: "resource-1".to_string(),
                url: "https://example.com".to_string(),
                headers: HeadersMap::new(),
                tags: HeadersMap::new(),
            }],
            attributes: Some(Value::Object(Map::new())),
        }
    }
}
