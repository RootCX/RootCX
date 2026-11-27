//! An anonymous reader has authority only over the bytes of the currently published file.
use std::io;
use std::ops::Range;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use futures::stream;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::routes::SharedRuntime;

pub(super) fn routes() -> Router<SharedRuntime> {
    Router::new().route(
        "/api/v1/public/apps/{app_id}/files/{channel}/{key}",
        get(download),
    )
}

pub(super) fn detect_media_type(prefix: &[u8]) -> Option<&'static str> {
    if prefix.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if prefix.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if prefix.starts_with(b"GIF87a") || prefix.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if prefix.starts_with(b"RIFF")
        && prefix.get(8..12) == Some(b"WEBP")
        && matches!(prefix.get(12..16), Some(b"VP8 " | b"VP8L" | b"VP8X"))
    {
        return Some("image/webp");
    }
    if prefix.starts_with(b"%PDF-")
        && matches!(prefix.get(5..8), Some([b'1' | b'2', b'.', b'0'..=b'9']))
    {
        return Some("application/pdf");
    }
    if prefix.starts_with(b"\x1a\x45\xdf\xa3")
        && prefix.windows(7).any(|field| field == b"\x42\x82\x84webm")
    {
        return Some("video/webm");
    }
    if prefix.get(4..8) != Some(b"ftyp") {
        return None;
    }
    let size = u32::from_be_bytes(prefix.get(..4)?.try_into().ok()?) as usize;
    let brands = prefix.get(16..size)?;
    if brands.len() % 4 != 0 {
        return None;
    }
    let major = prefix.get(8..12)?;
    if matches!(major, b"avif" | b"avis")
        || brands
            .chunks_exact(4)
            .any(|b| matches!(b, b"avif" | b"avis"))
    {
        return Some("image/avif");
    }
    match major {
        b"qt  " => Some("video/quicktime"),
        b"mp41" | b"mp42" | b"isom" | b"iso2" | b"iso3" | b"iso4" | b"iso5" | b"iso6" | b"avc1"
        | b"M4V " | b"dash" => Some("video/mp4"),
        _ => None,
    }
}

fn decimal(value: &str) -> Result<u64, ()> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(());
    }
    // Numbers larger than u64 are still valid HTTP decimals. Clamp before offset arithmetic.
    Ok(value.parse().unwrap_or(u64::MAX))
}

fn requested_range(value: &str, size: u64) -> Result<Option<Range<u64>>, ()> {
    let (unit, spec) = value.split_once('=').ok_or(())?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return Ok(None);
    }
    let (start, last) = spec.trim().split_once('-').ok_or(())?;
    if size == 0 {
        return Err(());
    }
    if start.is_empty() {
        let length = decimal(last)?;
        return if length == 0 {
            Err(())
        } else {
            Ok(Some(size.saturating_sub(length)..size))
        };
    }
    let start = decimal(start)?;
    let end = if last.is_empty() {
        size
    } else {
        decimal(last)?.saturating_add(1).min(size)
    };
    if start >= end {
        return Err(());
    }
    Ok(Some(start..end))
}

const AUTHORIZED_FILE: &str = "
 FROM rootcx_system.file_publications p
 JOIN rootcx_system.app_installations i ON i.id=p.installation_id AND i.active
 JOIN rootcx_system.files f ON f.id=p.file_id AND f.app_id=i.app_id
 WHERE i.app_id=$1 AND p.channel=$2 AND p.key=$3";

struct PublicFile {
    pool: PgPool,
    app: String,
    channel: String,
    key: String,
    file_id: Uuid,
    installation_id: Uuid,
}

impl PublicFile {
    fn stream(self, range: Range<u64>) -> Body {
        // Recheck live authority and identity per chunk. Slow readers never occupy a pool
        // connection between chunks, and replacement cannot splice bytes from two files.
        let query = format!(
            "SELECT CASE WHEN f.content IS NOT NULL THEN substring(f.content FROM ($6::bigint + 1)::integer FOR $7)
             ELSE lo_get(f.content_oid,$6::bigint,$7) END
             {AUTHORIZED_FILE} AND f.id=$4 AND i.id=$5"
        );
        Body::from_stream(stream::try_unfold(
            (range.start, self, query),
            move |(start, file, query)| async move {
                if start == range.end {
                    return Ok(None);
                }
                let length = (range.end - start).min(256 * 1024) as i32;
                let bytes: Vec<u8> = sqlx::query_scalar(&query)
                    .bind(&file.app)
                    .bind(&file.channel)
                    .bind(&file.key)
                    .bind(file.file_id)
                    .bind(file.installation_id)
                    .bind(start as i64)
                    .bind(length)
                    .fetch_one(&file.pool)
                    .await
                    .map_err(|_| io::Error::other("published file unavailable"))?;
                if bytes.len() != length as usize {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "published file truncated",
                    ));
                }
                Ok(Some((
                    Bytes::from(bytes),
                    (start + length as u64, file, query),
                )))
            },
        ))
    }
}

