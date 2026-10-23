use super::types::{ChannelError, MediaRef, MediaSource};
use crate::extensions::agents::attachments::{MAX_IMAGE_BYTES, validate};
use axum::body::Bytes;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;

// Managed channels relay at most three files: photos and PDF documents. The
// portal bounds the same limits before signing; inline bytes are authenticated by
// the relay signature, so no caller URL is ever fetched.
pub(super) const MAX_CHANNEL_ATTACHMENTS: usize = 3;
pub(super) const MAX_DOCUMENT_BYTES: usize = 10 * 1024 * 1024;

pub(super) fn parse(payload: &Value) -> Result<Vec<MediaRef>, ChannelError> {
    let invalid = || ChannelError::InvalidWebhook("invalid channel attachment".into());
    if payload.get("images").is_some() {
        return Err(invalid());
    }
    let Some(files) = payload.get("attachments") else {
        return Ok(vec![]);
    };
    let files = files
        .as_array()
        .filter(|a| a.len() <= MAX_CHANNEL_ATTACHMENTS)
        .ok_or_else(invalid)?;
    files
        .iter()
        .map(|file| {
            let content_type = file["content_type"]
                .as_str()
                .filter(|m| matches!(*m, "image/png" | "image/jpeg" | "application/pdf"))
                .ok_or_else(invalid)?;
            let limit = if content_type == "application/pdf" { MAX_DOCUMENT_BYTES } else { MAX_IMAGE_BYTES };
            let data = file["data"]
                .as_str()
                .filter(|s| s.len() <= limit.div_ceil(3) * 4)
                .ok_or_else(invalid)?;
            let bytes = STANDARD.decode(data).map_err(|_| invalid())?;
            if content_type == "application/pdf" && !bytes.starts_with(b"%PDF-") {
                return Err(invalid());
            }
            validate(&bytes, content_type).map_err(|_| invalid())?;
            let fallback = match content_type {
                "image/png" => "photo.png",
                "image/jpeg" => "photo.jpg",
                _ => "document.pdf",
            };
            let name = file.get("name").and_then(Value::as_str).unwrap_or(fallback);
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

    #[test]
    fn signed_media_carries_up_to_three_photos_and_pdfs() {
        let png = STANDARD.encode([137, 80, 78, 71, 13, 10, 26, 10]);
        let pdf = STANDARD.encode(b"%PDF-1.7 devis");
        let photo = json!({"content_type":"image/png","data":png});
        let quote = json!({"content_type":"application/pdf","data":pdf,"name":"ancien-devis.pdf"});
        let refs = parse(&json!({"attachments":[photo, quote, quote]})).unwrap();
        assert_eq!(refs.len(), 3);
        let (_, content_type, name) = download(&refs[1]).unwrap();
        assert_eq!((content_type.as_str(), name.as_str()), ("application/pdf", "ancien-devis.pdf"));
        assert!(parse(&json!({"attachments":[photo, quote, quote, photo]})).is_err());
        let fake = json!({"content_type":"application/pdf","data":STANDARD.encode(b"<html>")});
        assert!(parse(&json!({"attachments":[fake]})).is_err());
        let unnamed = parse(&json!({"attachments":[{"content_type":"application/pdf","data":pdf}]})).unwrap();
        assert_eq!(download(&unnamed[0]).unwrap().2, "document.pdf");
    }
}
