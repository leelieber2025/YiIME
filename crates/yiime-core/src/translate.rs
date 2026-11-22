use crate::settings::{ApiSettings, Engine};
use crate::style::{TranslationStyle, system_prompt};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::env;
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub fn translate_openai(source: &str) -> Result<String, String> {
    let endpoint = env::var("TRANSLATION_API_URL").map_err(|_| "缺少 TRANSLATION_API_URL")?;
    let model = env::var("TRANSLATION_MODEL").map_err(|_| "缺少 TRANSLATION_MODEL")?;
    let key = env::var("TRANSLATION_API_KEY").map_err(|_| "缺少 TRANSLATION_API_KEY")?;
    translate_openai_with(
        source,
        &endpoint,
        &model,
        &key,
        TranslationStyle::default(),
        "",
    )
}

/// Timing of one request, for the diagnostic log (never the key or the text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub engine: Engine,
    /// LLM model; empty for Google.
    pub model: String,
    pub host: String,
    /// Milliseconds from the start of the request to the response headers.
    pub first_byte_ms: Option<u128>,
    pub total_ms: u128,
    pub chars: usize,
    /// Failure category only (e.g. `http 429`, `timeout`); never provider error text.
    pub failure: Option<String>,
}

/// A failed request: `message` is shown to the user, `category` is the only part that is logged.
#[derive(Debug)]
struct Fail {
    category: String,
    message: String,
}

impl Fail {
    fn new(category: &str, message: impl Into<String>) -> Self {
        Self {
            category: category.to_owned(),
            message: message.into(),
        }
    }
}

impl From<&str> for Fail {
    fn from(message: &str) -> Self {
        Fail::new("bad response", message)
    }
}

impl From<String> for Fail {
    fn from(message: String) -> Self {
        Fail::new("bad response", message)
    }
}

/// Result of a translation and its attempts (e.g. Google failed, then the LLM).
#[derive(Debug)]
pub struct Outcome {
    pub result: Result<String, String>,
    pub attempts: Vec<Attempt>,
}

impl Attempt {
    /// One diagnostic log line, e.g. `engine=llm model=gpt-4o-mini host=api.openai.com chars=12
    /// first_byte=820ms total=905ms ok`.
    pub fn log_line(&self) -> String {
        let model = if self.model.is_empty() {
            String::new()
        } else {
            format!(" model={}", self.model)
        };
        let first = self
            .first_byte_ms
            .map(|ms| format!(" first_byte={ms}ms"))
            .unwrap_or_default();
        let status = match &self.failure {
            None => "ok".to_owned(),
            Some(category) => format!("error={category}"),
        };
        format!(
            "engine={}{model} host={} chars={}{first} total={}ms {status}",
            self.engine.id(),
            self.host,
            self.chars,
            self.total_ms
        )
    }
}

/// Free Google usually answers in 200–400 ms; after this the LLM is tried instead.
const GOOGLE_FREE_TIMEOUT: Duration = Duration::from_secs(4);
const GOOGLE_CLOUD_TIMEOUT: Duration = Duration::from_secs(8);
const LLM_TIMEOUT: Duration = Duration::from_secs(20);
const GOOGLE_FREE_URL: &str = "https://translate.googleapis.com/translate_a/single";
const AZURE_TIMEOUT: Duration = Duration::from_secs(6);
const AZURE_URL: &str = "https://api.cognitive.microsofttranslator.com/translate";
const GOOGLE_CLOUD_URL: &str = "https://translation.googleapis.com/language/translate/v2";

/// Shared HTTP clients (with and without proxy) so connections are reused and DNS/TCP/TLS happen
/// once.
fn agent(bypass_proxy: bool) -> &'static ureq::Agent {
    static AGENTS: OnceLock<[ureq::Agent; 2]> = OnceLock::new();
    let agents = AGENTS.get_or_init(|| {
        let build = |proxy: bool| {
            ureq::AgentBuilder::new()
                .try_proxy_from_env(proxy)
                .timeout_connect(Duration::from_secs(5))
                .max_idle_connections_per_host(2)
                .build()
        };
        [build(true), build(false)]
    });
    &agents[bypass_proxy as usize]
}