async fn download(
    State(rt): State<SharedRuntime>,
    Path((app, channel, key)): Path<(String, String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let query = format!("SELECT f.id, i.id, f.size, p.media_type {AUTHORIZED_FILE}");
    let meta: Option<(Uuid, Uuid, i64, String)> = sqlx::query_as(&query)
        .bind(&app)
        .bind(&channel)
        .bind(&key)
        .fetch_optional(rt.pool())
        .await?;
    let mut response = Response::builder()
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        .header("content-security-policy", "sandbox; default-src 'none'");
    let Some((file_id, installation_id, size, mime)) = meta else {
        return Ok(response
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap());
    };
    let size =
        u64::try_from(size).map_err(|_| ApiError::Internal("invalid stored file size".into()))?;
    let etag = format!("\"{:x}\"", Sha256::digest(file_id.as_bytes()));
    let disposition = match mime.as_str() {
        "application/pdf" => "attachment; filename=\"document.pdf\"",
        "image/jpeg" | "image/png" | "image/gif" | "image/webp" | "image/avif" | "video/mp4"
        | "video/webm" | "video/quicktime" => "inline",
        _ => return Err(ApiError::Internal("invalid published media type".into())),
    };
    response = response
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ETAG, &etag);
    let range = if method == Method::GET
        && headers
            .get(header::IF_RANGE)
            .is_none_or(|value| value.as_bytes() == etag.as_bytes())
    {
        match headers.get(header::RANGE) {
            Some(value) => match value
                .to_str()
                .ok()
                .and_then(|s| requested_range(s, size).ok())
            {
                Some(range) if headers.get_all(header::RANGE).iter().count() == 1 => range,
                _ => {
                    return Ok(response
                        .status(StatusCode::RANGE_NOT_SATISFIABLE)
                        .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                        .body(Body::empty())
                        .unwrap());
                }
            },
            None => None,
        }
    } else {
        None
    };
    if let Some(range) = &range {
        response = response.status(StatusCode::PARTIAL_CONTENT).header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{size}", range.start, range.end - 1),
        );
    }
    let range = range.unwrap_or(0..size);
    response = response
        .header(header::CONTENT_LENGTH, range.end - range.start)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_DISPOSITION, disposition);
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        PublicFile {
            pool: rt.pool().clone(),
            app,
            channel,
            key,
            file_id,
            installation_id,
        }
        .stream(range)
    };
    Ok(response.body(body).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_ranges_are_bounded_without_overflow() {
        for (header, expected) in [
            ("bytes=0-0", 0..1),
            ("bytes=3-", 3..10),
            ("bytes=-3", 7..10),
            ("bytes=6-200", 6..10),
            ("bytes=-200", 0..10),
            ("bytes=9-1844674407370955161600", 9..10),
        ] {
            assert_eq!(requested_range(header, 10), Ok(Some(expected)), "{header}");
        }
        for header in [
            "bytes=10-",
            "bytes=8-3",
            "bytes=-",
            "bytes=-0",
            "bytes=3-4,7-8",
            "bytes=+1-2",
        ] {
            assert!(requested_range(header, 10).is_err(), "{header}");
        }
        assert!(requested_range("bytes=0-", 0).is_err());
        assert_eq!(requested_range("items=1-3", 10), Ok(None));
    }

    #[test]
    fn signatures_identify_allowed_media_without_filenames_or_mime_hints() {
        let examples: &[(&[u8], &str)] = &[
            (b"\x89PNG\r\n\x1a\n", "image/png"),
            (b"\xff\xd8\xff", "image/jpeg"),
            (b"GIF89a", "image/gif"),
            (b"RIFF1234WEBPVP8L", "image/webp"),
            (b"\0\0\0\x14ftypmif1\0\0\0\0avif", "image/avif"),
            (b"\0\0\0\x14ftypmp42\0\0\0\0isom", "video/mp4"),
            (b"\0\0\0\x10ftypqt  \0\0\0\0", "video/quicktime"),
            (b"\x1a\x45\xdf\xa3\x87\x42\x82\x84webm", "video/webm"),
            (b"%PDF-1.7", "application/pdf"),
        ];
        for (bytes, mime) in examples {
            assert_eq!(detect_media_type(bytes), Some(*mime));
        }
        for bytes in [
            b"<html>".as_slice(),
            b"<svg/>",
            b"plain text",
            b"RIFF1234WAVE",
            b"\0\0\0\x18ftypmp42",
        ] {
            assert_eq!(detect_media_type(bytes), None);
        }
    }
}
