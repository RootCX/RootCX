use std::{
    collections::BTreeMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::{
    Method, StatusCode, Url,
    header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, LOCATION},
};
use rootcx_types::ToolDescriptor;
use serde_json::{Value as JsonValue, json};

use super::{Tool, ToolContext};

const DEFAULT_TIMEOUT_MS: u64 = 10_000;
const MAX_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const HARD_MAX_RESPONSE_BYTES: usize = 5 * 1024 * 1024;
const DEFAULT_MAX_REDIRECTS: usize = 3;
const HARD_MAX_REDIRECTS: usize = 5;
const MANAGED_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "connection",
    "transfer-encoding",
    "upgrade",
];

pub struct HttpRequestTool;

#[async_trait]
impl Tool for HttpRequestTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "http_request".into(),
            description: concat!(
                "Call an external HTTP endpoint from a workflow. Supports JSON/text responses, ",
                "headers, query parameters, JSON or text request bodies, timeouts, response size ",
                "limits, and SSRF protection that blocks localhost/private/link-local networks."
            )
            .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "method": {
                        "type": "string",
                        "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"],
                        "default": "GET"
                    },
                    "url": { "type": "string" },
                    "headers": {
                        "type": "object",
                        "additionalProperties": true
                    },
                    "query": {
                        "type": "object",
                        "additionalProperties": true
                    },
                    "body": {},
                    "responseFormat": {
                        "type": "string",
                        "enum": ["json", "text"],
                        "default": "json"
                    },
                    "timeoutMs": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_TIMEOUT_MS
                    },
                    "maxResponseBytes": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": HARD_MAX_RESPONSE_BYTES
                    },
                    "followRedirects": {
                        "type": "boolean",
                        "default": true
                    },
                    "maxRedirects": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": HARD_MAX_REDIRECTS
                    },
                    "failOnHttpError": {
                        "type": "boolean",
                        "default": true
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        let args = &ctx.args;
        let method = parse_method(args)?;
        let mut url = parse_url(args)?;
        apply_query(&mut url, args.get("query"))?;

        let headers = parse_headers(args.get("headers"))?;
        let body = parse_body(args.get("body"), &headers)?;
        validate_method_body(&method, &body)?;

        let response_format = parse_response_format(args)?;
        let timeout_ms = bounded_u64(args, "timeoutMs", DEFAULT_TIMEOUT_MS, MAX_TIMEOUT_MS)?;
        let max_response_bytes = bounded_usize(
            args,
            "maxResponseBytes",
            DEFAULT_MAX_RESPONSE_BYTES,
            HARD_MAX_RESPONSE_BYTES,
        )?;
        let follow_redirects = bool_arg(args, "followRedirects", true)?;
        let max_redirects = bounded_usize_allow_zero(
            args,
            "maxRedirects",
            DEFAULT_MAX_REDIRECTS,
            HARD_MAX_REDIRECTS,
        )?;
        let fail_on_http_error = bool_arg(args, "failOnHttpError", true)?;

        let response = send_with_redirects(
            method,
            url,
            headers,
            body,
            timeout_ms,
            follow_redirects,
            max_redirects,
        )
        .await?;

        let final_url = response.url().to_string();
        let status = response.status();
        let response_headers = headers_to_json(response.headers());
        ensure_content_length_allowed(response.content_length(), max_response_bytes)?;
        let bytes = read_limited_body(response, max_response_bytes).await?;

        if fail_on_http_error && !status.is_success() {
            return Err(format!(
                "HTTP request failed with status {}: {}",
                status.as_u16(),
                body_snippet(&bytes)
            ));
        }

        let body = parse_response_body(&bytes, response_format)?;
        Ok(json!({
            "status": status.as_u16(),
            "headers": response_headers,
            "body": body,
            "url": final_url
        }))
    }
}

