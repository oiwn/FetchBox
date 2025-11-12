//! API utility functions
//!
//! Pure, stateless helper functions for HTTP request processing.
//! These functions are extracted from services.rs to enable unit testing
//! and reusability across different handlers.

use crate::api::error::ApiError;

/// Parses and validates Content-Type header for application/json
///
/// Accepts:
/// - `application/json`
/// - `application/json; charset=utf-8`
///
/// Rejects:
/// - `application/jsonp`
/// - `application/json-patch+json`
/// - `text/json`
/// - Malformed media types
pub fn parse_content_type(content_type: &str) -> Result<mime::Mime, ApiError> {
    let media_type: mime::Mime = content_type.parse().map_err(|_| {
        ApiError::InvalidPayload(format!("invalid Content-Type: {}", content_type))
    })?;

    if media_type.type_() != mime::APPLICATION || media_type.subtype() != mime::JSON {
        return Err(ApiError::InvalidPayload(format!(
            "Content-Type must be application/json, got: {}/{}",
            media_type.type_(),
            media_type.subtype()
        )));
    }

    Ok(media_type)
}

/// Validates that body size does not exceed the maximum allowed size
pub fn validate_body_size(data: &[u8], max_size: usize) -> Result<(), ApiError> {
    if data.len() > max_size {
        return Err(ApiError::PayloadTooLarge(data.len()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_content_type_valid() {
        assert!(parse_content_type("application/json").is_ok());
        assert!(parse_content_type("application/json; charset=utf-8").is_ok());
        assert!(parse_content_type("application/json; charset=UTF-8").is_ok());
    }

    #[test]
    fn test_parse_content_type_invalid() {
        assert!(parse_content_type("application/jsonp").is_err());
        assert!(parse_content_type("application/json-patch+json").is_err());
        assert!(parse_content_type("text/json").is_err());
        assert!(parse_content_type("text/plain").is_err());
        assert!(parse_content_type("invalid").is_err());
        assert!(parse_content_type("").is_err());
    }

    #[test]
    fn test_validate_body_size_ok() {
        let data = vec![0u8; 1000];
        assert!(validate_body_size(&data, 1000).is_ok());
        assert!(validate_body_size(&data, 2000).is_ok());
        assert!(validate_body_size(&[], 100).is_ok());
    }

    #[test]
    fn test_validate_body_size_too_large() {
        let data = vec![0u8; 1000];
        let result = validate_body_size(&data, 999);
        assert!(result.is_err());
        match result {
            Err(ApiError::PayloadTooLarge(size)) => assert_eq!(size, 1000),
            _ => panic!("Expected PayloadTooLarge error"),
        }
    }
}
