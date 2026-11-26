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
const DEEPL_TIMEOUT: Duration = Duration::from_secs(8);
const DEEPL_FREE_URL: &str = "https://api-free.deepl.com/v2/translate";
const DEEPL_PRO_URL: &str = "https://api.deepl.com/v2/translate";
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

/// Windows reports `ERROR_SEM_TIMEOUT` as "semaphore timeout period", which does not contain
/// the words "timed out".
fn is_timeout(transport: &ureq::Transport) -> bool {
    let rendered = transport.to_string().to_ascii_lowercase();
    if rendered.contains("timed out") || rendered.contains("timeout") {
        return true;
    }
    let mut source = std::error::Error::source(transport);
    while let Some(err) = source {
        if err
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::TimedOut)
        {
            return true;
        }
        source = err.source();
    }
    false
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
                .and_then(|json| {
                    json["error"]["message"]
                        .as_str()
                        .or_else(|| json["message"].as_str())
                        .map(str::to_owned)
                })
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
            } else if code == 456 {
                Fail::new(&category, format!("HTTP 456（翻译额度已用完）{message}"))
            } else {
                Fail::new(&category, format!("HTTP {code}{message}"))
            }
        }
        ureq::Error::Transport(transport) => {
            // `Display` includes the request URL (the free Google query carries the source
            // text), so the user sees only the kind and the URL-free message.
            if is_timeout(&transport) {
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
static CACHE: Mutex<TranslationCache> = Mutex::new(TranslationCache::new());

/// Recent translations. Each entry remembers the service that actually produced it, so a result
/// from a fallback service is not handed out once fallback is turned off (that text came from a
/// service the user no longer allows).
struct TranslationCache {
    entries: Vec<(CacheKey, Engine, String)>,
}

impl TranslationCache {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// A cached result for `key`: one produced by `primary`, or by any service when
    /// `allow_fallback` is on.
    fn get(&self, key: &CacheKey, primary: Engine, allow_fallback: bool) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|(k, provider, _)| k == key && (allow_fallback || *provider == primary))
            .map(|(_, _, text)| text.as_str())
    }

    fn put(&mut self, key: CacheKey, provider: Engine, text: String) {
        self.entries
            .retain(|(k, other, _)| !(k == &key && *other == provider));
        if self.entries.len() >= CACHE_CAP {
            self.entries.remove(0);
        }
        self.entries.push((key, provider, text));
    }
}

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
    if let Err(e) = settings.check_key_access() {
        return Outcome {
            result: Err(e.to_owned()),
            attempts: Vec::new(),
        };
    }
    let key = cache_key(settings, source);
    let engine = match settings.engine {
        Engine::Azure if settings.azure_key.trim().is_empty() => Engine::GoogleFree,
        other => other,
    };
    // Using another service after a failure sends the text somewhere the user did not pick, so
    // it only happens when the setting allows it (off by default).
    let fallback = settings.allow_fallback;
    {
        let cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(text) = cache.get(&key, engine, fallback) {
            return Outcome {
                result: Ok(text.to_owned()),
                attempts: Vec::new(),
            };
        }
    }
    let mut attempts = Vec::new();
    let llm_ok = settings.validate_llm().is_ok();
    let skip_google = fallback && engine == Engine::GoogleFree && llm_ok && google_blocked();
    // Service that produced the result, for the cache.
    let mut provider = engine;
    let result = if skip_google {
        provider = Engine::Llm;
        run(settings, Engine::Llm, source, &mut attempts)
    } else {
        let first = run(settings, engine, source, &mut attempts);
        if engine == Engine::GoogleFree {
            let limited = attempts.last().and_then(|a| a.failure.as_deref()) == Some("http 429");
            set_google_blocked(limited);
        }
        let could_fall_back = (engine != Engine::Llm && llm_ok) || engine == Engine::Azure;
        match first {
            Err(error) if fallback && engine != Engine::Llm && llm_ok => {
                provider = Engine::Llm;
                run(settings, Engine::Llm, source, &mut attempts)
                    .map_err(|llm| format!("{error}；改用大模型也失败：{llm}"))
            }
            Err(error) if fallback && engine == Engine::Azure && !google_blocked() => {
                provider = Engine::GoogleFree;
                let retry = run(settings, Engine::GoogleFree, source, &mut attempts);
                set_google_blocked(
                    attempts.last().and_then(|a| a.failure.as_deref()) == Some("http 429"),
                );
                retry.map_err(|google| format!("{error}；改用 Google 也失败：{google}"))
            }
            Err(error) if !fallback && could_fall_back => Err(format!("{error}{NO_FALLBACK_NOTE}")),
            other => other,
        }
    };
    if let Ok(text) = &result {
        CACHE
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .put(key, provider, text.clone());
    }
    Outcome { result, attempts }
}

