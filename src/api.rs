//! OpenAI 호출 계층 — 두 경로.
//! - images:    POST {base}/images/generations  (API key 직결. 오늘의 검증된 경로)
//! - responses: POST {base}/responses + image_generation 툴 (OAuth 프록시 = ChatGPT 구독 quota)
//!
//! 재시도는 "요청이 서버에 닿기 전" 실패에만 한다: 연결 실패(`is_connect`, DNS·연결 타임아웃 포함)·
//! 429·500/502/503. 요청이 닿은 뒤의 실패(응답 대기 타임아웃, 응답 끊김, 504, 본문 수신 실패)는
//! 서버가 생성을 끝내 과금했을 수 있으므로 재시도하지 않는다. 최대 MAX_ATTEMPTS.
//! 오류 본문은 삼키지 않고 한 줄로 요약해 올린다. 키처럼 보이는 문자열과 env 의 실제 비밀값은 항상 가린다.

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::time::Duration;

pub const DEFAULT_BASE: &str = "https://api.openai.com/v1";
/// 총 시도 횟수 (= 1 + 재시도 3). 유료 API 라 상한을 둔다.
pub const MAX_ATTEMPTS: u32 = 4;
pub const RETRY_AFTER_CAP_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Images,
    Responses,
}

impl Route {
    pub fn name(self) -> &'static str {
        match self {
            Route::Images => "images",
            Route::Responses => "responses",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    ApiKey,
    ProxyToken,
    None,
}

impl AuthMode {
    pub fn label(self) -> &'static str {
        match self {
            AuthMode::ApiKey => "api-key",
            AuthMode::ProxyToken => "proxy-token",
            AuthMode::None => "none",
        }
    }
}

/// (base url, 프록시 사용 여부). OPENAI_BASE_URL 이 비어 있지 않으면 프록시다.
pub fn base_url_from_env() -> (String, bool) {
    match std::env::var("OPENAI_BASE_URL") {
        Ok(b) if !b.trim().is_empty() => (b.trim().trim_end_matches('/').to_string(), true),
        _ => (DEFAULT_BASE.to_string(), false),
    }
}

/// base URL 은 http(s) 스킴과 호스트가 있어야 한다 — 호출 전에 걸러 "닿은 뒤 오류" 로 오인하지 않게.
pub fn validate_base(base: &str) -> Result<(), String> {
    let u = reqwest::Url::parse(base)
        .map_err(|e| format!("OPENAI_BASE_URL '{}' 파싱 실패: {e}", display_base(base)))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err(format!(
            "OPENAI_BASE_URL '{}': http/https 만 가능",
            display_base(base)
        ));
    }
    if u.host_str().map(str::is_empty).unwrap_or(true) {
        return Err(format!(
            "OPENAI_BASE_URL '{}': 호스트가 없다",
            display_base(base)
        ));
    }
    Ok(())
}

/// 표시용 base URL — `https://user:token@host/…` 의 userinfo 를 지운다.
pub fn display_base(base: &str) -> String {
    match reqwest::Url::parse(base) {
        Ok(mut u) if !u.username().is_empty() || u.password().is_some() => {
            let _ = u.set_username("");
            let _ = u.set_password(None);
            u.to_string().trim_end_matches('/').to_string()
        }
        _ => redact(base),
    }
}

/// 사설·루프백·Tailscale·단일 라벨 호스트인가. 프록시 토큰은 이런 호스트에만 보낸다. 빈 호스트는 아니다.
pub fn is_private_host(host: &str) -> bool {
    let h = host.trim_matches(|c| c == '[' || c == ']');
    if h.is_empty() {
        return false;
    }
    if let Ok(v4) = h.parse::<std::net::Ipv4Addr>() {
        let o = v4.octets();
        return v4.is_loopback()
            || v4.is_private()
            || v4.is_link_local()
            || (o[0] == 100 && (64..=127).contains(&o[1])); // CGNAT 100.64/10 = Tailscale
    }
    if let Ok(v6) = h.parse::<std::net::Ipv6Addr>() {
        let seg0 = v6.segments()[0];
        return v6.is_loopback() || (seg0 & 0xfe00) == 0xfc00 || (seg0 & 0xffc0) == 0xfe80;
    }
    let lower = h.to_ascii_lowercase();
    lower == "localhost"
        || !lower.contains('.')
        || lower.ends_with(".ts.net")
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
}

pub struct Auth {
    pub bearer: Option<String>,
    pub mode: AuthMode,
    /// 사용자에게 알릴 주의사항 (있으면 한 줄)
    pub note: Option<String>,
}