fn host_of(endpoint: &str) -> String {
    url::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default()
}

fn http_error(error: ureq::Error) -> Fail {
    match error {
        ureq::Error::Status(code, response) => {
            let category = format!("http {code}");
            let explanation = response.into_string().unwrap_or_default();
            let message = serde_json::from_str::<Value>(&explanation)
                .ok()
                .and_then(|json| json["error"]["message"].as_str().map(str::to_owned))
                .unwrap_or_else(|| {
                    // HTML error pages (Google rate limiting) do not fit in the status line.
                    if explanation.trim_start().starts_with('<') {
                        String::new()
                    } else {
                        explanation.chars().take(300).collect()
                    }
                });
            let message = if message.trim().is_empty() {
                String::new()
            } else {
                format!(": {message}")
            };
            if code == 429 {
                Fail::new(
                    &category,
                    format!("HTTP 429（请求太频繁，被限流）{message}"),
                )
            } else {
                Fail::new(&category, format!("HTTP {code}{message}"))
            }
        }
        ureq::Error::Transport(transport) => {
            // `Display` includes the request URL (the free Google query carries the source
            // text), so the user sees only the kind and the URL-free message.
            let text = transport.to_string();
            if text.contains("timed out") || text.contains("Timeout") {
                Fail::new("timeout", "请求超时")
            } else {
                let detail = transport
                    .message()
                    .map(|message| format!("（{message}）"))
                    .unwrap_or_default();
                Fail::new("network", format!("网络错误：{}{detail}", transport.kind()))
            }
        }
    }
}

/// Skip free Google this long after a 429 when an LLM can take over.
const GOOGLE_COOLDOWN: Duration = Duration::from_secs(300);
static GOOGLE_BLOCKED: Mutex<Option<Instant>> = Mutex::new(None);

fn google_blocked() -> bool {
    let blocked = GOOGLE_BLOCKED.lock().unwrap_or_else(|p| p.into_inner());
    blocked.is_some_and(|at| at.elapsed() < GOOGLE_COOLDOWN)
}

fn set_google_blocked(on: bool) {
    *GOOGLE_BLOCKED.lock().unwrap_or_else(|p| p.into_inner()) = on.then(Instant::now);
}

type CacheKey = (Engine, String, String, TranslationStyle, String, String);
const CACHE_CAP: usize = 128;
static CACHE: Mutex<Vec<(CacheKey, String)>> = Mutex::new(Vec::new());

fn cache_key(settings: &ApiSettings, source: &str) -> CacheKey {
    (
        settings.engine,
        settings.url.trim().to_owned(),
        settings.model.trim().to_owned(),
        settings.style,
        settings.custom.clone(),
        source.trim().to_owned(),
    )
}

/// Translates with the selected engine; if Google (free or Cloud) fails and an LLM is configured,
/// falls back to the LLM. Recent results are cached; free Google is skipped for a while after 429.
pub fn translate(settings: &ApiSettings, source: &str) -> Outcome {
    let key = cache_key(settings, source);
    {
        let cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((_, text)) = cache.iter().find(|(k, _)| *k == key) {
            return Outcome {
                result: Ok(text.clone()),
                attempts: Vec::new(),
            };
        }
    }
    let mut attempts = Vec::new();
    let llm_ok = settings.validate_llm().is_ok();
    let engine = match settings.engine {
        Engine::Azure if settings.azure_key.trim().is_empty() => Engine::GoogleFree,
        other => other,
    };
    let skip_google = engine == Engine::GoogleFree && llm_ok && google_blocked();
    let result = if skip_google {
        run(settings, Engine::Llm, source, &mut attempts)
    } else {
        let first = run(settings, engine, source, &mut attempts);
        if engine == Engine::GoogleFree {
            let limited = attempts.last().and_then(|a| a.failure.as_deref()) == Some("http 429");
            set_google_blocked(limited);
        }
        match first {
            Err(error) if engine != Engine::Llm && llm_ok => {
                run(settings, Engine::Llm, source, &mut attempts)
                    .map_err(|llm| format!("{error}；改用大模型也失败：{llm}"))
            }
            Err(error) if engine == Engine::Azure && !google_blocked() => {
                run(settings, Engine::GoogleFree, source, &mut attempts)
                    .map_err(|google| format!("{error}；改用 Google 也失败：{google}"))
            }
            other => other,
        }
    };
    if let Ok(text) = &result {
        let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if cache.len() >= CACHE_CAP {
            cache.remove(0);
        }
        cache.push((key, text.clone()));
    }
    Outcome { result, attempts }
}