/// Appended to errors when another service could have been tried but the setting is off.
pub const NO_FALLBACK_NOTE: &str = "（未改用其他翻译服务；如需失败时自动改用，请在设置中开启）";

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
            google_free(GOOGLE_FREE_URL, source, &mut first_byte, started),
        ),
        Engine::GoogleCloud => (
            host_of(GOOGLE_CLOUD_URL),
            String::new(),
            google_cloud(
                GOOGLE_CLOUD_URL,
                source,
                settings.google_key.trim(),
                &mut first_byte,
                started,
            ),
        ),
        Engine::DeepL => {
            let url = deepl_url(settings.deepl_key.trim());
            (
                host_of(url),
                String::new(),
                deepl(source, settings.deepl_key.trim(), &mut first_byte, started),
            )
        }
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
                LLM_TIMEOUT,
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

/// Longest piece sent to the free Google endpoint in one request. It only takes the text in the
/// URL query, so longer drafts are split at sentence ends (URLs of tens of kilobytes are rejected).
const GOOGLE_FREE_CHUNK: usize = 800;

/// Splits `text` into pieces of at most `limit` characters, preferably after sentence-ending
/// punctuation or a line break.
fn chunks(text: &str, limit: usize) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while rest.chars().count() > limit {
        let mut cut = 0;
        let mut last_break = None;
        for (count, (index, ch)) in rest.char_indices().enumerate() {
            if count >= limit {
                break;
            }
            cut = index + ch.len_utf8();
            if matches!(ch, '。' | '！' | '？' | '；' | '\n' | '.' | '!' | '?' | ';') {
                last_break = Some(cut);
            }
        }
        let cut = last_break.unwrap_or(cut);
        pieces.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    if !rest.is_empty() || pieces.is_empty() {
        pieces.push(rest);
    }
    pieces
}

/// Free Google endpoint, answering `[[["dst","src",…],…],…]`; the pieces are joined. Long text is
/// sent in several requests (see [`GOOGLE_FREE_CHUNK`]).
fn google_free(
    url: &str,
    source: &str,
    first_byte: &mut Option<u128>,
    started: Instant,
) -> Result<String, Fail> {
    let mut out = String::new();
    let mut newline = false;
    for piece in chunks(source, GOOGLE_FREE_CHUNK) {
        if piece.trim().is_empty() {
            newline |= piece.contains('\n');
            continue;
        }
        let response = agent(false)
            .get(url)
            .timeout(GOOGLE_FREE_TIMEOUT)
            .query("client", "gtx")
            .query("sl", "zh-CN")
            .query("tl", "en")
            .query("dt", "t")
            .query("q", piece)
            .call()
            .map_err(http_error)?;
        if first_byte.is_none() {
            *first_byte = Some(started.elapsed().as_millis());
        }
        let json: Value = response
            .into_json()
            .map_err(|_| "Google 翻译返回格式不对")?;
        let text = parse_google_free(&json)?;
        if !out.is_empty() && !out.ends_with(char::is_whitespace) {
            out.push(if newline || piece.starts_with('\n') {
                '\n'
            } else {
                ' '
            });
        }
        out.push_str(&text);
        newline = piece.ends_with('\n');
    }
    if out.is_empty() {
        return Err(Fail::new("empty", "译文为空"));
    }
    Ok(out)
}

fn parse_google_free(json: &Value) -> Result<String, String> {
    let segments = json[0].as_array().ok_or("Google 翻译返回格式不对")?;
    let mut text = String::new();
    for segment in segments {
        if let Some(piece) = segment[0].as_str() {
            text.push_str(piece);
        } else if segment[1].as_str().is_some_and(|src| !src.is_empty()) {
            return Err("Google 翻译返回不完整".into());
        }
    }
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

/// Free-plan keys end with `:fx` and must use the free host; other keys use Pro.
fn deepl_url(key: &str) -> &'static str {
    if key.ends_with(":fx") {
        DEEPL_FREE_URL
    } else {
        DEEPL_PRO_URL
    }
}