/// 자격 선택 (값은 절대 밖으로 내지 않는다).
/// 프록시: OPENCODEX_API_AUTH_TOKEN(사설 호스트일 때만) > OPENAI_API_KEY > 없음. 직결: OPENAI_API_KEY.
pub fn auth_from_env(base: &str, proxy: bool) -> Auth {
    let nonempty = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let key = nonempty("OPENAI_API_KEY");
    let mut note = None;
    if proxy {
        if let Some(t) = nonempty("OPENCODEX_API_AUTH_TOKEN") {
            let host = reqwest::Url::parse(base)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_default();
            if is_private_host(&host) {
                return Auth {
                    bearer: Some(t),
                    mode: AuthMode::ProxyToken,
                    note: None,
                };
            }
            note = Some(format!(
                "[daedal] 주의: OPENCODEX_API_AUTH_TOKEN 은 사설·루프백·Tailscale 호스트에만 보낸다 — '{host}' 는 공개 호스트라 보내지 않음"
            ));
        }
    }
    match key {
        Some(k) => Auth {
            bearer: Some(k),
            mode: AuthMode::ApiKey,
            note,
        },
        None => Auth {
            bearer: None,
            mode: AuthMode::None,
            note,
        },
    }
}

pub struct GenRequest<'a> {
    /// 이미지 모델. Responses 경로에서 None 이면 툴의 model 필드를 생략한다(백엔드 기본값 = gpt-image-1).
    pub image_model: Option<&'a str>,
    /// Responses 경로의 메인라인 모델 (툴을 부르는 텍스트 모델).
    pub mainline: &'a str,
    pub prompt: &'a str,
    pub developer: &'a str,
    pub size: &'a str,
    pub quality: &'a str,
    pub format: &'a str,
    pub background: Option<&'a str>,
    pub compression: Option<u8>,
    pub moderation: Option<&'a str>,
    pub reasoning_effort: &'a str,
}

pub struct Generated {
    pub bytes: Vec<u8>,
    pub revised_prompt: Option<String>,
    pub usage: Option<Value>,
}

pub struct Api {
    http: reqwest::Client,
    base: String,
    bearer: Option<String>,
}