fn run(
    settings: &ApiSettings,
    engine: Engine,
    source: &str,
    attempts: &mut Vec<Attempt>,
) -> Result<String, String> {
    let started = Instant::now();
    let mut first_byte = None;
    let (host, model, result) = match engine {
        Engine::Azure => (
            host_of(AZURE_URL),
            String::new(),
            azure(
                source,
                settings.azure_key.trim(),
                settings.azure_region.trim(),
                &mut first_byte,
                started,
            ),
        ),
        Engine::GoogleFree => (
            host_of(GOOGLE_FREE_URL),
            String::new(),
            google_free(source, &mut first_byte, started),
        ),
        Engine::GoogleCloud => (
            host_of(GOOGLE_CLOUD_URL),
            String::new(),
            google_cloud(source, settings.google_key.trim(), &mut first_byte, started),
        ),
        Engine::Llm => (
            host_of(settings.url.trim()),
            settings.model.trim().to_owned(),
            llm(
                source,
                settings.url.trim(),
                settings.model.trim(),
                settings.key.trim(),
                settings.style,
                &settings.custom,
                &mut first_byte,
                started,
            ),
        ),
    };
    attempts.push(Attempt {
        engine,
        model,
        host,
        first_byte_ms: first_byte,
        total_ms: started.elapsed().as_millis(),
        chars: source.chars().count(),
        failure: result.as_ref().err().map(|fail| fail.category.clone()),
    });
    result.map_err(|fail| fail.message)
}

/// Free Google endpoint, answering `[[["dst","src",…],…],…]`; the pieces are joined.
fn google_free(
    source: &str,
    first_byte: &mut Option<u128>,
    started: Instant,
) -> Result<String, Fail> {
    let response = agent(false)
        .get(GOOGLE_FREE_URL)
        .timeout(GOOGLE_FREE_TIMEOUT)
        .query("client", "gtx")
        .query("sl", "zh-CN")
        .query("tl", "en")
        .query("dt", "t")
        .query("q", source)
        .call()
        .map_err(http_error)?;
    *first_byte = Some(started.elapsed().as_millis());
    let json: Value = response
        .into_json()
        .map_err(|_| "Google 翻译返回格式不对")?;
    Ok(parse_google_free(&json)?)
}

fn parse_google_free(json: &Value) -> Result<String, String> {
    let text: String = json[0]
        .as_array()
        .ok_or("Google 翻译返回格式不对")?
        .iter()
        .filter_map(|segment| segment[0].as_str())
        .collect();
    let text = text.trim();
    if text.is_empty() {
        Err("译文为空".into())
    } else {
        Ok(text.to_owned())
    }
}

/// Azure Translator v3; region is required for regional resources, optional for global.
fn azure(
    source: &str,
    key: &str,
    region: &str,
    first_byte: &mut Option<u128>,
    started: Instant,
) -> Result<String, Fail> {
    if key.is_empty() {
        return Err(Fail::new("config", "缺少 Azure 翻译的密钥"));
    }
    let mut request = agent(false)
        .post(AZURE_URL)
        .timeout(AZURE_TIMEOUT)
        .query("api-version", "3.0")
        .query("from", "zh-Hans")
        .query("to", "en")
        .set("Ocp-Apim-Subscription-Key", key);
    if !region.is_empty() && !region.eq_ignore_ascii_case("global") {
        request = request.set("Ocp-Apim-Subscription-Region", region);
    }
    let response = request
        .send_json(json!([{ "Text": source }]))
        .map_err(http_error)?;
    *first_byte = Some(started.elapsed().as_millis());
    let json: Value = response.into_json().map_err(|_| "Azure 翻译返回格式不对")?;
    parse_azure(&json).map_err(|message| Fail::new("empty", message))
}

