use axum::http::{header, HeaderMap, HeaderValue};
use chrono::{DateTime, Utc};

/// Compute a stable quoted ETag from response bytes using FNV-1a (64-bit).
///
/// `std::hash::DefaultHasher` is intentionally non-stable across Rust versions
/// and process restarts, which would cause ETags to change on every deploy even
/// when the response body is identical.  FNV-1a is a simple, well-known hash
/// with a fixed algorithm that produces the same digest in every build.
pub fn compute_etag(body: &[u8]) -> String {
    const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
    const FNV_PRIME: u64 = 1_099_511_628_211;

    let hash = body.iter().fold(FNV_OFFSET, |acc, &byte| {
        acc ^ (byte as u64)
    }).wrapping_mul(FNV_PRIME);

    format!("\"{:x}\"", hash)
}

/// Build a Cache-Control value using max-age and stale-while-revalidate.
pub fn cache_control(max_age: u32, swr: u32) -> HeaderValue {
    // Inputs are u32 — the formatted string is always valid ASCII.
    HeaderValue::from_str(&format!(
        "max-age={}, stale-while-revalidate={}",
        max_age, swr
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("no-store"))
}

/// Build an RFC 7231 HTTP-date for Last-Modified.
pub fn last_modified(timestamp: DateTime<Utc>) -> HeaderValue {
    let formatted = timestamp.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    HeaderValue::from_str(&formatted).unwrap_or_else(|_| HeaderValue::from_static("0"))
}

/// Returns true when `If-None-Match` contains `*` or the exact current ETag.
pub fn if_none_match_matches(headers: &HeaderMap, current_etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(|raw| {
            raw.split(',')
                .map(|tag| tag.trim())
                .any(|tag| tag == "*" || tag == current_etag)
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etag_is_quoted() {
        let etag = compute_etag(br#"{"ok":true}"#);
        assert!(etag.starts_with('"'));
        assert!(etag.ends_with('"'));
    }

    #[test]
    fn if_none_match_matches_exact_tag() {
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("\"abc\""));

        assert!(if_none_match_matches(&headers, "\"abc\""));
        assert!(!if_none_match_matches(&headers, "\"def\""));
    }
}