impl Api {
    pub fn new(base: &str, bearer: Option<String>, timeout: Duration) -> Result<Api> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(timeout)
            .user_agent(concat!("daedal/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("HTTP 클라이언트 생성")?;
        Ok(Api {
            http,
            base: base.trim_end_matches('/').to_string(),
            bearer,
        })
    }

    pub fn endpoint(&self, path: &str) -> String {
        format!("{}/{}", self.base, path)
    }

    fn authed(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.bearer {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        }
    }

    /// GET /models — 무료. 이미지 모델 확인·키 검증용.
    pub async fn list_models(&self) -> Result<Vec<(String, u64)>> {
        let url = self.endpoint("models");
        let resp = self
            .authed(self.http.get(&url))
            .send()
            .await
            .map_err(|e| anyhow!("{}", describe_transport(&e)))?;
        let status = resp.status().as_u16();
        let body = resp
            .text()
            .await
            .map_err(|e| anyhow!("models 응답 본문 수신 실패: {}", describe_transport(&e)))?;
        if !(200..300).contains(&status) {
            bail!("{}", describe_api_error(status, &body));
        }
        let v: Value = serde_json::from_str(&body).context("models 응답 JSON 파싱")?;
        let mut out: Vec<(String, u64)> = v["data"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| {
                        m["id"]
                            .as_str()
                            .map(|id| (id.to_string(), m["created"].as_u64().unwrap_or(0)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        Ok(out)
    }

    /// 이미지 1장 생성. `log` 는 재시도 같은 중간 소식을 stderr 로 내는 콜백.
    pub async fn generate(
        &self,
        route: Route,
        r: &GenRequest<'_>,
        log: &dyn Fn(&str),
    ) -> Result<Generated> {
        let (url, body, sse) = match route {
            Route::Images => (self.endpoint("images/generations"), images_body(r), false),
            Route::Responses => (self.endpoint("responses"), responses_body(r), true),
        };
        let text = self.post_with_retry(&url, &body, sse, log).await?;
        let (b64, revised_prompt, usage) = match route {
            Route::Images => extract_images(&text)?,
            Route::Responses => extract_sse(&text)?,
        };
        let bytes = STANDARD
            .decode(b64.as_bytes())
            .context("이미지 base64 디코드")?;
        if bytes.is_empty() {
            bail!("빈 이미지 데이터를 받았다");
        }
        Ok(Generated {
            bytes,
            revised_prompt,
            usage,
        })
    }

    async fn post_with_retry(
        &self,
        url: &str,
        body: &Value,
        sse: bool,
        log: &dyn Fn(&str),
    ) -> Result<String> {
        let mut attempt: u32 = 1;
        loop {
            let mut rb = self.http.post(url).json(body);
            if sse {
                rb = rb.header("Accept", "text/event-stream");
            }
            match self.authed(rb).send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let retry_after = resp
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.trim().parse::<u64>().ok());
                    let text = match resp.text().await {
                        Ok(t) => t,
                        Err(e) if (200..300).contains(&status) => bail!(
                            "2xx 응답 본문 수신 실패 — 재시도하지 않는다(서버는 생성을 끝내 과금됐을 수 있음): {}",
                            describe_transport(&e)
                        ),
                        // 비-2xx 는 결과가 없는 응답이라 본문 없이 상태코드로 판정한다
                        Err(_) => String::new(),
                    };
                    if (200..300).contains(&status) {
                        return Ok(text);
                    }
                    let msg = describe_api_error(status, &text);
                    let retryable = should_retry_status(status, &text);
                    if retryable && attempt < MAX_ATTEMPTS {
                        let wait = backoff(attempt, retry_after);
                        log(&format!(
                            "[daedal] {msg} — {}초 후 재시도 ({attempt}/{})",
                            wait.as_secs(),
                            MAX_ATTEMPTS - 1
                        ));
                        tokio::time::sleep(wait).await;
                        attempt += 1;
                        continue;
                    }
                    let suffix = match status {
                        504 => " (504 는 상류 타임아웃 — 과금 가능성이 있어 재시도하지 않는다)",
                        _ if retryable => " (재시도 상한 도달)",
                        _ => "",
                    };
                    bail!("{msg}{suffix}");
                }
                Err(e) => {
                    let msg = describe_transport(&e);
                    // 요청을 만들지도 못한 경우(URL/설정 문제) — 서버에 닿지 않았다
                    if e.is_builder() {
                        bail!("{msg} — 요청 구성 오류(URL·설정 문제, 서버에 닿지 않음)");
                    }
                    // 연결 자체가 안 된 것만 재시도 — 요청이 서버에 닿지 않았음이 확실한 경우다.
                    if e.is_connect() {
                        if attempt < MAX_ATTEMPTS {
                            let wait = backoff(attempt, None);
                            log(&format!(
                                "[daedal] {msg} — {}초 후 재시도 ({attempt}/{})",
                                wait.as_secs(),
                                MAX_ATTEMPTS - 1
                            ));
                            tokio::time::sleep(wait).await;
                            attempt += 1;
                            continue;
                        }
                        bail!("{msg} (재시도 상한 도달)");
                    }
                    if e.is_timeout() {
                        bail!("{msg} — 응답 대기 타임아웃은 재시도하지 않는다(서버가 생성을 끝내 과금됐을 수 있음)");
                    }
                    bail!(
                        "{msg} — 요청이 서버에 닿은 뒤의 오류라 재시도하지 않는다(이중 과금 방지)"
                    );
                }
            }
        }
    }
}

/// 429 / 500 / 502 / 503 만 재시도. 429 라도 quota 소진(insufficient_quota)은 기다려도 안 풀린다.
/// 504(상류 타임아웃)는 원본 서버가 생성을 끝내 과금했을 수 있어 제외한다.
pub fn should_retry_status(status: u16, body: &str) -> bool {
    match status {
        429 => !body.contains("insufficient_quota"),
        500 | 502 | 503 => true,
        _ => false,
    }
}

/// 지수 백오프 2·4·8초 + 0~1초 지터. Retry-After(정수 초만 인식)가 더 길면 그것(상한 60초).
pub fn backoff(attempt: u32, retry_after: Option<u64>) -> Duration {
    let base = 2u64.saturating_pow(attempt.clamp(1, 6));
    let jitter_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()) % 1000)
        .unwrap_or(0);
    let mut secs = base;
    if let Some(ra) = retry_after {
        secs = secs.max(ra.min(RETRY_AFTER_CAP_SECS));
    }
    Duration::from_millis(secs * 1000 + jitter_ms)
}

pub fn images_body(r: &GenRequest<'_>) -> Value {
    let mut b = json!({
        "model": r.image_model.unwrap_or("gpt-image-2"),
        "prompt": r.prompt,
        "n": 1,
        "size": r.size,
        "quality": r.quality,
        "output_format": r.format,
    });
    if let Some(bg) = r.background {
        b["background"] = json!(bg);
    }
    if let Some(c) = r.compression {
        b["output_compression"] = json!(c);
    }
    if let Some(m) = r.moderation {
        b["moderation"] = json!(m);
    }
    b
}