/// DeepL `/v2/translate`, Chinese to American English.
fn deepl(
    source: &str,
    key: &str,
    first_byte: &mut Option<u128>,
    started: Instant,
) -> Result<String, Fail> {
    if key.is_empty() {
        return Err(Fail::new("config", "缺少 DeepL 的 API 密钥"));
    }
    let response = agent(false)
        .post(deepl_url(key))
        .timeout(DEEPL_TIMEOUT)
        .set("Authorization", &format!("DeepL-Auth-Key {key}"))
        .send_json(json!({
            "text": [source],
            "source_lang": "ZH",
            "target_lang": "EN-US",
        }))
        .map_err(http_error)?;
    *first_byte = Some(started.elapsed().as_millis());
    let json: Value = response.into_json().map_err(|_| "DeepL 返回格式不对")?;
    parse_deepl(&json).map_err(|message| Fail::new("empty", message))
}

fn parse_deepl(json: &Value) -> Result<String, String> {
    json["translations"][0]["text"]
        .as_str()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| "DeepL 没有返回译文".into())
}

/// Google Cloud Translation v2. The key goes in the `X-goog-api-key` header, not the URL, so it
/// does not end up in proxy or server access logs.
fn google_cloud(
    url: &str,
    source: &str,
    key: &str,
    first_byte: &mut Option<u128>,
    started: Instant,
) -> Result<String, Fail> {
    if key.is_empty() {
        return Err(Fail::new("config", "缺少 Google Cloud 翻译的 API 密钥"));
    }
    let response = agent(false)
        .post(url)
        .timeout(GOOGLE_CLOUD_TIMEOUT)
        .set("X-goog-api-key", key)
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

/// (endpoint, model) pairs that rejected the explicit "no thinking" parameter: sent without it.
static THINKING_PARAM_REJECTED: Mutex<BTreeSet<(String, String)>> = Mutex::new(BTreeSet::new());

fn thinking_param_rejected() -> std::sync::MutexGuard<'static, BTreeSet<(String, String)>> {
    THINKING_PARAM_REJECTED
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// DeepInfra (e.g. `deepseek-ai/DeepSeek-V4.1-Flash-1017`): hybrid reasoning models there take
/// `reasoning_effort: "none"` to answer without a thinking phase (docs.deepinfra.com/chat/reasoning;
/// equivalent to `reasoning: {"enabled": false}`, ignored by non-reasoning models). A translation
/// never needs the thinking phase: it only adds latency, cost and the risk of running out of
/// tokens.
fn disables_thinking(endpoint: &str) -> bool {
    let Ok(url) = url::Url::parse(endpoint.trim()) else {
        return false;
    };
    let deepinfra = url.host_str().is_some_and(|host| {
        let host = host.to_ascii_lowercase();
        host == "deepinfra.com" || host.ends_with(".deepinfra.com")
    });
    // Local fake endpoint in tests.
    deepinfra || (cfg!(test) && url.path().starts_with("/deepinfra-test/"))
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
        LLM_TIMEOUT,
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
    timeout: Duration,
) -> Result<String, Fail> {
    if endpoint.is_empty() || model.is_empty() || key.is_empty() {
        return Err(Fail::new("config", "请填写 API 地址、模型和密钥"));
    }
    // The key goes in a header: never to plain HTTP off this machine, whoever calls this.
    if !crate::settings::llm_url_allowed(endpoint) {
        return Err(Fail::new(
            "config",
            "API 地址请使用 HTTPS（不能带用户名或密码）；仅本机测试允许 HTTP",
        ));
    }
    let messages = json!([
        {"role": "system", "content": system_prompt(style, custom)},
        {"role": "user", "content": source}
    ]);
    // English is about 1–2 tokens per Chinese character: leave headroom without allowing long
    // rambles.
    let max_tokens = (source.chars().count() * 4 + 64).min(2048);
    let mut full = json!({
        "model": model,
        "messages": messages,
        "temperature": 0,
        "max_tokens": max_tokens,
    });
    let mut minimal = json!({"model": model, "messages": messages});
    let pair = (endpoint.to_owned(), model.to_owned());
    if disables_thinking(endpoint) && !thinking_param_rejected().contains(&pair) {
        full["reasoning_effort"] = json!("none");
        minimal["reasoning_effort"] = json!("none");
    }
    let agent = agent(bypass_proxy(endpoint));
    let post = |body: &Value| {
        agent
            .post(endpoint)
            .timeout(timeout)
            .set("Authorization", &format!("Bearer {key}"))
            .send_json(body.clone())
    };
    // An endpoint that refuses the "no thinking" parameter (400/422) is asked again without it,
    // and only that (endpoint, model) pair stops sending it once the retry is accepted.
    let send = |body: &Value| match post(body) {
        Err(ureq::Error::Status(400 | 422, _)) if body.get("reasoning_effort").is_some() => {
            let mut plain = body.clone();
            if let Some(fields) = plain.as_object_mut() {
                fields.remove("reasoning_effort");
            }
            let retry = post(&plain);
            if retry.is_ok() {
                thinking_param_rejected().insert(pair.clone());
            }
            retry
        }
        other => other,
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
    match choice["finish_reason"].as_str() {
        None | Some("stop" | "end_turn" | "eos" | "stop_sequence") => {}
        Some("length" | "model_context_window_exceeded") => {
            return Err(Fail::new(
                "truncated",
                "模型输出被截断（可能是推理模型思考过长），请换用非推理模型",
            ));
        }
        Some("content_filter" | "safety" | "recitation") => {
            return Err(Fail::new(
                "content_filter",
                "模型输出被内容安全策略拦截，已保留原文",
            ));
        }
        Some(_) => {
            return Err(Fail::new("incomplete", "模型输出未正常结束，已保留原文"));
        }
    }
    let content = choice["message"]["content"]
        .as_str()
        .ok_or_else(|| Fail::new("empty", "接口没有返回译文"))?;
    Ok(strip_leading_reasoning(content)?)
}

fn truncated_empty(choice: &Value) -> bool {
    matches!(
        choice["finish_reason"].as_str(),
        Some("length" | "model_context_window_exceeded")
    ) && choice["message"]["content"]
        .as_str()
        .is_none_or(|c| c.trim().is_empty())
}

/// Opens connections to the translation server early (when the input box opens) so DNS/TCP/TLS
/// finish while typing. Sends only a HEAD request without key or text; failures are ignored. At
/// most once per 30 seconds.
pub fn prewarm(settings: &ApiSettings) {
    if settings.check_key_access().is_err() {
        return;
    }
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut last = LAST.lock().unwrap_or_else(|poison| poison.into_inner());
        if last.is_some_and(|at| at.elapsed() < Duration::from_secs(30)) {
            return;
        }
        *last = Some(Instant::now());
    }
    prewarm_urls(&prewarm_targets(settings));
}

fn prewarm_targets(settings: &ApiSettings) -> Vec<(String, bool)> {
    let mut targets = Vec::new();
    match settings.engine {
        Engine::Azure if settings.azure_key.trim().is_empty() => {
            targets.push((GOOGLE_FREE_URL.to_owned(), false))
        }
        Engine::Azure => targets.push((AZURE_URL.to_owned(), false)),
        Engine::GoogleFree => targets.push((GOOGLE_FREE_URL.to_owned(), false)),
        Engine::GoogleCloud => targets.push((GOOGLE_CLOUD_URL.to_owned(), false)),
        Engine::DeepL if !settings.deepl_key.trim().is_empty() => {
            targets.push((deepl_url(settings.deepl_key.trim()).to_owned(), false))
        }
        Engine::DeepL | Engine::Llm => {}
    }
    if (settings.engine == Engine::Llm || settings.allow_fallback)
        && settings.validate_llm().is_ok()
    {
        let url = settings.url.trim().to_owned();
        let bypass = bypass_proxy(&url);
        targets.push((url, bypass));
    }
    targets
}

fn prewarm_urls(targets: &[(String, bool)]) {
    for (url, bypass) in targets {
        // A connection returns to the pool only after its response (4xx included) is read.
        let _ = match agent(*bypass)
            .head(url)
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
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Reads a whole HTTP request (headers and Content-Length body) and returns the body. Test
    /// servers must not reply and close before the request is read: Windows then resets the
    /// connection and the client sees a network error instead of the reply.
    fn read_request(stream: &mut std::net::TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap_or(0);
            data.extend_from_slice(&buf[..n]);
            if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&data[..at]).to_ascii_lowercase();
                let length = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if data.len() >= at + 4 + length {
                    return String::from_utf8_lossy(&data[at + 4..at + 4 + length]).into_owned();
                }
            }
            if n == 0 {
                return String::new();
            }
        }
    }
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
        let deepl: Value = serde_json::from_str(
            r#"{"translations":[{"detected_source_language":"ZH","text":" Hello "}]}"#,
        )
        .unwrap();
        assert_eq!(parse_deepl(&deepl), Ok("Hello".to_owned()));
        assert!(parse_deepl(&json!({"translations":[{"text":"  "}]})).is_err());
        assert_eq!(deepl_url("xxxxxxxx:fx"), DEEPL_FREE_URL);
        assert_eq!(deepl_url("xxxxxxxx"), DEEPL_PRO_URL);
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
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
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
                    "partial" => (
                        "200 OK",
                        json!({"choices":[{"finish_reason":"length","message":{"content":"Half a sentence"}}]}),
                    ),
                    "broken" => (
                        "500 Internal Server Error",
                        json!({"error": {"message": format!("echo: {body}")}}),
                    ),
                    "rate" => (
                        "429 Too Many Requests",
                        json!({"error": {"message": "slow down"}}),
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
    fn nonempty_truncation_is_an_error() {
        let (url, server) = fake_llm(1);
        assert!(ask(&url, "partial").is_err());
        server.join().unwrap();
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
    fn rate_limit_reports_http_status_without_logging_provider_text() {
        let (url, server) = fake_llm(1);
        let settings = ApiSettings {
            engine: Engine::Llm,
            url,
            model: "rate".into(),
            key: "key".into(),
            ..ApiSettings::default()
        };
        let outcome = translate(&settings, "限流测试");
        server.join().unwrap();
        assert!(outcome.result.unwrap_err().contains("HTTP 429"));
        assert_eq!(outcome.attempts[0].failure.as_deref(), Some("http 429"));
        assert!(!outcome.attempts[0].log_line().contains("slow down"));
    }

    #[test]
    fn llm_key_never_goes_to_a_disguised_host() {
        // Even when called directly with an address the settings check would reject (the CLI
        // reads settings.json without the GUI's check), nothing is sent: `localhost:80@` is user
        // info and the real host is the one after it.
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let settings = ApiSettings {
            engine: Engine::Llm,
            url: format!("http://localhost:80@{address}/v1/chat/completions"),
            model: "m".into(),
            key: "secret".into(),
            ..ApiSettings::default()
        };
        assert!(settings.validate_llm().is_err());
        let outcome = translate(&settings, "伪装主机测试");
        let error = outcome.result.unwrap_err();
        assert!(error.contains("HTTPS"), "{error}");
        assert!(matches!(
            listener.accept(),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
        ));
    }

    #[test]
    fn llm_timeout_is_reported_without_waiting_for_production_timeout() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let server = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(400));
        });
        let failure = llm(
            "你好",
            &url,
            "slow",
            "key",
            TranslationStyle::default(),
            "",
            &mut None,
            Instant::now(),
            Duration::from_millis(100),
        )
        .unwrap_err();
        server.join().unwrap();
        assert_eq!(failure.category, "timeout");
    }

    #[test]
    fn failed_primary_engine_falls_back_to_configured_llm_only_when_allowed() {
        // Off by default: the DeepL error is reported and nothing else is contacted.
        let settings = ApiSettings {
            engine: Engine::DeepL,
            deepl_key: String::new(),
            url: "http://127.0.0.1:9/v1/chat/completions".into(),
            model: "fallback".into(),
            key: "key".into(),
            ..ApiSettings::default()
        };
        assert!(!settings.allow_fallback);
        let outcome = translate(&settings, "不回退测试");
        assert_eq!(outcome.attempts.len(), 1);
        assert_eq!(outcome.attempts[0].engine, Engine::DeepL);
        let error = outcome.result.unwrap_err();
        assert!(
            error.contains("DeepL") && error.contains(NO_FALLBACK_NOTE),
            "{error}"
        );
        // Allowed: the configured LLM takes over.
        let (url, server) = fake_llm(1);
        let settings = ApiSettings {
            engine: Engine::DeepL,
            deepl_key: String::new(),
            url,
            model: "fallback".into(),
            key: "key".into(),
            allow_fallback: true,
            ..ApiSettings::default()
        };
        let outcome = translate(&settings, "回退测试");
        server.join().unwrap();
        assert_eq!(outcome.result.as_deref(), Ok("full"));
        assert_eq!(outcome.attempts.len(), 2);
        assert_eq!(outcome.attempts[0].engine, Engine::DeepL);
        assert_eq!(outcome.attempts[1].engine, Engine::Llm);
    }

    /// Local server answering `count` requests with `reply`; returns each raw request.
    fn capture(
        count: usize,
        reply: &'static str,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read, Write};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/translate", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..count {
                let (mut stream, _) = listener.accept().unwrap();
                let mut data = Vec::new();
                let mut chunk = [0; 65536];
                loop {
                    let size = stream.read(&mut chunk).unwrap();
                    data.extend_from_slice(&chunk[..size]);
                    let text = String::from_utf8_lossy(&data).to_string();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let length = head
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|n| n.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if body.len() >= length {
                            break;
                        }
                    }
                    if size == 0 {
                        break;
                    }
                }
                requests.push(String::from_utf8_lossy(&data).into_owned());
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{reply}",
                    reply.len()
                )
                .unwrap();
            }
            requests
        });
        (url, server)
    }

    #[test]
    fn google_cloud_key_goes_in_a_header_not_the_url() {
        let (url, server) = capture(1, r#"{"data":{"translations":[{"translatedText":"Hi"}]}}"#);
        let text = google_cloud(&url, "你好", "cloud-secret", &mut None, Instant::now()).unwrap();
        assert_eq!(text, "Hi");
        let request = server.join().unwrap().remove(0);
        let (head, body) = request.split_once("\r\n\r\n").unwrap();
        let request_line = head.lines().next().unwrap();
        assert!(
            request_line.starts_with("POST /translate "),
            "{request_line}"
        );
        assert!(!request_line.contains("cloud-secret") && !request_line.contains("key="));
        assert!(
            head.lines()
                .any(|line| line.eq_ignore_ascii_case("x-goog-api-key: cloud-secret")),
            "{head}"
        );
        assert!(body.contains("你好") && !body.contains("cloud-secret"));
    }

    #[test]
    fn long_text_for_free_google_is_split_into_requests() {
        let sentence = "这是一个用来测试分段的句子。";
        let long = sentence.repeat(200); // 2,800 characters
        let pieces = chunks(&long, GOOGLE_FREE_CHUNK);
        assert!(pieces.len() >= 4);
        assert!(
            pieces
                .iter()
                .all(|p| p.chars().count() <= GOOGLE_FREE_CHUNK)
        );
        assert!(pieces.iter().all(|p| p.ends_with('。')));
        assert_eq!(pieces.concat(), long);
        assert_eq!(chunks("短句", GOOGLE_FREE_CHUNK), ["短句"]);
        // No punctuation: hard split at the limit.
        let plain = "字".repeat(1700);
        assert_eq!(chunks(&plain, GOOGLE_FREE_CHUNK).len(), 3);
        let (url, server) = capture(
            pieces.len(),
            r#"[[["Part.","x",null,null,10]],null,"zh-CN"]"#,
        );
        let text = google_free(&url, &long, &mut None, Instant::now()).unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), pieces.len());
        assert!(requests.iter().all(|r| r.len() < 16_000));
        assert_eq!(text, vec!["Part."; pieces.len()].join(" "));
    }

    #[test]
    fn parses_azure_response() {
        let json = serde_json::json!([{"translations": [{"text": " Hello ", "to": "en"}]}]);
        assert_eq!(super::parse_azure(&json).unwrap(), "Hello");
        assert!(super::parse_azure(&serde_json::json!([])).is_err());
    }

    #[test]
    fn unknown_engine_makes_zero_external_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let dir = std::env::temp_dir().join(format!("yiime-zero-req-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(crate::SETTINGS_FILE),
            format!(
                r#"{{"engine":"future_engine","allow_fallback":true,"url":"http://127.0.0.1:{port}/v1","model":"m"}}"#
            ),
        )
        .unwrap();
        let settings = ApiSettings::load(&dir, &crate::PlainSealer);
        assert!(settings.unknown_engine);
        let outcome = translate(&settings, "测试不会发出请求");
        assert_eq!(outcome.result, Err(crate::UNKNOWN_ENGINE.to_owned()));
        assert!(outcome.attempts.is_empty());
        assert!(matches!(
            listener.accept(),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn broken_settings_file_never_routes_to_google() {
        let settings = ApiSettings {
            keys_unreadable: true,
            settings_broken: true,
            allow_fallback: true,
            ..Default::default()
        };
        let outcome = translate(&settings, "本地测试");
        assert_eq!(outcome.result, Err(crate::SETTINGS_BROKEN.to_owned()));
        assert!(outcome.attempts.is_empty());
    }

    #[test]
    fn unavailable_azure_keys_never_route_to_google() {
        let settings = ApiSettings {
            engine: Engine::Azure,
            keys_unreadable: true,
            allow_fallback: true,
            ..Default::default()
        };
        let result = translate(&settings, "本地测试");
        assert!(result.result.is_err());
        assert!(result.attempts.is_empty());
    }

    #[test]
    fn cached_fallback_results_need_fallback_allowed() {
        let settings = ApiSettings {
            engine: Engine::GoogleFree,
            ..ApiSettings::default()
        };
        let key = cache_key(&settings, "你好");
        let mut cache = TranslationCache::new();
        // Google failed and the LLM (fallback) answered.
        cache.put(key.clone(), Engine::Llm, "Hello (LLM)".to_owned());
        assert_eq!(
            cache.get(&key, Engine::GoogleFree, true),
            Some("Hello (LLM)")
        );
        // Fallback turned off: only results from the selected service count.
        assert_eq!(cache.get(&key, Engine::GoogleFree, false), None);
        cache.put(key.clone(), Engine::GoogleFree, "Hello".to_owned());
        assert_eq!(cache.get(&key, Engine::GoogleFree, false), Some("Hello"));
        // A different text or style is a different key.
        let other = cache_key(&settings, "再见");
        assert_eq!(cache.get(&other, Engine::GoogleFree, true), None);
    }

    #[test]
    fn translation_cache_is_bounded_and_replaces_entries() {
        let settings = ApiSettings::default();
        let mut cache = TranslationCache::new();
        for i in 0..CACHE_CAP + 10 {
            cache.put(
                cache_key(&settings, &i.to_string()),
                Engine::Llm,
                i.to_string(),
            );
        }
        assert_eq!(cache.entries.len(), CACHE_CAP);
        assert_eq!(
            cache.get(&cache_key(&settings, "0"), Engine::Llm, false),
            None
        );
        let key = cache_key(&settings, "x");
        cache.put(key.clone(), Engine::Llm, "a".to_owned());
        cache.put(key.clone(), Engine::Llm, "b".to_owned());
        assert_eq!(cache.get(&key, Engine::Llm, false), Some("b"));
        assert_eq!(
            cache.entries.iter().filter(|(k, _, _)| *k == key).count(),
            1
        );
    }

    #[test]
    fn google_free_rejects_partial_null_segments_and_recovers_on_retry() {
        let incomplete = serde_json::json!([[["Hello ", "你好"], [null, "世界"]], [null, null]]);
        assert!(parse_google_free(&incomplete).is_err());

        let with_metadata = serde_json::json!([[["Hello world", "你好世界"], [null, null]]]);
        assert_eq!(
            parse_google_free(&with_metadata).as_deref(),
            Ok("Hello world")
        );

        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            for idx in 0..2 {
                let (mut stream, _) = server.accept().unwrap();
                let _ = read_request(&mut stream);
                let body = if idx == 0 {
                    r#"[[["Hello ","你好"],[null,"世界"]]]"#
                } else {
                    r#"[[["Hello world","你好世界"]]]"#
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(resp.as_bytes()).unwrap();
            }
        });

        let cache = TranslationCache::new();
        let settings = ApiSettings {
            engine: Engine::GoogleFree,
            ..ApiSettings::default()
        };
        let url = format!("http://{addr}/translate_a/single");
        let first = agent(true)
            .get(&url)
            .call()
            .unwrap()
            .into_json::<Value>()
            .map_err(|_| "parse".to_owned())
            .and_then(|v| parse_google_free(&v));
        assert!(first.is_err());
        assert_eq!(
            cache.get(&cache_key(&settings, "你好世界"), Engine::GoogleFree, false),
            None
        );

        let second = agent(true)
            .get(&url)
            .call()
            .unwrap()
            .into_json::<Value>()
            .map_err(|_| "parse".to_owned())
            .and_then(|v| parse_google_free(&v));
        assert_eq!(second.as_deref(), Ok("Hello world"));
        handle.join().unwrap();
    }

    #[test]
    fn deepinfra_requests_skip_thinking_and_retry_without_the_parameter_when_refused() {
        assert!(disables_thinking(
            "https://api.deepinfra.com/v1/openai/chat/completions"
        ));
        assert!(!disables_thinking(
            "https://api.openai.com/v1/chat/completions"
        ));
        assert!(!disables_thinking("https://deepinfra.com.evil.example/v1"));

        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        // Model `ok` accepts reasoning_effort; model `strict` refuses it (400).
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..4 {
                let (mut stream, _) = server.accept().unwrap();
                let body = read_request(&mut stream);
                let json: Value = serde_json::from_str(&body).unwrap();
                let effort = json["reasoning_effort"].as_str().map(str::to_owned);
                seen.push((json["model"].as_str().unwrap().to_owned(), effort.clone()));
                let (status, reply) = match (json["model"].as_str().unwrap(), effort) {
                    ("strict", Some(_)) => (
                        "400 Bad Request",
                        r#"{"error":{"message":"unknown field reasoning_effort"}}"#.to_owned(),
                    ),
                    _ => (
                        "200 OK",
                        r#"{"choices":[{"finish_reason":"stop","message":{"content":"Hello"}}]}"#
                            .to_owned(),
                    ),
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                stream.write_all(resp.as_bytes()).unwrap();
            }
            seen
        });
        let url = format!("http://{addr}/deepinfra-test/v1/openai/chat/completions");
        let run = |model: &str| {
            translate_openai_with("你好", &url, model, "k", TranslationStyle::default(), "")
        };
        assert_eq!(run("ok").as_deref(), Ok("Hello"));
        assert_eq!(run("strict").as_deref(), Ok("Hello"));
        // Remembered: the next request for `strict` goes without the parameter at once.
        assert_eq!(run("strict").as_deref(), Ok("Hello"));
        let seen = handle.join().unwrap();
        assert_eq!(
            seen,
            vec![
                ("ok".to_owned(), Some("none".to_owned())),
                ("strict".to_owned(), Some("none".to_owned())),
                ("strict".to_owned(), None),
                ("strict".to_owned(), None),
            ]
        );
    }

    #[test]
    fn llm_rejects_non_normal_finish_reasons_and_allows_normal_ones() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let reasons = [
            "content_filter",
            "tool_calls",
            "model_context_window_exceeded",
            "end_turn",
        ];
        let handle = std::thread::spawn(move || {
            for reason in reasons {
                let (mut stream, _) = server.accept().unwrap();
                let _ = read_request(&mut stream);
                let body = format!(
                    r#"{{"choices":[{{"finish_reason":"{reason}","message":{{"content":"Hello"}}}}]}}"#
                );
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(resp.as_bytes()).unwrap();
            }
        });
        let settings = ApiSettings {
            engine: Engine::Llm,
            url: format!("http://{addr}/v1"),
            model: "test".into(),
            key: "k".into(),
            ..ApiSettings::default()
        };
        assert!(translate(&settings, "一").result.is_err());
        assert!(translate(&settings, "二").result.is_err());
        assert!(translate(&settings, "三").result.is_err());
        assert_eq!(translate(&settings, "四").result.as_deref(), Ok("Hello"));
        handle.join().unwrap();
    }

    #[test]
    fn prewarm_skips_llm_unless_primary_or_fallback_allowed() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_srv = count.clone();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        server.set_nonblocking(true).unwrap();
        let handle = std::thread::spawn(move || {
            while stop_rx.try_recv().is_err() {
                if let Ok((mut stream, _)) = server.accept() {
                    let _ = stream.set_nonblocking(false);
                    let _ = read_request(&mut stream);
                    count_srv.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK
Content-Length: 0
Connection: close

",
                    );
                } else {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        });

        let llm_url = format!("http://{addr}/v1");
        let base = ApiSettings {
            engine: Engine::GoogleFree,
            allow_fallback: false,
            url: llm_url.clone(),
            model: "m".into(),
            key: "k".into(),
            ..ApiSettings::default()
        };
        let targets = prewarm_targets(&base);
        assert!(!targets.iter().any(|(u, _)| u == &llm_url));
        let llm_only: Vec<_> = targets.into_iter().filter(|(u, _)| u == &llm_url).collect();
        prewarm_urls(&llm_only);
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);

        let mut with_fallback = base.clone();
        with_fallback.allow_fallback = true;
        let targets_fb = prewarm_targets(&with_fallback);
        assert!(targets_fb.iter().any(|(u, _)| u == &llm_url));
        let llm_only: Vec<_> = targets_fb
            .into_iter()
            .filter(|(u, _)| u == &llm_url)
            .collect();
        prewarm_urls(&llm_only);
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);

        let mut primary_llm = base;
        primary_llm.engine = Engine::Llm;
        let targets_llm = prewarm_targets(&primary_llm);
        assert_eq!(targets_llm, vec![(llm_url, true)]);
        prewarm_urls(&targets_llm);
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);

        let _ = stop_tx.send(());
        handle.join().unwrap();
    }
}
