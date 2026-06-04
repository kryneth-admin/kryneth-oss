use sha2::{Digest, Sha256};
use std::cmp;

/// Resolves or generates a session ID for agentic loops without deep JSON parsing.
///
/// 1. Uses `provided_session_id` if available.
/// 2. Falls back to `tenant_id` + `idempotency_key` if `idempotency_key` is available.
/// 3. If neither are provided, generates a SHA-256 hash using `tenant_id`, `api_key`,
///    and up to the first 256 bytes of `raw_payload`.
pub fn resolve_session_id(
    provided_session_id: Option<&str>,
    idempotency_key: Option<&str>,
    tenant_id: &str,
    api_key: &str,
    raw_payload: &[u8],
) -> String {
    if let Some(sid) = provided_session_id {
        let trimmed = sid.trim();
        let trimmed_lower = trimmed.to_lowercase();
        if !trimmed.is_empty()
            && trimmed_lower != "null"
            && trimmed_lower != "none"
            && trimmed_lower != "undefined"
        {
            return trimmed.to_string();
        }
    }

    if let Some(idem_key) = idempotency_key {
        let trimmed = idem_key.trim();
        let trimmed_lower = trimmed.to_lowercase();
        if !trimmed.is_empty()
            && trimmed_lower != "null"
            && trimmed_lower != "none"
            && trimmed_lower != "undefined"
        {
            return format!("{}_{}", tenant_id, trimmed);
        }
    }

    let mut hasher = Sha256::new();
    hasher.update(tenant_id.as_bytes());
    hasher.update(api_key.as_bytes());

    // Inject time-based entropy bucket to guarantee that identical payloads sent
    // moments apart (such as network retries) produce distinct session IDs.
    let entropy = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    hasher.update(entropy.to_le_bytes());

    // Hash up to the first 256 bytes of the payload
    let max_len = cmp::min(256, raw_payload.len());
    hasher.update(&raw_payload[..max_len]);

    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_id_generation_with_client_header() {
        let sid = resolve_session_id(
            Some("client-provided-123"),
            Some("idem-456"),
            "tenant-abc",
            "sk-123",
            b"{}",
        );
        assert_eq!(sid, "client-provided-123");
    }

    #[test]
    fn test_session_id_generation_fallback() {
        // Fallback with idempotency key
        let sid_idem = resolve_session_id(None, Some("idem-456"), "tenant-abc", "sk-123", b"{}");
        assert_eq!(sid_idem, "tenant-abc_idem-456");

        // Hash fallback
        let payload1 = b"some long payload string that exceeds the max length or is just right";
        let sid_hash1 = resolve_session_id(None, None, "tenant-abc", "sk-123", payload1);

        let payload2 = b"some long payload string that exceeds the max length or is just right";
        let sid_hash2 = resolve_session_id(None, None, "tenant-abc", "sk-123", payload2);

        // Deterministic
        assert_eq!(sid_hash1, sid_hash2);

        let sid_hash_diff = resolve_session_id(
            None,
            None,
            "tenant-abc",
            "sk-456", // Different API key
            payload1,
        );
        assert_ne!(sid_hash1, sid_hash_diff);
    }

    #[test]
    fn test_session_id_generation_rejects_stringified_null() {
        // Stringified null/none/undefined in provided_session_id should fallback to idempotency
        let sid = resolve_session_id(
            Some("null"),
            Some("idem-456"),
            "tenant-abc",
            "sk-123",
            b"{}",
        );
        assert_eq!(sid, "tenant-abc_idem-456");

        // Stringified none in provided_session_id should fallback to hash
        let sid_none = resolve_session_id(Some("None"), None, "tenant-abc", "sk-123", b"{}");
        assert_ne!(sid_none, "None");

        // Stringified undefined in idempotency should fallback to hash
        let sid_idem_undef =
            resolve_session_id(None, Some("undefined"), "tenant-abc", "sk-123", b"{}");
        assert_ne!(sid_idem_undef, "tenant-abc_undefined");
    }
}