pub fn responses_body(r: &GenRequest<'_>) -> Value {
    let mut tool = json!({
        "type": "image_generation",
        "quality": r.quality,
        "size": r.size,
        "output_format": r.format,
    });
    if let Some(m) = r.image_model {
        tool["model"] = json!(m);
    }
    if let Some(bg) = r.background {
        tool["background"] = json!(bg);
    }
    if let Some(c) = r.compression {
        tool["output_compression"] = json!(c);
    }
    if let Some(m) = r.moderation {
        tool["moderation"] = json!(m);
    }
    json!({
        "model": r.mainline,
        "input": [
            {"role": "developer", "content": r.developer},
            {"role": "user", "content": r.prompt},
        ],
        "tools": [tool],
        "tool_choice": "required",
        "reasoning": {"effort": r.reasoning_effort},
        "stream": true,
    })
}

/// Images API 응답 → (b64, revised_prompt, usage)
pub fn extract_images(body: &str) -> Result<(String, Option<String>, Option<Value>)> {
    let v: Value = serde_json::from_str(body).map_err(|e| {
        anyhow!(
            "Images 응답 JSON 파싱 실패: {e} (본문 머리: {})",
            head(body, 300)
        )
    })?;
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        bail!("API 오류 본문: {}", redact(&err.to_string()));
    }
    let first = v["data"]
        .as_array()
        .and_then(|a| a.first())
        .ok_or_else(|| anyhow!("응답에 data[] 가 없다 (본문 머리: {})", head(body, 300)))?;
    let b64 = match first.get("b64_json").and_then(Value::as_str) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            if first.get("url").is_some() {
                bail!("url 형식 응답은 지원하지 않는다 (gpt-image 계열은 b64_json 을 준다)");
            }
            bail!(
                "data[0] 에 b64_json 이 없다 (본문 머리: {})",
                head(body, 300)
            );
        }
    };
    let revised = first
        .get("revised_prompt")
        .and_then(Value::as_str)
        .map(str::to_string);
    let usage = v.get("usage").cloned().filter(|u| !u.is_null());
    Ok((b64, revised, usage))
}

/// Response 객체(`output[]`·`usage`·`error`)를 훑은 결과.
#[derive(Default)]
struct Scan {
    found: Option<(String, Option<String>)>,
    usage: Option<Value>,
    /// 결과가 없을 때 사람이 볼 원인 후보 (툴 status, 모델 메시지, error)
    notes: Vec<String>,
}