fn parse_azure(json: &Value) -> Result<String, String> {
    json[0]["translations"][0]["text"]
        .as_str()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| "Azure 翻译没有返回译文".into())
}

/// Google Cloud Translation v2 (`?key=`).
fn google_cloud(
    source: &str,
    key: &str,
    first_byte: &mut Option<u128>,
    started: Instant,
) -> Result<String, Fail> {
    if key.is_empty() {
        return Err(Fail::new("config", "缺少 Google Cloud 翻译的 API 密钥"));
    }
    let response = agent(false)
        .post(GOOGLE_CLOUD_URL)
        .timeout(GOOGLE_CLOUD_TIMEOUT)
        .query("key", key)
        .send_json(json!({"q": source, "source": "zh-CN", "target": "en", "format": "text"}))
        .map_err(http_error)?;
    *first_byte = Some(started.elapsed().as_millis());
    let json: Value = response
        .into_json()
        .map_err(|_| "Google Cloud 返回格式不对")?;
    json["data"]["translations"][0]["translatedText"]
        .as_str()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| Fail::new("empty", "Google Cloud 没有返回译文"))
}

/// (endpoint, model) pairs that rejected temperature / max_tokens (some reasoning models answer
/// 400); only those pairs get the reduced request, other models keep the full one.
static MINIMAL_BODY: Mutex<BTreeSet<(String, String)>> = Mutex::new(BTreeSet::new());

fn minimal_body() -> std::sync::MutexGuard<'static, BTreeSet<(String, String)>> {
    MINIMAL_BODY
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn wants_minimal(endpoint: &str, model: &str) -> bool {
    minimal_body().contains(&(endpoint.to_owned(), model.to_owned()))
}

fn mark_minimal(endpoint: &str, model: &str) {
    minimal_body().insert((endpoint.to_owned(), model.to_owned()));
}

pub fn translate_openai_with(
    source: &str,
    endpoint: &str,
    model: &str,
    key: &str,
    style: TranslationStyle,
    custom: &str,
) -> Result<String, String> {
    llm(
        source,
        endpoint.trim(),
        model.trim(),
        key.trim(),
        style,
        custom,
        &mut None,
        Instant::now(),
    )
    .map_err(|fail| fail.message)
}

#[allow(clippy::too_many_arguments, clippy::result_large_err)]
fn llm(
    source: &str,
    endpoint: &str,
    model: &str,
    key: &str,
    style: TranslationStyle,
    custom: &str,
    first_byte: &mut Option<u128>,
    started: Instant,
) -> Result<String, Fail> {
    if endpoint.is_empty() || model.is_empty() || key.is_empty() {
        return Err(Fail::new("config", "请填写 API 地址、模型和密钥"));
    }
    let messages = json!([
        {"role": "system", "content": system_prompt(style, custom)},
        {"role": "user", "content": source}
    ]);
    // English is about 1–2 tokens per Chinese character: leave headroom without allowing long
    // rambles.
    let max_tokens = (source.chars().count() * 4 + 64).min(2048);
    let full = json!({
        "model": model,
        "messages": messages,
        "temperature": 0,
        "max_tokens": max_tokens,
    });
    let minimal = json!({"model": model, "messages": messages});
    let agent = agent(bypass_proxy(endpoint));
    let send = |body: &Value| {
        agent
            .post(endpoint)
            .timeout(LLM_TIMEOUT)
            .set("Authorization", &format!("Bearer {key}"))
            .send_json(body.clone())
    };
    let response = if wants_minimal(endpoint, model) {
        send(&minimal)
    } else {
        match send(&full) {
            Err(ureq::Error::Status(400, response)) => {
                let text = response.into_string().unwrap_or_default();
                let lower = text.to_ascii_lowercase();
                if lower.contains("temperature")
                    || lower.contains("max_tokens")
                    || lower.contains("unsupported")
                {
                    // Remember only when the reduced request works: the 400 may be about
                    // something else (e.g. an unsupported `system` role or model).
                    let retry = send(&minimal);
                    if retry.is_ok() {
                        mark_minimal(endpoint, model);
                    }
                    retry
                } else {
                    let message = serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|json| json["error"]["message"].as_str().map(str::to_owned))
                        .unwrap_or_else(|| text.chars().take(300).collect());
                    return Err(Fail::new("http 400", format!("HTTP 400: {message}")));
                }
            }
            other => other,
        }
    }
    .map_err(http_error)?;
    *first_byte = Some(started.elapsed().as_millis());
    let response: Value = response.into_json().map_err(|_| "接口返回格式不对")?;
    let mut choice = response["choices"][0].clone();
    // Reasoning models may spend the whole max_tokens budget thinking: retry without the cap.
    if truncated_empty(&choice) && !wants_minimal(endpoint, model) {
        let retry: Value = send(&minimal)
            .map_err(http_error)?
            .into_json()
            .map_err(|_| "接口返回格式不对")?;
        choice = retry["choices"][0].clone();
        if !truncated_empty(&choice) && choice["message"]["content"].is_string() {
            mark_minimal(endpoint, model);
        }
    }
    if truncated_empty(&choice) {
        return Err(Fail::new(
            "truncated",
            "模型输出被截断（可能是推理模型思考过长），请换用非推理模型",
        ));
    }
    let content = choice["message"]["content"]
        .as_str()
        .ok_or_else(|| Fail::new("empty", "接口没有返回译文"))?;
    Ok(strip_leading_reasoning(content)?)
}