#[derive(Clone, Debug, PartialEq)]
enum RequestBody {
    Json(JsonValue),
    Text(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResponseFormat {
    Json,
    Text,
}

async fn send_with_redirects(
    original_method: Method,
    original_url: Url,
    headers: HeaderMap,
    original_body: Option<RequestBody>,
    timeout_ms: u64,
    follow_redirects: bool,
    max_redirects: usize,
) -> Result<reqwest::Response, String> {
    let mut method = original_method;
    let mut url = original_url;
    let mut body = original_body;

    for redirect_count in 0..=max_redirects {
        let resolved_addrs = resolve_allowed_url(&url).await?;
        let client = build_client(timeout_ms, &url, &resolved_addrs)?;

        let mut request = client
            .request(method.clone(), url.clone())
            .headers(headers.clone());
        if let Some(request_body) = &body {
            request = match request_body {
                RequestBody::Json(value) => request.json(value),
                RequestBody::Text(value) => request.body(value.clone()),
            };
        }

        let response = request
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        if !follow_redirects || !response.status().is_redirection() {
            return Ok(response);
        }

        if redirect_count == max_redirects {
            return Err(format!("too many redirects: maximum is {max_redirects}"));
        }

        let location = response
            .headers()
            .get(LOCATION)
            .ok_or_else(|| {
                format!(
                    "redirect {} missing Location header",
                    response.status().as_u16()
                )
            })?
            .to_str()
            .map_err(|_| "redirect Location header is not valid UTF-8".to_string())?;
        url = url
            .join(location)
            .map_err(|e| format!("invalid redirect Location: {e}"))?;

        if should_switch_redirect_to_get(response.status(), &method) {
            method = Method::GET;
            body = None;
        }
    }

    Err("redirect loop terminated unexpectedly".into())
}

fn build_client(
    timeout_ms: u64,
    url: &Url,
    resolved_addrs: &[SocketAddr],
) -> Result<reqwest::Client, String> {
    let host = url
        .host_str()
        .ok_or_else(|| "URL host is required".to_string())?;
    reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, resolved_addrs)
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))
}

fn should_switch_redirect_to_get(status: StatusCode, method: &Method) -> bool {
    status == StatusCode::SEE_OTHER
        || ((status == StatusCode::MOVED_PERMANENTLY || status == StatusCode::FOUND)
            && *method == Method::POST)
}

fn parse_method(args: &JsonValue) -> Result<Method, String> {
    let raw = args.get("method").and_then(|v| v.as_str()).unwrap_or("GET");
    match raw.to_ascii_uppercase().as_str() {
        "GET" => Ok(Method::GET),
        "POST" => Ok(Method::POST),
        "PUT" => Ok(Method::PUT),
        "PATCH" => Ok(Method::PATCH),
        "DELETE" => Ok(Method::DELETE),
        "HEAD" => Ok(Method::HEAD),
        _ => Err(format!("unsupported HTTP method: {raw}")),
    }
}

fn parse_url(args: &JsonValue) -> Result<Url, String> {
    let raw = args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing: url".to_string())?;
    Url::parse(raw).map_err(|e| format!("invalid URL: {e}"))
}

fn parse_response_format(args: &JsonValue) -> Result<ResponseFormat, String> {
    let raw = args
        .get("responseFormat")
        .and_then(|v| v.as_str())
        .unwrap_or("json");
    match raw {
        "json" => Ok(ResponseFormat::Json),
        "text" => Ok(ResponseFormat::Text),
        _ => Err(format!("unsupported responseFormat: {raw}")),
    }
}

fn parse_headers(value: Option<&JsonValue>) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    let Some(value) = value else {
        return Ok(headers);
    };
    let object = value
        .as_object()
        .ok_or_else(|| "headers must be an object".to_string())?;

    for (name, value) in object {
        let lowered = name.to_ascii_lowercase();
        if MANAGED_HEADERS.contains(&lowered.as_str()) {
            return Err(format!(
                "header is managed by the HTTP client and cannot be set: {name}"
            ));
        }
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| format!("invalid header name {name}: {e}"))?;
        let header_value = HeaderValue::from_str(&scalar_to_string(value)?)
            .map_err(|e| format!("invalid header value for {name}: {e}"))?;
        headers.insert(header_name, header_value);
    }

    Ok(headers)
}