fn scan_response_object(resp: &Value, scan: &mut Scan) {
    if let Some(items) = resp["output"].as_array() {
        for item in items {
            match item["type"].as_str().unwrap_or("") {
                "image_generation_call" => {
                    if let Some(b64) = item["result"].as_str().filter(|s| !s.is_empty()) {
                        if scan.found.is_none() {
                            let rp = item["revised_prompt"].as_str().map(str::to_string);
                            scan.found = Some((b64.to_string(), rp));
                        }
                    } else {
                        let status = item["status"].as_str().unwrap_or("?");
                        scan.notes
                            .push(format!("image_generation_call status={status}"));
                    }
                }
                "message" => {
                    if let Some(parts) = item["content"].as_array() {
                        for p in parts {
                            if let Some(t) = p["text"].as_str().filter(|t| !t.is_empty()) {
                                let t: String = t.chars().take(300).collect();
                                scan.notes.push(format!("모델 메시지: {t}"));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if !resp["usage"].is_null() {
        scan.usage = Some(resp["usage"].clone());
    }
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        let m = err["message"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| err.to_string());
        scan.notes.push(format!("error: {m}"));
    }
    if let Some(d) = resp.get("incomplete_details").filter(|d| !d.is_null()) {
        scan.notes.push(format!("incomplete_details: {d}"));
    }
}

/// Responses SSE 본문 → (b64, revised_prompt, usage). usage 는 메인라인 모델 토큰이다.
/// CRLF·CR 프레이밍과, 프록시가 `stream:true` 를 무시하고 준 비스트리밍 JSON 도 받는다.
pub fn extract_sse(raw: &str) -> Result<(String, Option<String>, Option<Value>)> {
    // JSON 문자열 안에는 원시 CR/LF 가 올 수 없으므로 줄바꿈 정규화는 안전하다
    let body = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut scan = Scan::default();
    let mut saw_data = false;

    for block in body.split("\n\n") {
        let mut data_lines: Vec<&str> = Vec::new();
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.strip_prefix(' ').unwrap_or(rest));
            }
        }
        if data_lines.is_empty() {
            continue;
        }
        saw_data = true;
        let data = data_lines.join("\n");
        if data.trim() == "[DONE]" {
            continue;
        }
        let ev: Value = match serde_json::from_str(&data) {
            Ok(e) => e,
            Err(_) => continue,
        };
        match ev["type"].as_str().unwrap_or("") {
            "response.output_item.done" => {
                let item = &ev["item"];
                if item["type"] == "image_generation_call" && scan.found.is_none() {
                    if let Some(b64) = item["result"].as_str().filter(|s| !s.is_empty()) {
                        let rp = item["revised_prompt"].as_str().map(str::to_string);
                        scan.found = Some((b64.to_string(), rp));
                    }
                }
            }
            "response.completed" | "response.incomplete" | "response.failed" => {
                scan_response_object(&ev["response"], &mut scan);
            }
            "error" => {
                let msg = ev["message"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| ev.to_string());
                scan.notes.push(format!(
                    "SSE error {}: {msg}",
                    ev["code"].as_str().unwrap_or("")
                ));
            }
            _ => {}
        }
    }

    // 비스트리밍 폴백: 프록시가 stream 을 무시하고 Response 객체 하나를 준 경우
    if !saw_data && body.trim_start().starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(&body) {
            scan_response_object(&v, &mut scan);
        }
    }

    match scan.found {
        Some((b64, rp)) => Ok((b64, rp, scan.usage)),
        None if !scan.notes.is_empty() => bail!(
            "Responses 경로에 이미지 결과가 없다 — {}",
            redact(&scan.notes.join(" | "))
        ),
        None => bail!(
            "SSE 스트림에 image_generation_call 결과가 없다 (본문 머리: {})",
            head(&body, 300)
        ),
    }
}

/// 비-2xx 응답을 한 줄로: `API 오류 HTTP 400 [invalid_request_error/invalid_value] param=size: message`
/// `{"error":"문자열"}` 형태(프록시가 흔히 쓴다)도 받는다.
pub fn describe_api_error(status: u16, body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            if let Some(s) = err.as_str() {
                return format!("API 오류 HTTP {status}: {}", redact(s));
            }
            let typ = err["type"].as_str().unwrap_or("");
            let code = err["code"].as_str().unwrap_or("");
            let msg = err["message"].as_str().unwrap_or("");
            let param = err["param"].as_str().unwrap_or("");
            let mut s = format!("API 오류 HTTP {status}");
            match (typ.is_empty(), code.is_empty()) {
                (false, false) => s.push_str(&format!(" [{typ}/{code}]")),
                (false, true) => s.push_str(&format!(" [{typ}]")),
                (true, false) => s.push_str(&format!(" [{code}]")),
                (true, true) => {}
            }
            if !param.is_empty() {
                s.push_str(&format!(" param={param}"));
            }
            if !msg.is_empty() {
                s.push_str(": ");
                s.push_str(&redact(msg));
            }
            return s;
        }
    }
    if body.trim().is_empty() {
        return format!("API 오류 HTTP {status} (본문 없음)");
    }
    format!("API 오류 HTTP {status}: {}", head(body, 600))
}

/// 오류와 그 source 체인을 한 줄로 (reqwest 의 Display 는 원인을 감춘다).
pub fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![e.to_string()];
    let mut cur = e.source();
    while let Some(s) = cur {
        let m = s.to_string();
        if parts.last() != Some(&m) {
            parts.push(m);
        }
        cur = s.source();
    }
    parts.join(" ← ")
}

pub fn describe_transport(e: &reqwest::Error) -> String {
    let kind = if e.is_builder() {
        "요청 구성 오류"
    } else if e.is_connect() {
        "연결 실패"
    } else if e.is_timeout() {
        "타임아웃"
    } else if e.is_decode() {
        "응답 디코드 실패"
    } else {
        "전송 오류"
    };
    format!("{kind}: {}", redact(&error_chain(e)))
}

fn head(s: &str, n: usize) -> String {
    let h: String = s.chars().take(n).collect();
    redact(&h).replace('\n', " ")
}

/// 비밀값 가리기. (1) `sk-…`·`Bearer …` 패턴 (2) env 에 든 실제 값(OPENAI_API_KEY·OPENCODEX_API_AUTH_TOKEN).
/// 어떤 오류 문구도 이 함수를 거쳐 나간다.
pub fn redact(s: &str) -> String {
    let mut out = redact_patterns(s);
    for name in ["OPENAI_API_KEY", "OPENCODEX_API_AUTH_TOKEN"] {
        if let Ok(v) = std::env::var(name) {
            let v = v.trim();
            if v.len() >= 8 && out.contains(v) {
                out = out.replace(v, "***");
            }
        }
    }
    out
}

