//! Files an MCP host attaches by reference (a `file` object with a
//! `download_url`, as hosts declaring `openai/fileParams` send them). Only hosts
//! the operator lists in `ROOTCX_MCP_FILE_HOSTS` are fetched, so the tool cannot
//! be aimed at internal addresses; every redirect hop is checked again.

use schemars::JsonSchema;
use serde::Deserialize;

const MAX_REDIRECTS: usize = 3;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AttachedFile {
    pub download_url: String,
    pub file_id: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
}

pub struct Fetched {
    pub name: String,
    pub content_type: String,
    pub content: Vec<u8>,
}

fn allowed(url: &url::Url) -> bool {
    let hosts = std::env::var("ROOTCX_MCP_FILE_HOSTS").unwrap_or_default();
    let Some(host) = url.host_str() else { return false };
    url.scheme() == "https" && url.username().is_empty() && url.password().is_none() && url.port().is_none()
        && hosts.split(',').map(str::trim).filter(|h| !h.is_empty())
            .any(|allowed| host == allowed || host.ends_with(&format!(".{allowed}")))
}

fn sniff(content: &[u8]) -> Option<(&'static str, &'static str)> {
    if content.starts_with(b"%PDF") { return Some(("application/pdf", "pdf")) }
    if content.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) { return Some(("image/png", "png")) }
    if content.starts_with(&[0xff, 0xd8, 0xff]) { return Some(("image/jpeg", "jpg")) }
    None
}

pub async fn fetch(file: &AttachedFile, max_bytes: usize) -> Result<Fetched, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|_| "file download unavailable".to_string())?;
    let mut url = url::Url::parse(&file.download_url).map_err(|_| "the file link is not valid".to_string())?;
    for _ in 0..=MAX_REDIRECTS {
        if !allowed(&url) {
            return Err("this file link cannot be read; ask the person to add the file in the app".into());
        }
        let mut response = client.get(url.clone()).send().await.map_err(|_| "the file could not be downloaded".to_string())?;
        if response.status().is_redirection() {
            let location = response.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok())
                .ok_or("the file could not be downloaded")?;
            url = url.join(location).map_err(|_| "the file link is not valid".to_string())?;
            continue;
        }
        if !response.status().is_success() {
            return Err("the file could not be downloaded; ask the person to attach it again".into());
        }
        let mut content = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "the file could not be downloaded".to_string())? {
            content.extend_from_slice(&chunk);
            if content.len() > max_bytes {
                return Err(format!("the file is larger than {max_bytes} bytes; ask the person to add it in the app"));
            }
        }
        let sniffed = sniff(&content);
        let content_type = file.mime_type.clone().or_else(|| sniffed.map(|(t, _)| t.to_string()))
            .unwrap_or_else(|| "application/octet-stream".into());
        let name = file.file_name.clone().filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| format!("{}.{}", file.file_id, sniffed.map_or("bin", |(_, ext)| ext)));
        return Ok(Fetched { name, content_type, content });
    }
    Err("the file link redirects too many times".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_listed_https_hosts_are_fetched() {
        // SAFETY: single-threaded test-only env mutation.
        unsafe { std::env::set_var("ROOTCX_MCP_FILE_HOSTS", "files.example, cdn.example") };
        for (url, expected) in [
            ("https://files.example/a.pdf", true),
            ("https://eu.files.example/a.pdf", true),
            ("https://cdn.example/a.pdf", true),
            ("http://files.example/a.pdf", false),
            ("https://files.example:8443/a.pdf", false),
            ("https://user@files.example/a.pdf", false),
            ("https://evilfiles.example/a.pdf", false),
            ("https://files.example.evil/a.pdf", false),
            ("https://169.254.169.254/latest", false),
        ] {
            assert_eq!(allowed(&url::Url::parse(url).unwrap()), expected, "{url}");
        }
    }

    #[test]
    fn content_type_comes_from_the_bytes_when_the_host_omits_it() {
        assert_eq!(sniff(b"%PDF-1.7"), Some(("application/pdf", "pdf")));
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some(("image/jpeg", "jpg")));
        assert_eq!(sniff(b"<html>"), None);
    }
}