fn parse_body(
    value: Option<&JsonValue>,
    headers: &HeaderMap,
) -> Result<Option<RequestBody>, String> {
    let Some(value) = value else {
        return Ok(None);
    };

    if value.is_null() {
        return Ok(None);
    }

    if let Some(value) = value.as_str() {
        return Ok(Some(RequestBody::Text(value.to_string())));
    }

    if headers.contains_key(CONTENT_TYPE) {
        Ok(Some(RequestBody::Text(scalar_or_json_string(value)?)))
    } else {
        Ok(Some(RequestBody::Json(value.clone())))
    }
}

fn validate_method_body(method: &Method, body: &Option<RequestBody>) -> Result<(), String> {
    if body.is_some() && (*method == Method::GET || *method == Method::HEAD) {
        return Err(format!(
            "{} requests cannot include a body",
            method.as_str()
        ));
    }
    Ok(())
}

fn apply_query(url: &mut Url, value: Option<&JsonValue>) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    let object = value
        .as_object()
        .ok_or_else(|| "query must be an object".to_string())?;
    if object.is_empty() {
        return Ok(());
    }

    let mut pairs = url.query_pairs_mut();
    for (key, value) in object {
        match value {
            JsonValue::Array(values) => {
                for item in values {
                    pairs.append_pair(key, &scalar_to_string(item)?);
                }
            }
            _ => {
                pairs.append_pair(key, &scalar_to_string(value)?);
            }
        }
    }
    Ok(())
}

async fn resolve_allowed_url(url: &Url) -> Result<Vec<SocketAddr>, String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("only http and https URLs are allowed".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URL credentials are not allowed".into());
    }

    let host = url
        .host_str()
        .ok_or_else(|| "URL host is required".to_string())?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err("localhost URLs are not allowed".into());
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        reject_blocked_ip(ip)?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| "URL port is required".to_string())?;
        return Ok(vec![SocketAddr::new(ip, port)]);
    }

    let port = url
        .port_or_known_default()
        .ok_or_else(|| "URL port is required".to_string())?;
    let resolved = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("failed to resolve URL host: {e}"))?;
    let mut allowed = Vec::new();
    for socket_addr in resolved {
        reject_blocked_ip(socket_addr.ip())?;
        allowed.push(socket_addr);
    }

    if allowed.is_empty() {
        return Err("URL host did not resolve to any address".into());
    }

    Ok(allowed)
}

fn reject_blocked_ip(ip: IpAddr) -> Result<(), String> {
    if is_blocked_ip(ip) {
        Err(format!("URL resolves to a blocked network address: {ip}"))
    } else {
        Ok(())
    }
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_blocked_ipv4(ip),
        IpAddr::V6(ip) => is_blocked_ipv6(ip),
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        || octets[0] == 0
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
}

fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    let first = segments[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
}

fn bounded_u64(args: &JsonValue, key: &str, default: u64, max: u64) -> Result<u64, String> {
    match args.get(key) {
        None => Ok(default),
        Some(value) => {
            let n = value
                .as_u64()
                .ok_or_else(|| format!("{key} must be a positive integer"))?;
            if n == 0 || n > max {
                return Err(format!("{key} must be between 1 and {max}"));
            }
            Ok(n)
        }
    }
}

fn bounded_usize(args: &JsonValue, key: &str, default: usize, max: usize) -> Result<usize, String> {
    let n = bounded_usize_allow_zero(args, key, default, max)?;
    if n == 0 {
        return Err(format!("{key} must be between 1 and {max}"));
    }
    Ok(n)
}