fn truncated_empty(choice: &Value) -> bool {
    choice["finish_reason"].as_str() == Some("length")
        && choice["message"]["content"]
            .as_str()
            .is_none_or(|c| c.trim().is_empty())
}

/// Opens connections to the translation server early (when the input box opens) so DNS/TCP/TLS
/// finish while typing. Sends only a HEAD request without key or text; failures are ignored. At
/// most once per 30 seconds.
pub fn prewarm(settings: &ApiSettings) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut last = LAST.lock().unwrap_or_else(|poison| poison.into_inner());
        if last.is_some_and(|at| at.elapsed() < Duration::from_secs(30)) {
            return;
        }
        *last = Some(Instant::now());
    }
    let mut targets = Vec::new();
    match settings.engine {
        Engine::Azure if settings.azure_key.trim().is_empty() => {
            targets.push((GOOGLE_FREE_URL.to_owned(), false))
        }
        Engine::Azure => targets.push((AZURE_URL.to_owned(), false)),
        Engine::GoogleFree => targets.push((GOOGLE_FREE_URL.to_owned(), false)),
        Engine::GoogleCloud => targets.push((GOOGLE_CLOUD_URL.to_owned(), false)),
        Engine::Llm => {}
    }
    if settings.validate_llm().is_ok() {
        let url = settings.url.trim().to_owned();
        let bypass = bypass_proxy(&url);
        targets.push((url, bypass));
    }
    for (url, bypass) in targets {
        // A connection returns to the pool only after its response (4xx included) is read.
        let _ = match agent(bypass)
            .head(&url)
            .timeout(Duration::from_secs(5))
            .call()
        {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response.into_string(),
            Err(_) => continue,
        };
    }
}

/// Strips a leading `<think>…</think>`. An unclosed block means the output is incomplete, and an
/// empty translation is an error, so reasoning or blanks are never committed.
fn strip_leading_reasoning(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    let answer = match trimmed.strip_prefix("<think>") {
        Some(rest) => rest
            .split_once("</think>")
            .map(|(_, answer)| answer.trim())
            .ok_or("模型输出不完整（推理未结束），请重试")?,
        None => trimmed,
    };
    if answer.is_empty() {
        return Err("译文为空".into());
    }
    Ok(answer.to_owned())
}

fn bypass_proxy(endpoint: &str) -> bool {
    let Ok(url) = url::Url::parse(endpoint) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
    {
        return true;
    }
    ["NO_PROXY", "no_proxy"]
        .into_iter()
        .filter_map(|name| env::var(name).ok())
        .any(|patterns| matches_no_proxy(host, url.port_or_known_default(), &patterns))
}

