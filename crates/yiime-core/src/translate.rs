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
    /// Set when a later model in the saved list produced the translation after a transient failure.
    pub fallback_hint: Option<String>,
    /// Short success line, e.g. `模型 1 · DeepSeek-V4.1-Flash-1017 · 1.8s`.
    pub status: Option<String>,
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

/// The 400/422 names the `reasoning_effort` parameter or its value, so dropping that field and
/// sending the text once more is the right retry. A model-not-found (or any other) 400 is not.
fn reasoning_effort_refused(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("reasoning_effort") || lower.contains("reasoning effort")
}

fn http_fail(code: u16, explanation: &str) -> Fail {
    let category = format!("http {code}");
    let message = serde_json::from_str::<Value>(explanation)
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

/// How long to wait after a 429 before sending the same body again. Integer `Retry-After`
/// seconds are honoured, capped so a translation is not held for a long queue.
fn retry_after_delay(header: Option<&str>) -> Option<Duration> {
    header?.trim().parse::<u64>().ok().map(Duration::from_secs)
}

const BUSY_RETRIES: u32 = 2;
const BUSY_RETRY_AFTER_MAX: Duration = Duration::from_secs(5);

fn busy_backoff(attempt: u32) -> Duration {
    if attempt == 0 {
        Duration::from_millis(500)
    } else {
        Duration::from_secs(2)
    }
}

fn http_error(error: ureq::Error) -> Fail {
    match error {
        ureq::Error::Status(code, response) => {
            let explanation = response.into_string().unwrap_or_default();
            http_fail(code, &explanation)
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
struct CachedTranslation {
    text: String,
    /// Slot that produced an LLM result (`0` = 模型 1). `None` for other services.
    llm_slot: Option<usize>,
}

struct TranslationCache {
    entries: Vec<(CacheKey, Engine, CachedTranslation)>,
}

impl TranslationCache {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// A cached result for `key`: one produced by `primary`, or by any service when
    /// `allow_fallback` is on.
    fn get(
        &self,
        key: &CacheKey,
        primary: Engine,
        allow_fallback: bool,
    ) -> Option<(Engine, &CachedTranslation)> {
        self.entries
            .iter()
            .rev()
            .find(|(k, provider, _)| k == key && (allow_fallback || *provider == primary))
            .map(|(_, provider, hit)| (*provider, hit))
    }

    fn put(&mut self, key: CacheKey, provider: Engine, text: String, llm_slot: Option<usize>) {
        self.entries
            .retain(|(k, other, _)| !(k == &key && *other == provider));
        if self.entries.len() >= CACHE_CAP {
            self.entries.remove(0);
        }
        self.entries
            .push((key, provider, CachedTranslation { text, llm_slot }));
    }
}

fn cache_hit_status(provider: Engine, llm_slot: Option<usize>) -> String {
    if provider == Engine::Llm {
        format!("缓存 · 模型 {}", llm_slot.unwrap_or(0) + 1)
    } else {
        format!("缓存 · {}", engine_status_name(provider))
    }
}

fn cache_key(settings: &ApiSettings, source: &str) -> CacheKey {
    let profiles = settings
        .llm_profiles()
        .into_iter()
        .map(|profile| {
            format!(
                "{} {} {}",
                profile.url.trim(),
                profile.model.trim(),
                profile.disable_thinking as u8
            )
        })
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    (
        settings.engine,
        profiles,
        String::new(),
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
            fallback_hint: None,
            status: None,
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
        if let Some((provider, hit)) = cache.get(&key, engine, fallback) {
            return Outcome {
                result: Ok(hit.text.clone()),
                attempts: Vec::new(),
                fallback_hint: None,
                status: Some(cache_hit_status(provider, hit.llm_slot)),
            };
        }
    }
    let started = Instant::now();
    let mut attempts = Vec::new();
    let mut llm_slot = None;
    let llm_ok = settings.validate_llm().is_ok();
    let skip_google = fallback && engine == Engine::GoogleFree && llm_ok && google_blocked();
    // Service that produced the result, for the cache.
    let mut provider = engine;
    let mut fallback_hint = None;
    let result = if skip_google {
        provider = Engine::Llm;
        let (result, hint, slot) = run_llm(settings, source, &mut attempts);
        fallback_hint = hint;
        llm_slot = slot;
        result
    } else {
        let first = if engine == Engine::Llm {
            let (result, hint, slot) = run_llm(settings, source, &mut attempts);
            fallback_hint = hint;
            llm_slot = slot;
            result
        } else {
            run(settings, engine, source, &mut attempts)
        };
        if engine == Engine::GoogleFree {
            let limited = attempts.last().and_then(|a| a.failure.as_deref()) == Some("http 429");
            set_google_blocked(limited);
        }
        let could_fall_back = (engine != Engine::Llm && llm_ok) || engine == Engine::Azure;
        match first {
            Err(error) if fallback && engine != Engine::Llm && llm_ok => {
                provider = Engine::Llm;
                let (result, hint, slot) = run_llm(settings, source, &mut attempts);
                fallback_hint = hint;
                llm_slot = slot;
                result.map_err(|llm| format!("{error}；改用大模型也失败：{llm}"))
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
        CACHE.lock().unwrap_or_else(|p| p.into_inner()).put(
            key,
            provider,
            text.clone(),
            llm_slot.filter(|_| provider == Engine::Llm),
        );
    }
    let status = result.as_ref().ok().map(|_| {
        success_status(
            provider,
            llm_slot,
            &attempts,
            fallback_hint.as_deref(),
            started.elapsed(),
        )
    });
    Outcome {
        result,
        attempts,
        fallback_hint,
        status,
    }
}

/// Appended to errors when another service could have been tried but the setting is off.
pub const NO_FALLBACK_NOTE: &str = "（未改用其他翻译服务；如需失败时自动改用，请在设置中开启）";

fn is_transient_llm_failure(fail: &Fail) -> bool {
    if fail.category == "timeout" || fail.category == "network" || fail.category == "http 429" {
        return true;
    }
    fail.category
        .strip_prefix("http ")
        .and_then(|code| code.parse::<u16>().ok())
        .is_some_and(|code| (500..600).contains(&code))
}

/// Wall-clock budget for trying the saved model list.
const LLM_TOTAL_BUDGET: Duration = Duration::from_secs(30);
const LLM_MIN_ATTEMPT: Duration = Duration::from_secs(4);

#[cfg(test)]
thread_local! {
    static TEST_LLM_TOTAL_BUDGET: std::cell::Cell<Option<Duration>> =
        const { std::cell::Cell::new(None) };
}

fn llm_total_budget() -> Duration {
    #[cfg(test)]
    {
        if let Some(budget) = TEST_LLM_TOTAL_BUDGET.with(|cell| cell.get()) {
            return budget;
        }
    }
    LLM_TOTAL_BUDGET
}
/// When a later ready profile exists, abandon a silent or busy model after this, not the full
/// 20 s solo timeout — otherwise a hanging primary (e.g. DeepInfra 1017 at ~16–20 s) blocks
/// a healthy backup.
const LLM_FALLBACK_ATTEMPT: Duration = Duration::from_secs(5);

/// Last path segment of a model id (`deepseek-ai/DeepSeek-V4.1-Flash-1017` → `DeepSeek-V4.1-Flash-1017`).
fn short_model_name(model: &str) -> &str {
    model
        .rsplit('/')
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or(model)
}

fn format_elapsed(elapsed: Duration) -> String {
    format!("{:.1}s", elapsed.as_secs_f64())
}

fn engine_status_name(engine: Engine) -> &'static str {
    match engine {
        Engine::Azure => "Azure",
        Engine::GoogleFree => "Google",
        Engine::GoogleCloud => "Google Cloud",
        Engine::DeepL => "DeepL",
        Engine::Llm => "大模型",
    }
}

/// Status shown after a successful translation: which model answered, and how long the user waited.
fn success_status(
    provider: Engine,
    llm_slot: Option<usize>,
    attempts: &[Attempt],
    fallback_hint: Option<&str>,
    elapsed: Duration,
) -> String {
    let time = format_elapsed(elapsed);
    if provider == Engine::Llm {
        let name = attempts
            .iter()
            .rev()
            .find(|attempt| attempt.failure.is_none() && attempt.engine == Engine::Llm)
            .map(|attempt| short_model_name(&attempt.model).to_owned())
            .unwrap_or_else(|| "大模型".to_owned());
        if let Some(hint) = fallback_hint {
            format!("{hint} · {name} · {time}")
        } else {
            let slot = llm_slot.unwrap_or(0) + 1;
            format!("模型 {slot} · {name} · {time}")
        }
    } else {
        format!("{} · {time}", engine_status_name(provider))
    }
}

fn run_llm(
    settings: &ApiSettings,
    source: &str,
    attempts: &mut Vec<Attempt>,
) -> (Result<String, String>, Option<String>, Option<usize>) {
    let profiles = settings.llm_profiles();
    let deadline = Instant::now() + llm_total_budget();
    let mut last_error: Option<Fail> = None;
    let mut tried = false;
    for (slot, profile) in profiles.iter().enumerate() {
        if !profile.is_ready() {
            continue;
        }
        tried = true;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if last_error.is_some() && remaining < LLM_MIN_ATTEMPT {
            break;
        }
        let has_later = profiles
            .iter()
            .skip(slot + 1)
            .any(crate::LlmProfile::is_ready);
        let cap = if has_later {
            LLM_FALLBACK_ATTEMPT
        } else {
            LLM_TIMEOUT
        };
        let started = Instant::now();
        let mut first_byte = None;
        let result = llm(
            source,
            profile.url.trim(),
            profile.model.trim(),
            profile.key.trim(),
            settings.style,
            &settings.custom,
            &mut first_byte,
            started,
            deadline,
            cap,
            profile.disable_thinking,
            !has_later,
        );
        attempts.push(Attempt {
            engine: Engine::Llm,
            model: profile.model.trim().to_owned(),
            host: host_of(profile.url.trim()),
            first_byte_ms: first_byte,
            total_ms: started.elapsed().as_millis(),
            chars: source.chars().count(),
            failure: result.as_ref().err().map(|fail| fail.category.clone()),
        });
        match result {
            Ok(text) => {
                let hint = last_error
                    .is_some()
                    .then(|| crate::settings::llm_backup_hint(slot))
                    .flatten();
                return (Ok(text), hint, Some(slot));
            }
            Err(fail) if is_transient_llm_failure(&fail) => {
                last_error = Some(fail);
            }
            Err(fail) => return (Err(fail.message), None, None),
        }
    }
    if !tried {
        let message = settings
            .llm_profiles()
            .iter()
            .find_map(|profile| {
                if profile.is_empty() {
                    None
                } else {
                    profile.validate().err().map(str::to_owned)
                }
            })
            .unwrap_or_else(|| "请先在“设置”中填写 API 地址、模型和密钥".to_owned());
        return (Err(message), None, None);
    }
    (
        Err(last_error
            .map(|fail| fail.message)
            .unwrap_or_else(|| "翻译失败".to_owned())),
        None,
        None,
    )
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
                started + LLM_TIMEOUT,
                LLM_TIMEOUT,
                settings.disable_thinking,
                true,
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
    crate::settings::default_disable_thinking(endpoint)
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
        Instant::now() + LLM_TIMEOUT,
        LLM_TIMEOUT,
        disables_thinking(endpoint),
        true,
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
    deadline: Instant,
    cap: Duration,
    disable_thinking: bool,
    // When false (a later ready profile exists), a 429 is returned immediately so fallback can start.
    busy_retry: bool,
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
    if disable_thinking && !thinking_param_rejected().contains(&pair) {
        full["reasoning_effort"] = json!("none");
        minimal["reasoning_effort"] = json!("none");
    }
    let agent = agent(bypass_proxy(endpoint));
    let post = |body: &Value| {
        let mut attempt = 0;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out").into());
            }
            let timeout = cap.min(remaining);
            match agent
                .post(endpoint)
                .timeout(timeout)
                .set("Authorization", &format!("Bearer {key}"))
                .send_json(body.clone())
            {
                Err(ureq::Error::Status(429, response)) if busy_retry && attempt < BUSY_RETRIES => {
                    let delay = retry_after_delay(response.header("retry-after"))
                        .unwrap_or_else(|| busy_backoff(attempt))
                        .min(BUSY_RETRY_AFTER_MAX);
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if delay >= remaining {
                        return Err(ureq::Error::Status(429, response));
                    }
                    let _ = response.into_string();
                    if !delay.is_zero() {
                        std::thread::sleep(delay);
                    }
                    attempt += 1;
                }
                other => return other,
            }
        }
    };
    // An endpoint that refuses the "no thinking" parameter is asked again without it, and only
    // that (endpoint, model) pair stops sending it once the retry is accepted. Any other 400/422
    // (a missing model, for example) is returned as-is so the original text is not sent twice.
    let send = |body: &Value| -> Result<ureq::Response, Fail> {
        match post(body) {
            Ok(response) => Ok(response),
            Err(ureq::Error::Status(code @ (400 | 422), response))
                if body.get("reasoning_effort").is_some() =>
            {
                let text = response.into_string().unwrap_or_default();
                if reasoning_effort_refused(&text) {
                    let mut plain = body.clone();
                    if let Some(fields) = plain.as_object_mut() {
                        fields.remove("reasoning_effort");
                    }
                    let retry = post(&plain);
                    if retry.is_ok() {
                        thinking_param_rejected().insert(pair.clone());
                    }
                    retry.map_err(http_error)
                } else {
                    Err(http_fail(code, &text))
                }
            }
            Err(error) => Err(http_error(error)),
        }
    };
    let response = if wants_minimal(endpoint, model) {
        send(&minimal)?
    } else {
        match send(&full) {
            Err(fail) if fail.category == "http 400" => {
                let lower = fail.message.to_ascii_lowercase();
                if lower.contains("temperature")
                    || lower.contains("max_tokens")
                    || (lower.contains("unsupported") && !reasoning_effort_refused(&lower))
                {
                    // Remember only when the reduced request works: the 400 may be about
                    // something else (e.g. an unsupported `system` role or model).
                    let retry = send(&minimal);
                    if retry.is_ok() {
                        mark_minimal(endpoint, model);
                    }
                    retry?
                } else {
                    return Err(fail);
                }
            }
            other => other?,
        }
    };
    *first_byte = Some(started.elapsed().as_millis());
    let response: Value = response.into_json().map_err(|_| "接口返回格式不对")?;
    let mut choice = response["choices"][0].clone();
    // Reasoning models may spend the whole max_tokens budget thinking: retry without the cap.
    if truncated_empty(&choice) && !wants_minimal(endpoint, model) {
        let retry: Value = send(&minimal)?
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
    fn read_full_request(stream: &mut std::net::TcpStream) -> String {
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
                    return String::from_utf8_lossy(&data[..at + 4 + length]).into_owned();
                }
            }
            if n == 0 {
                return String::from_utf8_lossy(&data).into_owned();
            }
        }
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let full = read_full_request(stream);
        match full.split_once("\r\n\r\n") {
            Some((_, body)) => body.to_owned(),
            None => String::new(),
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
                // Retry-After: 0 keeps the 429 retry tests from sleeping on the default backoff.
                let retry_after = if status.starts_with("429") {
                    "Retry-After: 0\r\n"
                } else {
                    ""
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{retry_after}Connection: close\r\nContent-Length: {}\r\n\r\n{reply}",
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
        // One initial 429 plus two short retries, all declined.
        let (url, server) = fake_llm(3);
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
    fn retry_after_parses_integer_seconds_and_ignores_http_dates() {
        assert_eq!(retry_after_delay(Some("0")), Some(Duration::from_secs(0)));
        assert_eq!(retry_after_delay(Some(" 2 ")), Some(Duration::from_secs(2)));
        assert_eq!(
            retry_after_delay(Some("Wed, 21 Oct 2015 07:28:00 GMT")),
            None
        );
        assert_eq!(retry_after_delay(None), None);
    }

    #[test]
    fn llm_retries_a_429_and_returns_the_next_success() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let mut seen = 0;
            for _ in 0..2 {
                let (mut stream, _) = server.accept().unwrap();
                let _ = read_request(&mut stream);
                seen += 1;
                let (status, retry, reply) = if seen == 1 {
                    (
                        "429 Too Many Requests",
                        "Retry-After: 0\r\n",
                        r#"{"error":{"message":"Model busy, retry later","code":"engine_overloaded"}}"#,
                    )
                } else {
                    (
                        "200 OK",
                        "",
                        r#"{"choices":[{"finish_reason":"stop","message":{"content":"Hello"}}]}"#,
                    )
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{retry}Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                stream.write_all(resp.as_bytes()).unwrap();
            }
            seen
        });
        let url = format!("http://{addr}/v1/chat/completions");
        assert_eq!(
            translate_openai_with("你好", &url, "m", "k", TranslationStyle::default(), "")
                .as_deref(),
            Ok("Hello")
        );
        assert_eq!(handle.join().unwrap(), 2);
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
            Instant::now() + Duration::from_millis(100),
            Duration::from_millis(100),
            false,
            true,
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
        cache.put(key.clone(), Engine::Llm, "Hello (LLM)".to_owned(), Some(1));
        assert_eq!(
            cache
                .get(&key, Engine::GoogleFree, true)
                .map(|(_, hit)| hit.text.as_str()),
            Some("Hello (LLM)")
        );
        // Fallback turned off: only results from the selected service count.
        assert!(cache.get(&key, Engine::GoogleFree, false).is_none());
        cache.put(key.clone(), Engine::GoogleFree, "Hello".to_owned(), None);
        assert_eq!(
            cache
                .get(&key, Engine::GoogleFree, false)
                .map(|(_, hit)| hit.text.as_str()),
            Some("Hello")
        );
        // A different text or style is a different key.
        let other = cache_key(&settings, "再见");
        assert!(cache.get(&other, Engine::GoogleFree, true).is_none());
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
                Some(0),
            );
        }
        assert_eq!(cache.entries.len(), CACHE_CAP);
        assert!(
            cache
                .get(&cache_key(&settings, "0"), Engine::Llm, false)
                .is_none()
        );
        let key = cache_key(&settings, "x");
        cache.put(key.clone(), Engine::Llm, "a".to_owned(), Some(0));
        cache.put(key.clone(), Engine::Llm, "b".to_owned(), Some(0));
        assert_eq!(
            cache
                .get(&key, Engine::Llm, false)
                .map(|(_, hit)| hit.text.as_str()),
            Some("b")
        );
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
        assert!(
            cache
                .get(&cache_key(&settings, "你好世界"), Engine::GoogleFree, false)
                .is_none()
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
    fn deepinfra_retries_only_when_the_error_names_reasoning_effort() {
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..3 {
                let (mut stream, _) = server.accept().unwrap();
                let body = read_request(&mut stream);
                let json: Value = serde_json::from_str(&body).unwrap();
                let model = json["model"].as_str().unwrap().to_owned();
                let effort = json["reasoning_effort"].as_str().map(str::to_owned);
                seen.push((model.clone(), effort.clone()));
                let (status, reply) = match (model.as_str(), effort.as_deref()) {
                    ("missing", _) => (
                        "400 Bad Request",
                        r#"{"error":{"message":"model not found"}}"#.to_owned(),
                    ),
                    ("strict", Some(_)) => (
                        "400 Bad Request",
                        r#"{"error":{"message":"unsupported value for reasoning_effort"}}"#
                            .to_owned(),
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
        let missing = run("missing").unwrap_err();
        assert!(missing.contains("model not found"), "{missing}");
        assert_eq!(run("strict").as_deref(), Ok("Hello"));
        let seen = handle.join().unwrap();
        assert_eq!(
            seen,
            vec![
                ("missing".to_owned(), Some("none".to_owned())),
                ("strict".to_owned(), Some("none".to_owned())),
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

    fn profile(url: &str, model: &str, key: &str, disable_thinking: bool) -> crate::LlmProfile {
        crate::LlmProfile {
            url: url.to_owned(),
            model: model.to_owned(),
            key: key.to_owned(),
            disable_thinking,
        }
    }

    fn llm_settings(url: &str, model: &str) -> ApiSettings {
        ApiSettings {
            engine: Engine::Llm,
            url: url.to_owned(),
            model: model.to_owned(),
            key: "k".into(),
            disable_thinking: false,
            ..ApiSettings::default()
        }
    }

    /// Serves one JSON response per connection: 429 / 500 / 400 / 200 chosen by the request's model.
    fn model_script_server(
        script: &'static [(&'static str, &'static str, &'static str)],
    ) -> (String, std::thread::JoinHandle<Vec<(String, bool)>>) {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for &(status, reply, _) in script {
                let (mut stream, _) = server.accept().unwrap();
                let body = read_request(&mut stream);
                let json: Value = serde_json::from_str(&body).unwrap();
                let model = json["model"].as_str().unwrap().to_owned();
                let has_effort = json.get("reasoning_effort").is_some();
                seen.push((model, has_effort));
                let retry = if status.starts_with("429") {
                    "Retry-After: 0\r\n"
                } else {
                    ""
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{retry}Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                stream.write_all(resp.as_bytes()).unwrap();
            }
            seen
        });
        (format!("http://{addr}/v1/chat/completions"), handle)
    }

    const BUSY: (&str, &str, &str) = (
        "429 Too Many Requests",
        r#"{"error":{"message":"Model busy, retry later","code":"engine_overloaded"}}"#,
        "",
    );
    const OK: (&str, &str, &str) = (
        "200 OK",
        r#"{"choices":[{"finish_reason":"stop","message":{"content":"Hello"}}]}"#,
        "",
    );
    const BAD: (&str, &str, &str) = (
        "400 Bad Request",
        r#"{"error":{"message":"model not found"}}"#,
        "",
    );
    const FAIL5: (&str, &str, &str) = (
        "503 Service Unavailable",
        r#"{"error":{"message":"unavailable"}}"#,
        "",
    );

    #[test]
    fn disable_thinking_toggle_controls_reasoning_effort() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = server.accept().unwrap();
                let body = read_request(&mut stream);
                let json: Value = serde_json::from_str(&body).unwrap();
                seen.push(
                    json.get("reasoning_effort")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                );
                let reply =
                    r#"{"choices":[{"finish_reason":"stop","message":{"content":"Hello"}}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                stream.write_all(resp.as_bytes()).unwrap();
            }
            seen
        });
        let url = format!("http://{addr}/deepinfra-test/v1/openai/chat/completions");
        let mut on = llm_settings(&url, "m");
        on.disable_thinking = true;
        let mut off = llm_settings(&url, "m");
        off.disable_thinking = false;
        assert_eq!(translate(&on, "你好").result.as_deref(), Ok("Hello"));
        assert_eq!(translate(&off, "你好").result.as_deref(), Ok("Hello"));
        assert_eq!(handle.join().unwrap(), vec![Some("none".to_owned()), None]);
        // Automatic DeepInfra default still applies when the field is omitted in older files,
        // which `default_disable_thinking` already covers; the toggle is what llm() consults.
        assert!(crate::default_disable_thinking(&url));
    }

    #[test]
    fn llm_falls_back_in_saved_order_and_sets_hint() {
        // With a backup configured, a 429 is not retried on the same model.
        let (url, handle) = model_script_server(&[BUSY, OK]);
        let mut settings = llm_settings(&url, "model-a");
        settings.backup_llm = [
            profile(&url, "model-b", "k", false),
            crate::LlmProfile::default(),
        ];
        let outcome = translate(&settings, "你好");
        assert_eq!(outcome.result.as_deref(), Ok("Hello"));
        assert_eq!(outcome.fallback_hint.as_deref(), Some("已切换到备用模型 2"));
        let status = outcome.status.as_deref().unwrap();
        assert!(
            status.starts_with("已切换到备用模型 2 · model-b · ") && status.ends_with("s"),
            "{status}"
        );
        assert_eq!(
            handle.join().unwrap(),
            vec![("model-a".into(), false), ("model-b".into(), false)]
        );
    }

    #[test]
    fn llm_does_not_fall_back_on_http_400() {
        let (url, handle) = model_script_server(&[BAD]);
        let mut settings = llm_settings(&url, "missing");
        settings.backup_llm[0] = profile(&url, "ok", "k", false);
        let outcome = translate(&settings, "你好");
        let error = outcome.result.unwrap_err();
        assert!(error.contains("HTTP 400"), "{error}");
        assert!(outcome.fallback_hint.is_none());
        assert_eq!(handle.join().unwrap(), vec![("missing".into(), false)]);
    }

    #[test]
    fn llm_falls_back_on_5xx() {
        let (url, handle) = model_script_server(&[FAIL5, OK]);
        let mut settings = llm_settings(&url, "down");
        settings.backup_llm[0] = profile(&url, "ok", "k", false);
        let outcome = translate(&settings, "你好");
        assert_eq!(outcome.result.as_deref(), Ok("Hello"));
        assert_eq!(outcome.fallback_hint.as_deref(), Some("已切换到备用模型 2"));
        assert_eq!(
            handle.join().unwrap(),
            vec![("down".into(), false), ("ok".into(), false)]
        );
    }

    #[test]
    fn reordered_profiles_change_fallback_order() {
        let (url, handle) = model_script_server(&[BUSY, OK]);
        let mut settings = llm_settings(&url, "model-a");
        settings.backup_llm[0] = profile(&url, "model-b", "k", false);
        // Saved order becomes B, A: B is busy, A succeeds — hint names saved slot 2.
        let mut profiles = settings.llm_profiles();
        profiles.swap(0, 1);
        settings.set_llm_profiles(profiles);
        assert_eq!(settings.model, "model-b");
        let outcome = translate(&settings, "你好");
        assert_eq!(outcome.result.as_deref(), Ok("Hello"));
        assert_eq!(outcome.fallback_hint.as_deref(), Some("已切换到备用模型 2"));
        assert_eq!(
            handle.join().unwrap(),
            vec![("model-b".into(), false), ("model-a".into(), false)]
        );
    }

    #[test]
    fn all_models_failing_returns_the_last_error_without_a_hint() {
        let (url, handle) = model_script_server(&[BUSY, FAIL5]);
        let mut settings = llm_settings(&url, "a");
        settings.backup_llm[0] = profile(&url, "b", "k", false);
        let outcome = translate(&settings, "你好");
        let error = outcome.result.unwrap_err();
        assert!(error.contains("HTTP 503"), "{error}");
        assert!(outcome.fallback_hint.is_none());
        assert!(outcome.status.is_none());
        assert_eq!(
            handle.join().unwrap(),
            vec![("a".into(), false), ("b".into(), false)]
        );
    }

    #[test]
    fn short_model_name_strips_provider_prefix() {
        assert_eq!(
            short_model_name("deepseek-ai/DeepSeek-V4.1-Flash-1017"),
            "DeepSeek-V4.1-Flash-1017"
        );
        assert_eq!(short_model_name("deepseek-v4-flash"), "deepseek-v4-flash");
        assert_eq!(format_elapsed(Duration::from_millis(1800)), "1.8s");
        assert_eq!(format_elapsed(Duration::from_millis(15900)), "15.9s");
        assert_eq!(
            success_status(
                Engine::Llm,
                Some(0),
                &[Attempt {
                    engine: Engine::Llm,
                    model: "deepseek-ai/DeepSeek-V4.1-Flash-1017".into(),
                    host: "api.deepinfra.com".into(),
                    first_byte_ms: Some(15891),
                    total_ms: 15900,
                    chars: 14,
                    failure: None,
                }],
                None,
                Duration::from_millis(15900),
            ),
            "模型 1 · DeepSeek-V4.1-Flash-1017 · 15.9s"
        );
        assert_eq!(
            success_status(
                Engine::Llm,
                Some(1),
                &[Attempt {
                    engine: Engine::Llm,
                    model: "deepseek-v4-flash".into(),
                    host: "api.deepseek.com".into(),
                    first_byte_ms: Some(1254),
                    total_ms: 1263,
                    chars: 9,
                    failure: None,
                }],
                Some("已切换到备用模型 2"),
                Duration::from_millis(6300),
            ),
            "已切换到备用模型 2 · deepseek-v4-flash · 6.3s"
        );
    }

    #[test]
    fn solo_profile_still_retries_429() {
        let (url, handle) = model_script_server(&[BUSY, BUSY, OK]);
        let settings = llm_settings(&url, "only");
        let outcome = translate(&settings, "你好");
        assert_eq!(outcome.result.as_deref(), Ok("Hello"));
        assert!(outcome.fallback_hint.is_none());
        let status = outcome.status.as_deref().unwrap();
        assert!(
            status.starts_with("模型 1 · only · ") && status.ends_with("s"),
            "{status}"
        );
        assert_eq!(
            handle.join().unwrap(),
            vec![
                ("only".into(), false),
                ("only".into(), false),
                ("only".into(), false),
            ]
        );
    }

    #[test]
    fn hanging_primary_falls_back_before_the_solo_timeout() {
        let primary = TcpListener::bind("127.0.0.1:0").unwrap();
        let primary_url = format!(
            "http://{}/v1/chat/completions",
            primary.local_addr().unwrap()
        );
        let hang = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = primary.accept() {
                // Keep the socket open until the client gives up (fallback timeout).
                let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
                let mut buf = Vec::new();
                let _ = std::io::Read::read_to_end(&mut stream, &mut buf);
            }
        });
        let (backup_url, handle) = model_script_server(&[OK]);
        let mut settings = llm_settings(&primary_url, "slow");
        settings.backup_llm[0] = profile(&backup_url, "fast", "k", false);
        let started = Instant::now();
        let outcome = translate(&settings, "你好");
        let elapsed = started.elapsed();
        assert_eq!(outcome.result.as_deref(), Ok("Hello"));
        assert_eq!(outcome.fallback_hint.as_deref(), Some("已切换到备用模型 2"));
        assert_eq!(outcome.attempts[0].failure.as_deref(), Some("timeout"));
        assert!(
            elapsed < Duration::from_secs(10),
            "fallback waited {elapsed:?}, expected under 10s"
        );
        assert!(
            outcome.attempts[0].total_ms < 10_000,
            "primary attempt {}ms",
            outcome.attempts[0].total_ms
        );
        assert_eq!(handle.join().unwrap(), vec![("fast".into(), false)]);
        hang.join().unwrap();
    }

    fn with_llm_budget<T>(budget: Duration, f: impl FnOnce() -> T) -> T {
        TEST_LLM_TOTAL_BUDGET.with(|cell| cell.set(Some(budget)));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                TEST_LLM_TOTAL_BUDGET.with(|cell| cell.set(None));
            }
        }
        let _reset = Reset;
        f()
    }

    struct XorSealer;
    impl crate::Sealer for XorSealer {
        fn seal(&self, data: &[u8]) -> Option<Vec<u8>> {
            Some(data.iter().map(|b| b ^ 0x5a).collect())
        }
        fn open(&self, data: &[u8]) -> Option<Vec<u8>> {
            Some(data.iter().map(|b| b ^ 0x5a).collect())
        }
    }

    fn auth_of(request: &str) -> Option<String> {
        request.lines().find_map(|line| {
            let line = line.trim();
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.trim().to_owned())
        })
    }

    fn serve_one_ok() -> (String, std::thread::JoinHandle<String>) {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            let request = read_full_request(&mut stream);
            let reply = r#"{"choices":[{"finish_reason":"stop","message":{"content":"Hello"}}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            request
        });
        (format!("http://{addr}/v1/chat/completions"), handle)
    }

    #[test]
    fn concurrent_reorder_does_not_send_the_other_models_key() {
        let (url_b, handle_b) = serve_one_ok();
        // A is only an identity; if a bad merge still put A first, this address fails fast.
        let url_a = format!(
            "http://127.0.0.1:1/yiime-unused-a-{}-{}/v1/chat/completions",
            std::process::id(),
            started_nanos()
        );
        let dir = std::env::temp_dir().join(format!(
            "yiime-key-merge-{}-{}",
            std::process::id(),
            started_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let sealer = XorSealer;
        let mut base = ApiSettings {
            engine: Engine::Llm,
            ..ApiSettings::default()
        };
        base.set_llm_profiles([
            profile(&url_a, "model-a", "test-key-a", false),
            profile(&url_b, "model-b", "test-key-b", false),
            crate::LlmProfile::default(),
        ]);
        base.save(&dir, &sealer).unwrap();

        let mut window1 = base.clone();
        let mut profiles = window1.llm_profiles();
        profiles.swap(0, 1);
        window1.set_llm_profiles(profiles);
        window1.save_merged(Some(&base), &dir, &sealer).unwrap();

        let mut window2 = base.clone();
        window2.key = "test-key-a-new".into();
        let saved = window2.save_merged(Some(&base), &dir, &sealer).unwrap();
        assert_eq!(saved.model, "model-b");
        let same_key = saved.key == "test-key-b";
        assert!(same_key, "profile B was given the other profile's key");

        let outcome = translate(&saved, "你好");
        assert_eq!(outcome.result.as_deref(), Ok("Hello"));
        let request = handle_b.join().unwrap();
        let auth = auth_of(&request);
        let used_b = auth.as_deref() == Some("Bearer test-key-b");
        let used_a = auth.as_deref() == Some("Bearer test-key-a")
            || auth.as_deref() == Some("Bearer test-key-a-new");
        assert!(used_b, "profile B did not receive its own key");
        assert!(!used_a, "profile B received the other profile's key");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_hit_keeps_which_model_answered() {
        let (url1, handle1) = model_script_server(&[BUSY]);
        let (url2, handle2) = model_script_server(&[OK]);
        let mut settings = ApiSettings {
            engine: Engine::Llm,
            ..ApiSettings::default()
        };
        settings.set_llm_profiles([
            profile(&url1, "model-one", "test-key-1", false),
            profile(&url2, "model-two", "test-key-2", false),
            crate::LlmProfile::default(),
        ]);
        let first = translate(&settings, "同一段待译");
        assert_eq!(first.result.as_deref(), Ok("Hello"));
        assert_eq!(first.fallback_hint.as_deref(), Some("已切换到备用模型 2"));
        assert_eq!(handle1.join().unwrap().len(), 1);
        assert_eq!(handle2.join().unwrap().len(), 1);

        let second = translate(&settings, "同一段待译");
        assert_eq!(second.result.as_deref(), Ok("Hello"));
        assert_eq!(second.status.as_deref(), Some("缓存 · 模型 2"));
        assert!(second.attempts.is_empty());
        assert!(second.fallback_hint.is_none());
    }

    fn started_nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }

    #[test]
    fn llm_retries_stop_when_the_total_budget_runs_out() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        server.set_nonblocking(true).unwrap();
        let addr = server.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen_thread = seen.clone();
        let handle = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match server.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = read_request(&mut stream);
                        seen_thread.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let reply = r#"{"error":{"message":"busy","code":"engine_overloaded"}}"#;
                        let resp = format!(
                            "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nRetry-After: 5\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                            reply.len()
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        let url = format!("http://{addr}/v1/chat/completions");
        let settings = llm_settings(&url, "only");
        let started = Instant::now();
        let outcome = with_llm_budget(Duration::from_millis(600), || translate(&settings, "你好"));
        let elapsed = started.elapsed();
        assert!(outcome.result.is_err());
        assert!(
            elapsed < Duration::from_secs(2),
            "translation ran past the total budget: {elapsed:?}"
        );
        handle.join().unwrap();
        let seen = seen.load(std::sync::atomic::Ordering::Relaxed);
        assert!(
            seen <= 2,
            "kept retrying after the budget ended ({seen} requests)"
        );
    }
}
