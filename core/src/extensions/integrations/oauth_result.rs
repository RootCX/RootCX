//! Optional host-owned OAuth result presentation. This receipt grants no authority.
use axum::response::{IntoResponse, Redirect, Response};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub(super) fn configured_url() -> Option<String> {
    let destination = std::env::var("ROOTCX_OAUTH_RESULT_URL").ok()?;
    let callback = std::env::var("OAUTH_CALLBACK_URL").ok()?;
    std::env::var("OAUTH_CALLBACK_HMAC_KEY")
        .ok()
        .filter(|key| !key.is_empty())?;
    std::env::var("ROOTCX_TENANT_REF").ok()?;
    trusted_destination(&destination, &callback)
}

fn trusted_destination(destination: &str, callback: &str) -> Option<String> {
    let target = url::Url::parse(destination).ok()?;
    let source = url::Url::parse(callback).ok()?;
    if !matches!(target.scheme(), "https" | "http")
        || target.host_str().is_none()
        || target.origin() != source.origin()
        || !target.username().is_empty()
        || target.password().is_some()
        || target.query().is_some()
        || target.fragment().is_some()
    {
        return None;
    }
    Some(target.into())
}

pub(super) fn redirect(destination: &str, provider: &str, status: &str) -> Option<Response> {
    let key = std::env::var("OAUTH_CALLBACK_HMAC_KEY").ok()?;
    let tenant = std::env::var("ROOTCX_TENANT_REF").ok()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let receipt = receipt(&key, &tenant, provider, status, now)?;
    let mut target = url::Url::parse(destination).ok()?;
    target.query_pairs_mut().append_pair("receipt", &receipt);
    Some(Redirect::to(target.as_str()).into_response())
}

fn receipt(key: &str, tenant: &str, provider: &str, status: &str, now: u64) -> Option<String> {
    if key.is_empty() || !matches!(status, "connected" | "cancelled" | "failed") {
        return None;
    }
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&serde_json::json!({
            "tenant": tenant, "provider": provider, "status": status, "issuedAt": now,
        }))
        .ok()?,
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).ok()?;
    mac.update(format!("oauth-result-v1:{payload}").as_bytes());
    Some(format!(
        "{payload}.{}",
        hex::encode(mac.finalize().into_bytes())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_destination_is_operator_owned_and_same_origin() {
        let callback = "https://host.example/api/callback";
        assert_eq!(
            trusted_destination("https://host.example/api/result", callback).as_deref(),
            Some("https://host.example/api/result")
        );
        for invalid in [
            "",
            "//evil.example/result",
            "https://evil.example/result",
            "http://host.example/result",
            "https://host.example:444/result",
            "https://user:secret@host.example/result",
            "https://host.example/result?next=evil",
            "https://host.example/result#fragment",
            "javascript:alert(1)",
        ] {
            assert!(
                trusted_destination(invalid, callback).is_none(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn receipt_is_domain_separated_and_only_contains_display_metadata() {
        let receipt = receipt("tenant-key", "tenant", "notion", "connected", 1234).unwrap();
        let (payload, signature) = receipt.split_once('.').unwrap();
        let decoded: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
        assert_eq!(
            decoded,
            serde_json::json!({"tenant":"tenant","provider":"notion","status":"connected","issuedAt":1234})
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(b"tenant-key").unwrap();
        mac.update(format!("oauth-result-v1:{payload}").as_bytes());
        mac.verify_slice(&hex::decode(signature).unwrap()).unwrap();
        let mut wrong_purpose = Hmac::<Sha256>::new_from_slice(b"tenant-key").unwrap();
        wrong_purpose.update(payload.as_bytes());
        assert!(
            wrong_purpose
                .verify_slice(&hex::decode(signature).unwrap())
                .is_err()
        );
    }
}
