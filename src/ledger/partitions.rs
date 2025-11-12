/// Key layout and encoding utilities for Fjall partitions
///
/// Partition structure:
/// - `jobs`: job:{job_id} -> JobSnapshot (JSON)
/// - `logs`: log:{job_id}:{offset:016} -> LogEntry (JSON)
/// - `idempotency`: idem:{key} -> job_id (string)
/// - `metadata`: meta:{key} -> value (JSON/string)

/// Encode a job key: job:{job_id}
pub fn encode_job_key(job_id: &str) -> Vec<u8> {
    format!("job:{}", job_id).into_bytes()
}

/// Decode a job key: job:{job_id} -> job_id
pub fn decode_job_key(key: &[u8]) -> Option<String> {
    let key_str = std::str::from_utf8(key).ok()?;
    key_str.strip_prefix("job:").map(String::from)
}

/// Encode a log key: log:{job_id}:{offset:016}
pub fn encode_log_key(job_id: &str, offset: u64) -> Vec<u8> {
    format!("log:{}:{:016}", job_id, offset).into_bytes()
}

/// Encode a log prefix for range scan: log:{job_id}:
pub fn encode_log_prefix(job_id: &str) -> Vec<u8> {
    format!("log:{}:", job_id).into_bytes()
}

/// Decode a log key: log:{job_id}:{offset:016} -> (job_id, offset)
pub fn decode_log_key(key: &[u8]) -> Option<(String, u64)> {
    let key_str = std::str::from_utf8(key).ok()?;
    let parts: Vec<&str> = key_str.strip_prefix("log:")?.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let job_id = parts[0].to_string();
    let offset = parts[1].parse().ok()?;
    Some((job_id, offset))
}

/// Encode an idempotency key: idem:{key}
pub fn encode_idem_key(key: &str) -> Vec<u8> {
    format!("idem:{}", key).into_bytes()
}

/// Encode a metadata key: meta:{key}
pub fn encode_meta_key(key: &str) -> Vec<u8> {
    format!("meta:{}", key).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_key_encoding() {
        let job_id = "job_123";
        let key = encode_job_key(job_id);
        assert_eq!(key, b"job:job_123");

        let decoded = decode_job_key(&key).unwrap();
        assert_eq!(decoded, job_id);
    }

    #[test]
    fn test_log_key_encoding() {
        let job_id = "job_123";
        let offset = 42u64;
        let key = encode_log_key(job_id, offset);
        assert_eq!(key, b"log:job_123:0000000000000042");

        let (decoded_job_id, decoded_offset) = decode_log_key(&key).unwrap();
        assert_eq!(decoded_job_id, job_id);
        assert_eq!(decoded_offset, offset);
    }

    #[test]
    fn test_log_prefix() {
        let job_id = "job_123";
        let prefix = encode_log_prefix(job_id);
        assert_eq!(prefix, b"log:job_123:");
    }

    #[test]
    fn test_idem_key_encoding() {
        let key = encode_idem_key("test-key");
        assert_eq!(key, b"idem:test-key");
    }

    #[test]
    fn test_meta_key_encoding() {
        let key = encode_meta_key("last_prune");
        assert_eq!(key, b"meta:last_prune");
    }
}