fn bounded_usize_allow_zero(
    args: &JsonValue,
    key: &str,
    default: usize,
    max: usize,
) -> Result<usize, String> {
    match args.get(key) {
        None => Ok(default),
        Some(value) => {
            let n = value
                .as_u64()
                .ok_or_else(|| format!("{key} must be a positive integer"))?;
            if n as usize > max {
                return Err(format!("{key} must be between 0 and {max}"));
            }
            Ok(n as usize)
        }
    }
}

fn bool_arg(args: &JsonValue, key: &str, default: bool) -> Result<bool, String> {
    match args.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| format!("{key} must be a boolean")),
    }
}

fn scalar_to_string(value: &JsonValue) -> Result<String, String> {
    match value {
        JsonValue::Null => Ok(String::new()),
        JsonValue::Bool(value) => Ok(value.to_string()),
        JsonValue::Number(value) => Ok(value.to_string()),
        JsonValue::String(value) => Ok(value.clone()),
        JsonValue::Array(_) | JsonValue::Object(_) => {
            serde_json::to_string(value).map_err(|e| format!("failed to encode value: {e}"))
        }
    }
}

fn scalar_or_json_string(value: &JsonValue) -> Result<String, String> {
    match value {
        JsonValue::String(value) => Ok(value.clone()),
        _ => serde_json::to_string(value).map_err(|e| format!("failed to encode body: {e}")),
    }
}

fn ensure_content_length_allowed(
    content_length: Option<u64>,
    max_response_bytes: usize,
) -> Result<(), String> {
    if let Some(content_length) = content_length {
        if content_length > max_response_bytes as u64 {
            return Err(format!(
                "response is too large: content-length is {content_length} bytes, limit is {max_response_bytes}"
            ));
        }
    }
    Ok(())
}

