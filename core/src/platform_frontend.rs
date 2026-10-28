//! The host supplies an optional companion; App releases never own its version.

pub fn configured(html: Vec<u8>, app: &str) -> Vec<u8> {
    let Ok(script) = std::env::var("ROOTCX_FRONTEND_COMPANION_URL") else { return html };
    let company = std::env::var("ROOTCX_TENANT_REF").ok();
    inject(html, app, &script, company.as_deref(), std::env::var("ROOTCX_FRONTEND_LEGACY_COMPANION_PREFIXES").ok().as_deref())
}

fn inject(html: Vec<u8>, app: &str, script: &str, company: Option<&str>, legacy: Option<&str>) -> Vec<u8> {
    let Ok(url) = url::Url::parse(script) else { return html };
    if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none()
        || !url.username().is_empty() || url.password().is_some() {
        return html;
    }
    let Ok(mut document) = String::from_utf8(html.clone()) else { return html };
    strip_legacy(&mut document, &legacy_prefixes(app, Some(script), legacy));
    if document.contains("data-rootcx-companion") { return document.into_bytes(); }
    let source = url.as_str().replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
    let company_attribute = company
        .filter(|value| {
            (3..=64).contains(&value.len())
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
        .map(|value| format!(" data-rootcx-company=\"{value}\""))
        .unwrap_or_default();
    let tag = format!("<script type=\"module\" data-rootcx-companion{company_attribute} src=\"{source}\"></script>");
    let at = document.rfind("</body>").unwrap_or(document.len());
    document.insert_str(at, &tag);
    document.into_bytes()
}

pub fn without_legacy(html: Vec<u8>, app: &str) -> Vec<u8> {
    let Ok(mut document) = String::from_utf8(html.clone()) else { return html };
    let script = std::env::var("ROOTCX_FRONTEND_COMPANION_URL").ok();
    let legacy = std::env::var("ROOTCX_FRONTEND_LEGACY_COMPANION_PREFIXES").ok();
    strip_legacy(&mut document, &legacy_prefixes(app, script.as_deref(), legacy.as_deref()));
    document.into_bytes()
}

fn legacy_prefixes(app: &str, script: Option<&str>, configured: Option<&str>) -> Vec<String> {
    if let Some(configured) = configured {
        let Ok(prefixes) = serde_json::from_str::<Vec<String>>(configured) else { return vec![] };
        if prefixes.len() > 16 { return vec![]; }
        return prefixes.into_iter().map(|prefix| prefix.replace("{app}", app))
            .filter(|prefix| prefix.len() <= 256 && prefix.starts_with('/') && !prefix.starts_with("//")
                && prefix.ends_with('.') && !prefix.contains("..")
                && prefix.bytes().all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b)))
            .collect();
    }
    // Older hosts only configured a loader. Infer its namespace for their legacy bundles.
    let Some(url) = script.and_then(|script| url::Url::parse(script).ok()) else { return vec![] };
    if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none()
        || !url.username().is_empty() || url.password().is_some() { return vec![]; }
    let parts: Vec<_> = url.path().split('/').collect();
    let Some(namespace) = parts.iter().rev().nth(1).filter(|part| !part.is_empty()
        && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))) else { return vec![] };
    vec![format!("/apps/{app}/{namespace}/companion."), format!("/{namespace}/companion.")]
}

fn strip_legacy(document: &mut String, prefixes: &[String]) {
    // Only exact empty module tags are removed from responses; stored releases stay intact.
    for prefix in prefixes {
        let start = format!("<script type=\"module\" src=\"{prefix}");
        let suffix = ".js\"></script>";
        let mut cursor = 0;
        while let Some(found) = document[cursor..].find(&start) {
            let offset = cursor + found;
            let hash_at = offset + start.len();
            let hash_len = document[hash_at..].bytes().take_while(u8::is_ascii_hexdigit).count();
            if hash_len > 0 && document[hash_at + hash_len..].starts_with(suffix) {
                document.replace_range(offset..hash_at + hash_len + suffix.len(), "");
                cursor = offset;
            } else {
                cursor = hash_at;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_company_context_is_bounded_and_never_interpolates_html() {
        let html = b"<body>App</body>".to_vec();
        let script = "https://shapp.example/platform/shappy/loader.js";
        let valid = String::from_utf8(inject(html.clone(), "demo", script, Some("company_123"), None)).unwrap();
        assert!(valid.contains("data-rootcx-company=\"company_123\""));
        for company in ["", "x", "bad\" onload=\"attack", "<script>", &"x".repeat(65)] {
            let output = String::from_utf8(inject(html.clone(), "demo", script, Some(company), None)).unwrap();
            assert!(!output.contains("data-rootcx-company"));
        }
    }

    #[test]
    fn platform_updates_replace_legacy_without_changing_application_assets() {
        let html = br#"<html><body><script type="module" src="/apps/demo/assets/app.a12.js"></script><script type="module" src="/apps/demo/shappy/companion.abc123.js"></script></body></html>"#.to_vec();
        let rendered = String::from_utf8(inject(html.clone(), "demo", "https://shapp.example/platform/shappy/loader.js", None, None)).unwrap();
        assert!(!rendered.contains("companion.abc123"));
        assert!(rendered.contains("assets/app.a12.js"));
        assert_eq!(rendered.matches("data-rootcx-companion").count(), 1);
        assert_eq!(inject(rendered.as_bytes().to_vec(), "demo", "https://shapp.example/platform/shappy/loader.js", None, None), rendered.as_bytes());
        for invalid in ["javascript:alert(1)", "//untrusted.example/loader.js", "https://user:secret@example.com/loader.js"] {
            assert_eq!(inject(html.clone(), "demo", invalid, None, None), html, "{invalid}");
        }
        let mut shared = String::from_utf8(html.clone()).unwrap();
        strip_legacy(&mut shared, &legacy_prefixes("demo", Some("https://shapp.example/platform/shappy/loader.js"), None));
        assert!(!shared.contains("shappy/"));
        assert!(!shared.contains("data-rootcx-companion"));
        assert!(shared.contains("assets/app.a12.js"));
        let new_app = inject(b"<body>Clients</body>".to_vec(), "new_app", "http://127.0.0.1:4178/platform/shappy/loader.js", None, None);
        assert!(String::from_utf8(new_app).unwrap().contains("data-rootcx-companion"));
    }
    #[test]
    fn explicit_aliases_remove_only_matching_empty_module_tags() {
        let prefixes = legacy_prefixes("demo", None, Some(r#"["/apps/{app}/old/companion."]"#));
        let keep = r#"<script type="module" src="/apps/demo/old/companion.nothex.js"></script><script type="module" src="/apps/other/old/companion.abc.js"></script><script type="module" src="/apps/demo/old/companion.abc.js">code()</script><script type="module" src="/apps/demo/old/companion..js"></script>"#;
        let remove = r#"<script type="module" src="/apps/demo/old/companion.abc123.js"></script>"#;
        let mut document = format!("{keep}{remove}{remove}");
        strip_legacy(&mut document, &prefixes);
        assert_eq!(document, keep);
        for invalid in ["invalid JSON", r#"["//external/companion.","/../companion.","/x?companion.","/x\"companion."]"#, "[]"] {
            assert!(legacy_prefixes("demo", Some("https://host/platform/old/loader.js"), Some(invalid)).is_empty(), "{invalid}");
        }
        assert!(legacy_prefixes("demo", None, None).is_empty());
    }


}
