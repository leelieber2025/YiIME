use crate::settings::{ApiSettings, Engine};
use crate::style::{TranslationStyle, system_prompt};
use serde_json::{Value, json};
use std::env;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Timing of each request (used for writing diagnostic logs; excluding key and original text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub engine: Engine,
    /// Large model name; Google is empty.
    pub model: String,
    pub host: String,
    /// Time (in milliseconds, since the request began) when response headers were received.
    pub first_byte_ms: Option<u128>,
    pub total_ms: u128,
    pub chars: usize,
    pub error: Option<String>,
}

/// The result and process of a translation (maybe Google failed first, then the large model).
#[derive(Debug)]
pub struct Outcome {
    pub result: Result<String, String>,
    pub attempts: Vec<Attempt>,
}

impl Attempt {
    /// One line of diagnostic log, such as `engine=llm model=gpt-4o-mini host=api.openai.com
    /// chars=12 first_byte=820ms total=905ms ok`.
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
        let status = match &self.error {
            None => "ok".to_owned(),
            Some(error) => format!("error={}", error.chars().take(160).collect::<String>()),
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

/// Timeout for the free Google interface: it usually returns in 200–400ms, before using the larger
/// model instead.
const GOOGLE_FREE_TIMEOUT: Duration = Duration::from_secs(4);
const GOOGLE_CLOUD_TIMEOUT: Duration = Duration::from_secs(8);
const LLM_TIMEOUT: Duration = Duration::from_secs(20);
const GOOGLE_FREE_URL: &str = "https://translate.googleapis.com/translate_a/single";
const GOOGLE_CLOUD_URL: &str = "https://translation.googleapis.com/language/translate/v2";

/// Shared HTTP client (one with proxy/one without proxy): keep the connection reused, and save DNS,
/// TCP and TLS handshakes from the second time onwards.
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

fn http_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, response) => {
            let explanation = response.into_string().unwrap_or_default();
            let message = serde_json::from_str::<Value>(&explanation)
                .ok()
                .and_then(|json| json["error"]["message"].as_str().map(str::to_owned))
                .unwrap_or_else(|| {
                    // HTML error pages (when Google is throttling) do not fit into the status bar.
                    if explanation.trim_start().starts_with('<') {
                        String::new()
                    } else {
                        explanation.chars().take(300).collect()
                    }
                });
            if code == 429 {
                format!("HTTP 429（请求太频繁，被限流）: {message}")
            } else {
                format!("HTTP {code}: {message}")
            }
        }
        ureq::Error::Transport(transport) => {
            let text = transport.to_string();
            if text.contains("timed out") || text.contains("Timeout") {
                "请求超时".to_owned()
            } else {
                text
            }
        }
    }
}

/// Translate by settings: Use the selected engine first; automatically switch to the large model if
/// Google (free or Cloud) fails or times out and the large model has been configured.
pub fn translate(settings: &ApiSettings, source: &str) -> Outcome {
    let mut attempts = Vec::new();
    let first = run(settings, settings.engine, source, &mut attempts);
    let result = match first {
        Err(error) if settings.engine != Engine::Llm && settings.validate_llm().is_ok() => {
            run(settings, Engine::Llm, source, &mut attempts)
                .map_err(|llm| format!("{error}；改用大模型也失败：{llm}"))
        }
        other => other,
    };
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
        error: result.as_ref().err().cloned(),
    });
    result
}

/// Free Google interface: `[[["dst","src",…],…],…]`, put together the translations.
fn google_free(
    source: &str,
    first_byte: &mut Option<u128>,
    started: Instant,
) -> Result<String, String> {
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
    let json: Value = response.into_json().map_err(|error| error.to_string())?;
    parse_google_free(&json)
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

/// Google Cloud Translation v2 (`?key=`).
fn google_cloud(
    source: &str,
    key: &str,
    first_byte: &mut Option<u128>,
    started: Instant,
) -> Result<String, String> {
    if key.is_empty() {
        return Err("缺少 Google Cloud 翻译的 API 密钥".into());
    }
    let response = agent(false)
        .post(GOOGLE_CLOUD_URL)
        .timeout(GOOGLE_CLOUD_TIMEOUT)
        .query("key", key)
        .send_json(json!({"q": source, "source": "zh-CN", "target": "en", "format": "text"}))
        .map_err(http_error)?;
    *first_byte = Some(started.elapsed().as_millis());
    let json: Value = response.into_json().map_err(|error| error.to_string())?;
    json["data"]["translations"][0]["translatedText"]
        .as_str()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| "Google Cloud 没有返回译文".to_owned())
}

/// When the model does not accept temperature / max_tokens (some inference models will report 400),
/// remember it and never send it again.
static MINIMAL_BODY: AtomicBool = AtomicBool::new(false);

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
) -> Result<String, String> {
    if endpoint.is_empty() || model.is_empty() || key.is_empty() {
        return Err("请填写 API 地址、模型和密钥".into());
    }
    let messages = json!([
        {"role": "system", "content": system_prompt(style, custom)},
        {"role": "user", "content": source}
    ]);
    // The length of the translation is similar to the original text: about 1–2 tokens per Chinese
    // character in English, leaving enough margin but not allowing the model to output long
    // sentences.
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
    let response = if MINIMAL_BODY.load(Ordering::Relaxed) {
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
                    MINIMAL_BODY.store(true, Ordering::Relaxed);
                    send(&minimal)
                } else {
                    let message = serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|json| json["error"]["message"].as_str().map(str::to_owned))
                        .unwrap_or_else(|| text.chars().take(300).collect());
                    return Err(format!("HTTP 400: {message}"));
                }
            }
            other => other,
        }
    }
    .map_err(http_error)?;
    *first_byte = Some(started.elapsed().as_millis());
    let response: Value = response.into_json().map_err(|error| error.to_string())?;
    let choice = &response["choices"][0];
    let content = choice["message"]["content"]
        .as_str()
        .ok_or_else(|| "接口没有返回译文".to_owned())?;
    if choice["finish_reason"].as_str() == Some("length") && content.trim().is_empty() {
        return Err("模型输出被截断（可能是推理模型思考过长），请换用非推理模型".into());
    }
    strip_leading_reasoning(content)
}

/// Establish a connection to the translation server in advance (called when the floating box is
/// opened, and the DNS/TCP/TLS handshake is completed during typing). Only send a HEAD request
/// without the key and original text; failure will not affect subsequent translations. No repeat
/// within 30 seconds.
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
        // The connection will not be put back into the connection pool until the response
        // (including 4xx) is read.
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

/// Remove the `<think>…</think>` at the beginning; if it is not closed, the output will be regarded
/// as incomplete, and an error will be reported for empty translations to avoid submitting the
/// reasoning process or blanks.
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
            error: None,
        };
        assert_eq!(
            attempt.log_line(),
            "engine=llm model=m host=api.example.com chars=12 first_byte=820ms total=905ms ok"
        );
    }

    #[test]
    fn google_free_needs_no_settings_and_falls_back_only_when_llm_ready() {
        // Do not send network requests: only check fallback conditions.
        let defaults = ApiSettings::default();
        assert_eq!(defaults.engine, Engine::GoogleFree);
        assert!(defaults.validate().is_ok());
        assert!(defaults.validate_llm().is_err());
    }
}