fn redact_patterns(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let prev_alnum = i > 0 && bytes[i - 1].is_ascii_alphanumeric();
        let plen = if rest.starts_with("sk-") && !prev_alnum {
            3
        } else if rest.starts_with("Bearer ") {
            7
        } else {
            0
        };
        if plen > 0 {
            let tok_len = rest[plen..]
                .bytes()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
                .count();
            if tok_len >= 8 {
                out.push_str(&rest[..plen]);
                out.push_str("***");
                i += plen + tok_len;
                continue;
            }
        }
        let ch = rest.chars().next().unwrap_or('?');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn req(model: Option<&'static str>) -> GenRequest<'static> {
        GenRequest {
            image_model: model,
            mainline: "gpt-5.6-luna",
            prompt: "제목 '안전 출근' 포스터",
            developer: "dev",
            size: "1536x1024",
            quality: "high",
            format: "png",
            background: None,
            compression: None,
            moderation: None,
            reasoning_effort: "low",
        }
    }

    #[test]
    fn images_body_omits_optional_fields_and_sends_n1() {
        let b = images_body(&req(Some("gpt-image-2")));
        assert_eq!(b["model"], "gpt-image-2");
        assert_eq!(b["n"], 1);
        assert_eq!(b["output_format"], "png");
        assert!(b.get("background").is_none() && b.get("moderation").is_none());
        let mut r = req(Some("gpt-image-2"));
        r.background = Some("transparent");
        r.compression = Some(80);
        r.moderation = Some("low");
        let b = images_body(&r);
        assert_eq!(b["background"], "transparent");
        assert_eq!(b["output_compression"], 80);
        assert_eq!(b["moderation"], "low");
    }

    #[test]
    fn responses_body_pins_tool_model_when_given() {
        let b = responses_body(&req(Some("gpt-image-2")));
        assert_eq!(b["model"], "gpt-5.6-luna");
        assert_eq!(b["tools"][0]["type"], "image_generation");
        assert_eq!(b["tools"][0]["model"], "gpt-image-2");
        assert_eq!(b["tool_choice"], "required");
        assert_eq!(b["stream"], true);
        let b = responses_body(&req(None));
        assert!(b["tools"][0].get("model").is_none());
    }

    #[test]
    fn extract_images_happy_and_error_paths() {
        let body = r#"{"created":1,"data":[{"b64_json":"aGVsbG8=","revised_prompt":"rp"}],"usage":{"input_tokens":440,"input_tokens_details":{"image_tokens":0,"text_tokens":440},"output_tokens":5488,"total_tokens":5928}}"#;
        let (b64, rp, usage) = extract_images(body).unwrap();
        assert_eq!(b64, "aGVsbG8=");
        assert_eq!(rp.as_deref(), Some("rp"));
        assert_eq!(usage.unwrap()["output_tokens"], 5488);
        // "error": null 은 오류가 아니다
        assert!(extract_images(r#"{"error":null,"data":[{"b64_json":"aGk="}]}"#).is_ok());
        assert!(extract_images(r#"{"data":[{"url":"http://x"}]}"#)
            .unwrap_err()
            .to_string()
            .contains("url"));
        assert!(extract_images(r#"{"data":[]}"#).is_err());
        assert!(extract_images("not json").is_err());
    }

    const LF_SSE: &str = "event: x\ndata: {\"type\":\"response.created\"}\n\n\
data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"image_generation_call\",\"result\":\"aGVsbG8=\",\"revised_prompt\":\"r\"}}\n\n\
data: {\"type\":\"response.completed\",\"response\":{\"output\":[],\"usage\":{\"input_tokens\":700,\"output_tokens\":120}}}\n\n\
data: [DONE]\n\n";

    #[test]
    fn extract_sse_lf_crlf_cr_and_completed_fallback() {
        let (b64, rp, usage) = extract_sse(LF_SSE).unwrap();
        assert_eq!(b64, "aGVsbG8=");
        assert_eq!(rp.as_deref(), Some("r"));
        assert_eq!(usage.unwrap()["input_tokens"], 700);
        assert_eq!(
            extract_sse(&LF_SSE.replace('\n', "\r\n")).unwrap().0,
            "aGVsbG8="
        );
        assert_eq!(
            extract_sse(&LF_SSE.replace('\n', "\r")).unwrap().0,
            "aGVsbG8="
        ); // CR 단독

        let only_completed = "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"image_generation_call\",\"result\":\"YQ==\"}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n\n";
        let (b64, _, _) = extract_sse(only_completed).unwrap();
        assert_eq!(b64, "YQ==");
    }

    #[test]
    fn extract_sse_accepts_non_streaming_response_object() {
        let body = r#"{"object":"response","output":[{"type":"image_generation_call","result":"YQ=="}],"usage":{"input_tokens":3,"output_tokens":4}}"#;
        let (b64, _, usage) = extract_sse(body).unwrap();
        assert_eq!(b64, "YQ==");
        assert_eq!(usage.unwrap()["input_tokens"], 3);
    }

    #[test]
    fn extract_sse_surfaces_failures_with_cause() {
        let failed = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"boom sk-abcdefghijklmnop\"}}}\n\n";
        let e = extract_sse(failed).unwrap_err().to_string();
        assert!(e.contains("boom") && !e.contains("abcdefghijklmnop"), "{e}");
        let err_ev =
            "data: {\"type\":\"error\",\"code\":\"rate_limit\",\"message\":\"slow down\"}\n\n";
        assert!(extract_sse(err_ev)
            .unwrap_err()
            .to_string()
            .contains("slow down"));
        assert!(extract_sse("data: {\"type\":\"response.created\"}\n\n").is_err());
        let failed_item = "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"image_generation_call\",\"status\":\"failed\",\"result\":null},{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"The request was flagged by the safety system.\"}]}]}}\n\n";
        let e = extract_sse(failed_item).unwrap_err().to_string();
        assert!(
            e.contains("safety system") && e.contains("status=failed"),
            "{e}"
        );
    }

    #[test]
    fn api_error_description_and_retry_policy() {
        let body = r#"{"error":{"message":"Invalid value: '2048x1152'. Supported values are: '1024x1024'","type":"invalid_request_error","param":"size","code":"invalid_value"}}"#;
        let s = describe_api_error(400, body);
        assert!(
            s.starts_with(
                "API 오류 HTTP 400 [invalid_request_error/invalid_value] param=size: Invalid value"
            ),
            "{s}"
        );
        assert_eq!(
            describe_api_error(
                503,
                r#"{"error":{"message":"overloaded","type":"server_error"}}"#
            ),
            "API 오류 HTTP 503 [server_error]: overloaded"
        );
        // 프록시가 흔히 쓰는 문자열 error
        assert_eq!(
            describe_api_error(401, r#"{"error":"invalid token"}"#),
            "API 오류 HTTP 401: invalid token"
        );
        assert_eq!(
            describe_api_error(502, "<html>bad gateway</html>"),
            "API 오류 HTTP 502: <html>bad gateway</html>"
        );
        assert_eq!(describe_api_error(503, ""), "API 오류 HTTP 503 (본문 없음)");
        assert!(should_retry_status(429, "{}"));
        assert!(!should_retry_status(
            429,
            r#"{"error":{"code":"insufficient_quota"}}"#
        ));
        assert!(should_retry_status(503, "") && should_retry_status(500, ""));
        assert!(
            !should_retry_status(504, "")
                && !should_retry_status(400, "")
                && !should_retry_status(401, "")
        );
        let d = backoff(1, None);
        assert!(
            d >= Duration::from_secs(2) && d < Duration::from_secs(3),
            "{d:?}"
        );
        let d = backoff(2, Some(30));
        assert!(d >= Duration::from_secs(30) && d < Duration::from_secs(31));
        let d = backoff(3, Some(600));
        assert!(d >= Duration::from_secs(60) && d < Duration::from_secs(61));
    }

    #[test]
    fn redact_hides_keys_and_bearer_but_not_words() {
        let s = redact("key sk-abcdefghijkl and Bearer example.jwt.value done sk-ab");
        assert_eq!(s, "key sk-*** and Bearer *** done sk-ab");
        assert_eq!(redact("no secrets"), "no secrets");
        assert_eq!(redact("sk-"), "sk-");
        assert_eq!(redact("risk-assessment"), "risk-assessment");
        assert_eq!(redact("task-management done"), "task-management done");
        assert_eq!(redact("한글 sk-abcdefghijkl 끝"), "한글 sk-*** 끝");
    }

    #[test]
    fn base_validation_display_and_private_hosts() {
        assert!(validate_base("https://api.openai.com/v1").is_ok());
        assert!(validate_base("http://proxy.internal:11532/v1").is_ok());
        assert!(validate_base("foo:bar/v1").is_err());
        assert!(validate_base("ftp://x/v1").is_err());
        assert!(validate_base("/v1").is_err());
        assert_eq!(
            display_base("https://user:tok@proxy.internal:11532/v1"),
            "https://proxy.internal:11532/v1"
        );
        assert_eq!(
            display_base("https://api.openai.com/v1"),
            "https://api.openai.com/v1"
        );
        for h in [
            "127.0.0.1",
            "localhost",
            "proxy",
            "100.100.100.100",
            "192.168.1.10",
            "10.0.0.5",
            "host.tailnet.ts.net",
            "::1",
            "[::1]",
            "fd7a:115c:a1e0::1",
            "169.254.1.1",
        ] {
            assert!(is_private_host(h), "{h}");
        }
        for h in [
            "",
            "api.openai.com",
            "example.com",
            "8.8.8.8",
            "100.200.1.1",
            "100.128.0.1",
        ] {
            assert!(!is_private_host(h), "{h:?}");
        }
    }

    // ── 목(mock) 서버로 재시도 정책 실측 ──

    /// 요청 전문을 읽는다 (헤더 + Content-Length 만큼).
    fn read_request(s: &mut std::net::TcpStream) -> String {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            let n = s.read(&mut tmp).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            let text = String::from_utf8_lossy(&buf);
            if let Some(pos) = text.find("\r\n\r\n") {
                let cl = text
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                if buf.len() >= pos + 4 + cl {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    }

    /// Content-Length 를 본문 길이로 계산해 만든다 (손으로 세면 틀린다 — 실제로 틀렸었다).
    fn http(status: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn spawn_mock(
        handler: impl Fn(usize, &mut std::net::TcpStream) + Send + 'static,
    ) -> (u16, Arc<AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { break };
                let n = h.fetch_add(1, Ordering::SeqCst);
                handler(n, &mut s);
            }
        });
        (port, hits)
    }

    #[tokio::test]
    async fn request_delivered_then_dropped_is_not_retried() {
        // 서버가 요청 전문을 읽고 응답 없이 끊으면 재전송하지 않는다 (이중 과금 방지)
        let (port, hits) = spawn_mock(|_, s| {
            let _ = read_request(s);
            let _ = s.shutdown(std::net::Shutdown::Both);
        });
        let api = Api::new(
            &format!("http://127.0.0.1:{port}/v1"),
            None,
            Duration::from_secs(5),
        )
        .unwrap();
        let err = api
            .post_with_retry(
                &api.endpoint("images/generations"),
                &json!({"prompt": "x"}),
                false,
                &|_| {},
            )
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(hits.load(Ordering::SeqCst), 1, "{err}");
        assert!(err.contains("재시도하지 않는다"), "{err}");
    }

    #[tokio::test]
    async fn retries_503_then_succeeds_and_gives_up_on_400() {
        let (port, hits) = spawn_mock(|n, s| {
            let _ = read_request(s);
            let resp = if n == 0 {
                http(
                    "503 Service Unavailable",
                    r#"{"error":{"message":"overloaded","code":null}}"#,
                )
            } else {
                http("200 OK", r#"{"data":[{"b64_json":"aGk="}]}"#)
            };
            let _ = s.write_all(&resp);
            let _ = s.shutdown(std::net::Shutdown::Both);
        });
        let api = Api::new(
            &format!("http://127.0.0.1:{port}/v1"),
            None,
            Duration::from_secs(5),
        )
        .unwrap();
        let logs = std::sync::Mutex::new(Vec::new());
        let text = api
            .post_with_retry(
                &api.endpoint("images/generations"),
                &json!({"prompt": "x"}),
                false,
                &|m| logs.lock().unwrap().push(m.to_string()),
            )
            .await
            .unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert!(text.contains("aGk="));
        assert!(
            logs.lock().unwrap()[0].contains("503")
                && logs.lock().unwrap()[0].contains("재시도 (1/3)")
        );

        let (port2, hits2) = spawn_mock(|_, s| {
            let _ = read_request(s);
            let _ = s.write_all(&http(
                "400 Bad Request",
                r#"{"error":{"message":"bad size","type":"invalid_request_error"}}"#,
            ));
            let _ = s.shutdown(std::net::Shutdown::Both);
        });
        let api2 = Api::new(
            &format!("http://127.0.0.1:{port2}/v1"),
            None,
            Duration::from_secs(5),
        )
        .unwrap();
        let err = api2
            .post_with_retry(
                &api2.endpoint("images/generations"),
                &json!({}),
                false,
                &|_| {},
            )
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(hits2.load(Ordering::SeqCst), 1);
        assert!(
            err.contains("HTTP 400 [invalid_request_error]: bad size"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn builder_error_is_reported_as_config_not_delivered() {
        let api = Api::new("foo:bar/v1", None, Duration::from_secs(5)).unwrap();
        let err = api
            .post_with_retry(
                &api.endpoint("images/generations"),
                &json!({}),
                false,
                &|_| {},
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("요청 구성 오류") && !err.contains("서버에 닿은 뒤"),
            "{err}"
        );
    }
}
