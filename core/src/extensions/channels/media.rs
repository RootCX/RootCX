use super::types::{ChannelError, MediaRef, MediaSource};
use crate::extensions::agents::attachments::{MAX_IMAGE_BYTES, validate};
use axum::body::Bytes;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;

// Inline bytes are authenticated by the relay signature; no caller URL is fetched.
pub(super) fn parse(payload: &Value) -> Result<Vec<MediaRef>, ChannelError> {
    let invalid = || ChannelError::InvalidWebhook("invalid channel attachment".into());
    if payload.get("images").is_some() {
        return Err(invalid());
    }
    let Some(images) = payload.get("attachments") else {
        return Ok(vec![]);
    };
    let images = images
        .as_array()
        .filter(|a| a.len() <= 1)
        .ok_or_else(invalid)?;
    images
        .iter()
        .map(|image| {
            let content_type = image["content_type"]
                .as_str()
                .filter(|m| matches!(*m, "image/png" | "image/jpeg"))
                .ok_or_else(invalid)?;
            let data = image["data"]
                .as_str()
                .filter(|s| s.len() <= MAX_IMAGE_BYTES.div_ceil(3) * 4)
                .ok_or_else(invalid)?;
            let bytes = STANDARD.decode(data).map_err(|_| invalid())?;
            validate(&bytes, content_type).map_err(|_| invalid())?;
            let name = image.get("name").and_then(Value::as_str).unwrap_or(
                if content_type == "image/png" {
                    "photo.png"
                } else {
                    "photo.jpg"
                },
            );
            if name.is_empty() || name.len() > 255 || name.chars().any(char::is_control) {
                return Err(invalid());
            }
            Ok(MediaRef {
                source: MediaSource::Inline(Bytes::from(bytes)),
                content_type: Some(content_type.into()),
                name: Some(name.into()),
            })
        })
        .collect()
}

pub(super) fn download(media: &MediaRef) -> Option<(Bytes, String, String)> {
    let MediaSource::Inline(bytes) = &media.source else {
        return None;
    };
    Some((
        bytes.clone(),
        media.content_type.clone()?,
        media.name.clone()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn signed_media_refuses_ambiguous_or_unbounded_delivery() {
        let png = STANDARD.encode([137, 80, 78, 71, 13, 10, 26, 10]);
        for images in [
            json!({}),
            json!([{}, {}]),
            json!([{"content_type":"image/png", "data":""}]),
            json!([{"content_type":"image/png", "data":"https://example.com/private"}]),
            json!([{"content_type":"image/jpeg", "data":png}]),
        ] {
            assert!(parse(&json!({"attachments":images})).is_err());
        }
        assert!(parse(&json!({"attachments":[],"images":[]})).is_err());
        let refs = parse(
            &json!({"attachments":[{"content_type":"image/png","data":png,"name":"chantier.png"}]}),
        )
        .unwrap();
        assert_eq!(download(&refs[0]).unwrap().2, "chantier.png");
        assert!(refs[0].provider_file_id().is_none());
    }
}
