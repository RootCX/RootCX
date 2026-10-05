//! The host supplies an optional companion; App releases never own its version.

pub fn configured(html: Vec<u8>, app: &str) -> Vec<u8> {
    let Ok(script) = std::env::var("ROOTCX_FRONTEND_COMPANION_URL") else { return html };
    inject(html, app, &script)
}

fn inject(html: Vec<u8>, app: &str, script: &str) -> Vec<u8> {
    let Ok(url) = url::Url::parse(script) else { return html };
    if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none()
        || !url.username().is_empty() || url.password().is_some() {
        return html;
    }
    let Ok(mut document) = String::from_utf8(html.clone()) else { return html };
    strip_legacy(&mut document, app);
    if document.contains("data-rootcx-companion") { return document.into_bytes(); }
    let source = url.as_str().replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
    let tag = format!("<script type=\"module\" data-rootcx-companion src=\"{source}\"></script>");
    let at = document.rfind("</body>").unwrap_or(document.len());
    document.insert_str(at, &tag);
    document.into_bytes()
}

pub fn without_legacy(html: Vec<u8>, app: &str) -> Vec<u8> {
    let Ok(mut document) = String::from_utf8(html.clone()) else { return html };
    strip_legacy(&mut document, app);
    document.into_bytes()
}

fn strip_legacy(document: &mut String, app: &str) {
    // Compatibility with the old SHAPP packager. Only its exact empty module
    // tags are removed in the response; published files and Git stay untouched.
    for prefix in [format!("/apps/{app}/shappy/companion."), "/shappy/companion.".into()] {
        let start = format!("<script type=\"module\" src=\"{prefix}");
        while let Some(offset) = document.find(&start) {
            let remainder = &document[offset + start.len()..];
            let Some(end) = remainder.find(".js\"></script>") else { break };
            if !remainder[..end].bytes().all(|byte| byte.is_ascii_hexdigit()) { break; }
            document.replace_range(offset..offset + start.len() + end + ".js\"></script>".len(), "");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_updates_replace_legacy_without_changing_application_assets() {
        let html = br#"<html><body><script type="module" src="/apps/demo/assets/app.a12.js"></script><script type="module" src="/apps/demo/shappy/companion.abc123.js"></script></body></html>"#.to_vec();
        let rendered = String::from_utf8(inject(html.clone(), "demo", "https://shapp.example/platform/shappy/loader.js")).unwrap();
        assert!(!rendered.contains("companion.abc123"));
        assert!(rendered.contains("assets/app.a12.js"));
        assert_eq!(rendered.matches("data-rootcx-companion").count(), 1);
        assert_eq!(inject(rendered.as_bytes().to_vec(), "demo", "https://shapp.example/platform/shappy/loader.js"), rendered.as_bytes());
        for invalid in ["javascript:alert(1)", "//untrusted.example/loader.js", "https://user:secret@example.com/loader.js"] {
            assert_eq!(inject(html.clone(), "demo", invalid), html, "{invalid}");
        }
        let shared = String::from_utf8(without_legacy(html.clone(), "demo")).unwrap();
        assert!(!shared.contains("shappy/"));
        assert!(!shared.contains("data-rootcx-companion"));
        assert!(shared.contains("assets/app.a12.js"));
        let new_app = inject(b"<body>Clients</body>".to_vec(), "new_app", "http://127.0.0.1:4178/platform/shappy/loader.js");
        assert!(String::from_utf8(new_app).unwrap().contains("data-rootcx-companion"));
    }
}