async fn read_limited_body(
    response: reqwest::Response,
    max_response_bytes: usize,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("failed to read response body: {e}"))?;
        if bytes.len() + chunk.len() > max_response_bytes {
            return Err(format!("response body exceeded {max_response_bytes} bytes"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn parse_response_body(bytes: &[u8], response_format: ResponseFormat) -> Result<JsonValue, String> {
    match response_format {
        ResponseFormat::Json => {
            serde_json::from_slice(bytes).map_err(|e| format!("response is not valid JSON: {e}"))
        }
        ResponseFormat::Text => Ok(JsonValue::String(
            String::from_utf8_lossy(bytes).into_owned(),
        )),
    }
}

fn headers_to_json(headers: &HeaderMap) -> JsonValue {
    let mut out = BTreeMap::new();
    for (name, value) in headers {
        if let Ok(value) = value.to_str() {
            out.insert(
                name.as_str().to_string(),
                JsonValue::String(value.to_string()),
            );
        }
    }
    json!(out)
}

fn body_snippet(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut snippet: String = text.chars().take(500).collect();
    if text.chars().count() > 500 {
        snippet.push_str("...");
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_private_and_local_networks() {
        let blocked = [
            "0.0.0.0",
            "10.0.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.168.1.1",
            "224.0.0.1",
            "255.255.255.255",
            "100.64.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
        ];

        for input in blocked {
            let ip = input.parse().expect("test IP must parse");
            assert!(is_blocked_ip(ip), "{input} should be blocked");
        }
    }

    #[test]
    fn allows_public_networks() {
        let allowed = ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"];

        for input in allowed {
            let ip = input.parse().expect("test IP must parse");
            assert!(!is_blocked_ip(ip), "{input} should be allowed");
        }
    }

    #[tokio::test]
    async fn rejects_unsafe_url_forms_before_dns_lookup() {
        for input in [
            "ftp://example.com/file",
            "https://user:pass@example.com",
            "http://localhost:8080",
            "http://api.localhost:8080",
        ] {
            let url = Url::parse(input).expect("test URL must parse");
            let result = resolve_allowed_url(&url).await;
            assert!(result.is_err(), "{input} should be rejected");
        }
    }

    #[tokio::test]
    async fn resolves_public_ip_to_validated_socket_addr() {
        let url = Url::parse("https://1.1.1.1/status").expect("test URL must parse");
        let addrs = resolve_allowed_url(&url)
            .await
            .expect("public IP is allowed");
        assert_eq!(
            addrs,
            vec![SocketAddr::new("1.1.1.1".parse().unwrap(), 443)]
        );
    }

    #[test]
    fn rejects_unsupported_methods() {
        for method in ["CONNECT", "TRACE", "OPTIONS"] {
            let result = parse_method(&json!({ "method": method }));
            assert!(result.is_err(), "{method} should be rejected");
        }
    }

    #[test]
    fn rejects_invalid_response_format() {
        let result = parse_response_format(&json!({ "responseFormat": "bytes" }));
        assert!(result.is_err());
    }

    #[test]
    fn parses_body_as_json_unless_content_type_is_user_supplied() {
        let json_body =
            parse_body(Some(&json!({ "ok": true })), &HeaderMap::new()).expect("body should parse");
        assert_eq!(json_body, Some(RequestBody::Json(json!({ "ok": true }))));

        let headers = parse_headers(Some(&json!({ "content-type": "application/xml" })))
            .expect("headers should parse");
        let text_body =
            parse_body(Some(&json!({ "ok": true })), &headers).expect("body should parse");
        assert_eq!(text_body, Some(RequestBody::Text(r#"{"ok":true}"#.into())));
    }

    #[test]
    fn rejects_managed_request_headers() {
        for header in ["host", "Content-Length", "transfer-encoding", "Upgrade"] {
            let result = parse_headers(Some(&json!({ header: "x" })));
            assert!(result.is_err(), "{header} should be rejected");
        }
    }

    #[test]
    fn rejects_body_for_get_and_head() {
        for method in [Method::GET, Method::HEAD] {
            let body = Some(RequestBody::Text("payload".into()));
            let result = validate_method_body(&method, &body);
            assert!(
                result.is_err(),
                "{} should reject a request body",
                method.as_str()
            );
        }
    }

    #[test]
    fn applies_scalar_and_array_query_parameters() {
        let mut url = Url::parse("https://example.com/search?existing=1").unwrap();
        apply_query(
            &mut url,
            Some(&json!({
                "tag": ["a", "b"],
                "active": true
            })),
        )
        .expect("query should parse");

        let pairs: Vec<_> = url.query_pairs().collect();
        assert!(
            pairs.contains(&("existing".into(), "1".into())),
            "existing query parameter should be preserved"
        );
        assert!(
            pairs.contains(&("tag".into(), "a".into())),
            "array query parameter should include tag=a"
        );
        assert!(
            pairs.contains(&("tag".into(), "b".into())),
            "array query parameter should include tag=b"
        );
        assert!(
            pairs.contains(&("active".into(), "true".into())),
            "boolean query parameter should be stringified"
        );
    }

    #[test]
    fn enforces_configuration_bounds() {
        let zero_response_limit = bounded_usize(
            &json!({ "maxResponseBytes": 0 }),
            "maxResponseBytes",
            DEFAULT_MAX_RESPONSE_BYTES,
            HARD_MAX_RESPONSE_BYTES,
        );
        assert!(zero_response_limit.is_err());

        let zero_redirects = bounded_usize_allow_zero(
            &json!({ "maxRedirects": 0 }),
            "maxRedirects",
            DEFAULT_MAX_REDIRECTS,
            HARD_MAX_REDIRECTS,
        )
        .expect("zero redirects is allowed");
        assert_eq!(zero_redirects, 0);

        let too_many_redirects = bounded_usize_allow_zero(
            &json!({ "maxRedirects": HARD_MAX_REDIRECTS + 1 }),
            "maxRedirects",
            DEFAULT_MAX_REDIRECTS,
            HARD_MAX_REDIRECTS,
        );
        assert!(too_many_redirects.is_err());
    }

    #[test]
    fn rejects_content_length_over_limit_before_reading_body() {
        let result = ensure_content_length_allowed(Some(101), 100);
        assert!(result.is_err());
    }
}