fn matches_no_proxy(host: &str, port: Option<u16>, patterns: &str) -> bool {
    patterns.split(',').any(|raw| {
        let pattern = raw.trim().to_ascii_lowercase();
        if pattern == "*" {
            return true;
        }
        if let Some((network, bits)) = pattern.split_once('/') {
            return matches_cidr(host, network, bits);
        }
        if let (Ok(wanted), Ok(actual)) = (pattern.parse::<IpAddr>(), host.parse::<IpAddr>()) {
            return wanted == actual;
        }
        let (name, wanted_port) = if pattern.starts_with('[') {
            let Some(end) = pattern.find(']') else {
                return false;
            };
            (
                &pattern[1..end],
                pattern[end + 1..]
                    .strip_prefix(':')
                    .and_then(|p| p.parse::<u16>().ok()),
            )
        } else if let Some((name, port)) = pattern.rsplit_once(':') {
            if let Ok(port) = port.parse::<u16>() {
                (name, Some(port))
            } else {
                (pattern.as_str(), None)
            }
        } else {
            (pattern.as_str(), None)
        };
        if wanted_port.is_some_and(|wanted| Some(wanted) != port) {
            return false;
        }
        let name = name
            .strip_prefix("*.")
            .or_else(|| name.strip_prefix('.'))
            .unwrap_or(name);
        host.eq_ignore_ascii_case(name) || host.to_ascii_lowercase().ends_with(&format!(".{name}"))
    })
}

fn matches_cidr(host: &str, network: &str, bits: &str) -> bool {
    let (Ok(host), Ok(network), Ok(bits)) = (
        host.parse::<IpAddr>(),
        network
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>(),
        bits.parse::<u32>(),
    ) else {
        return false;
    };
    let (host, network, width) = match (host, network) {
        (IpAddr::V4(host), IpAddr::V4(network)) => {
            (u32::from(host) as u128, u32::from(network) as u128, 32)
        }
        (IpAddr::V6(host), IpAddr::V6(network)) => (u128::from(host), u128::from(network), 128),
        _ => return false,
    };
    if bits > width {
        return false;
    }
    let shift = width - bits;
    shift == width || (host >> shift) == (network >> shift)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_endpoint_and_reasoning_cleanup() {
        assert!(bypass_proxy("http://[::1]:8765/v1"));
        assert!(matches_no_proxy(
            "api.corp.local",
            Some(8080),
            ".corp.local,example.com"
        ));
        assert!(!matches_no_proxy(
            "evilcorp.local",
            Some(8080),
            ".corp.local"
        ));
        assert!(matches_no_proxy(
            "api.corp.local",
            Some(8080),
            "api.corp.local:8080"
        ));
        assert!(!matches_no_proxy(
            "api.corp.local",
            Some(8080),
            "api.corp.local:8081"
        ));
        assert!(matches_no_proxy("10.1.2.3", Some(443), "10.0.0.0/8"));
        assert!(!matches_no_proxy("11.1.2.3", Some(443), "10.0.0.0/8"));
        assert!(matches_no_proxy("fd00::1", Some(443), "fd00::/8"));
        assert!(!matches_no_proxy("api.corp.local", Some(443), "10.0.0.0/8"));
        assert_eq!(
            strip_leading_reasoning("<think>draft</think> Hello"),
            Ok("Hello".to_owned())
        );
        assert!(strip_leading_reasoning("<think>I should translate... Hello").is_err());
        assert_eq!(strip_leading_reasoning("   "), Err("译文为空".to_owned()));
        assert!(matches_no_proxy("fd00::1", Some(443), "fd00::1"));
        assert!(matches_no_proxy("::1", Some(80), "::1"));
        assert!(!matches_no_proxy("::2", Some(80), "::1"));
    }

    #[test]
    fn google_free_response_and_log_line() {
        let json: Value = serde_json::from_str(
            r#"[[["Hello, ","你好，",null,null,10],["world","世界",null,null,10]],null,"zh-CN"]"#,
        )
        .unwrap();
        assert_eq!(parse_google_free(&json), Ok("Hello, world".to_owned()));
        assert!(parse_google_free(&Value::Null).is_err());
        let attempt = Attempt {
            engine: Engine::Llm,
            model: "m".into(),
            host: "api.example.com".into(),
            first_byte_ms: Some(820),
            total_ms: 905,
            chars: 12,
            failure: None,
        };
        assert_eq!(
            attempt.log_line(),
            "engine=llm model=m host=api.example.com chars=12 first_byte=820ms total=905ms ok"
        );
    }

    #[test]
    fn google_free_needs_no_settings_and_falls_back_only_when_llm_ready() {
        // No network: only checks the fallback condition.
        let defaults = ApiSettings::default();
        assert_eq!(defaults.engine, Engine::Azure);
        assert!(defaults.validate().is_ok());
        assert!(defaults.validate_llm().is_err());
    }

    /// Local fake OpenAI endpoint: model `picky` rejects temperature, `broken` fails with a 500
    /// whose body echoes the request; others answer `full` or `minimal` by request shape.
    fn fake_llm(requests: usize) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let server = std::thread::spawn(move || {
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().unwrap();
                let mut data = Vec::new();
                let mut chunk = [0; 4096];
                let body = loop {
                    let size = stream.read(&mut chunk).unwrap();
                    assert_ne!(size, 0);
                    data.extend_from_slice(&chunk[..size]);
                    let text = String::from_utf8_lossy(&data).to_string();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let length = head
                            .lines()
                            .find_map(|line| {
                                let lower = line.to_ascii_lowercase();
                                lower
                                    .strip_prefix("content-length:")
                                    .map(|n| n.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if body.len() >= length {
                            break body.to_owned();
                        }
                    }
                };
                let json: Value = serde_json::from_str(&body).unwrap();
                let full = json.get("temperature").is_some();
                let (status, reply) = match json["model"].as_str().unwrap() {
                    "picky" if full => (
                        "400 Bad Request",
                        json!({"error": {"message": "Unsupported parameter: temperature"}}),
                    ),
                    "broken" => (
                        "500 Internal Server Error",
                        json!({"error": {"message": format!("echo: {body}")}}),
                    ),
                    _ => (
                        "200 OK",
                        json!({"choices": [{"message": {
                            "content": if full { "full" } else { "minimal" }
                        }}]}),
                    ),
                };
                let reply = reply.to_string();
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{reply}",
                    reply.len()
                )
                .unwrap();
            }
        });
        (url, server)
    }

    fn ask(url: &str, model: &str) -> Result<String, String> {
        translate_openai_with("你好", url, model, "k", TranslationStyle::default(), "")
    }

    #[test]
    fn reduced_request_is_remembered_per_model() {
        // picky: full (400) + minimal; picky again: minimal; other: full; picky: minimal.
        let (url, server) = fake_llm(5);
        assert_eq!(ask(&url, "picky").as_deref(), Ok("minimal"));
        assert_eq!(ask(&url, "picky").as_deref(), Ok("minimal"));
        assert_eq!(ask(&url, "other").as_deref(), Ok("full"));
        assert_eq!(ask(&url, "picky").as_deref(), Ok("minimal"));
        server.join().unwrap();
        assert!(wants_minimal(&url, "picky"));
        assert!(!wants_minimal(&url, "other"));
        assert!(!wants_minimal("https://elsewhere.example/v1", "picky"));
    }

    #[test]
    fn log_line_never_contains_provider_text() {
        let (url, server) = fake_llm(1);
        let settings = ApiSettings {
            engine: Engine::Llm,
            url: url.clone(),
            model: "broken".into(),
            key: "secret-key".into(),
            ..ApiSettings::default()
        };
        let outcome = translate(&settings, "秘密原文");
        server.join().unwrap();
        assert!(outcome.result.is_err());
        let line = outcome.attempts[0].log_line();
        assert!(line.ends_with("error=http 500"), "{line}");
        assert!(line.contains("chars=4"));
        assert!(!line.contains("秘密") && !line.contains("echo") && !line.contains("secret"));
    }

    #[test]
    fn parses_azure_response() {
        let json = serde_json::json!([{"translations": [{"text": " Hello ", "to": "en"}]}]);
        assert_eq!(super::parse_azure(&json).unwrap(), "Hello");
        assert!(super::parse_azure(&serde_json::json!([])).is_err());
    }
}
