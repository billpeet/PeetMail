//! Usage and performance telemetry: traces, metrics and logs over OTLP (HTTP/JSON) to the
//! LGTM stack (Tempo, Mimir, Loki).
//!
//! Privacy is enforced by types, not by care at call sites. Attributes are `(Key, Val)`:
//! `Key` is a closed list, and a `Val` is a `&'static str`, a number, a bool, or a `Safe`
//! string, which only this module's sanitisers (`route_template`, `code`) can make. So a
//! subject line, address or search term can't be attached to a span or metric without
//! changing this file. Log lines are scrubbed (`scrub.rs`) before they're exported; debug
//! lines never leave the PC.
//!
//! On by default with an opt-out (Settings → Privacy). Off, or without an endpoint compiled
//! in (`PEETMAIL_OTLP_ENDPOINT`, plus `PEETMAIL_OTLP_HEADERS` for auth), nothing is set up.
//!
//! Lessons kept from PolyCode's exporter: `service.instance.id` is per process (two
//! processes sharing one made Prometheus see interleaved counters as constant resets); the
//! install and a hashed machine name are separate attributes; byte histograms get byte
//! buckets, set on the instrument itself so no view can catch the wrong instruments.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use opentelemetry::logs::{AnyValue, LogRecord as _, Logger as _, LoggerProvider as _, Severity};
use opentelemetry::metrics::{Counter, Gauge, Histogram};
use opentelemetry::context::FutureExt as _;
use opentelemetry::trace::{Span as _, SpanKind, Status, TraceContextExt, Tracer as _};
use opentelemetry::{global, Context, KeyValue};
use opentelemetry_otlp::{LogExporter, MetricExporter, Protocol, SpanExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::logs::{SdkLogger, SdkLoggerProvider};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use sha2::{Digest, Sha256};

use crate::error::{AppError, Result};
use crate::testenv;

pub const KEY_USAGE_DATA: &str = "usage_data";

// ---------- attributes ----------

/// Every attribute key telemetry may carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Provider,
    Folder,
    Method,
    Route,
    Status,
    Outcome,
    ErrorKind,
    Attempts,
    Pages,
    Items,
    FullResync,
    Op,
    Cached,
    SizeBucket,
    Terms,
    Operators,
    Results,
    Binding,
    View,
    Channel,
    Code,
    Check,
}

impl Key {
    pub fn as_str(self) -> &'static str {
        match self {
            Key::Provider => "peetmail.provider",
            Key::Folder => "peetmail.folder",
            Key::Method => "http.request.method",
            Key::Route => "url.template",
            Key::Status => "http.response.status_code",
            Key::Outcome => "peetmail.outcome",
            Key::ErrorKind => "error.type",
            Key::Attempts => "peetmail.attempts",
            Key::Pages => "peetmail.pages",
            Key::Items => "peetmail.items",
            Key::FullResync => "peetmail.full_resync",
            Key::Op => "peetmail.op",
            Key::Cached => "peetmail.cached",
            Key::SizeBucket => "peetmail.size_bucket",
            Key::Terms => "peetmail.search.terms",
            Key::Operators => "peetmail.search.operators",
            Key::Results => "peetmail.results",
            Key::Binding => "peetmail.binding",
            Key::View => "peetmail.view",
            Key::Channel => "peetmail.channel",
            Key::Code => "peetmail.code",
            Key::Check => "peetmail.health.check",
        }
    }
}

/// A string that went through one of this module's sanitisers. The field is private, so
/// nothing outside can make one from arbitrary text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Safe(String);

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Str(&'static str),
    Int(i64),
    Bool(bool),
    Safe(Safe),
}

impl From<&'static str> for Val {
    fn from(s: &'static str) -> Self {
        Val::Str(s)
    }
}
impl From<bool> for Val {
    fn from(b: bool) -> Self {
        Val::Bool(b)
    }
}
impl From<Safe> for Val {
    fn from(s: Safe) -> Self {
        Val::Safe(s)
    }
}
macro_rules! int_val {
    ($($t:ty),*) => { $(impl From<$t> for Val { fn from(n: $t) -> Self { Val::Int(n as i64) } })* };
}
int_val!(i64, i32, u16, u32, u64, usize);

pub type Attr = (Key, Val);

/// `attr(Key::Pages, 3)`.
pub fn attr(key: Key, val: impl Into<Val>) -> Attr {
    (key, val.into())
}

fn kv(attrs: &[Attr]) -> Vec<KeyValue> {
    attrs
        .iter()
        .map(|(k, v)| match v {
            Val::Str(s) => KeyValue::new(k.as_str(), *s),
            Val::Int(n) => KeyValue::new(k.as_str(), *n),
            Val::Bool(b) => KeyValue::new(k.as_str(), *b),
            Val::Safe(s) => KeyValue::new(k.as_str(), s.0.clone()),
        })
        .collect()
}

/// A URL as its route: the path only, with every ID-like segment replaced by `{id}`.
/// `https://graph.microsoft.com/v1.0/me/messages/AAMk…=/attachments?$top=5` becomes
/// `/v1.0/me/messages/{id}/attachments`. Hosts and query strings are dropped.
pub fn route_template(url: &str) -> Safe {
    let path = url::Url::parse(url).map(|u| u.path().to_string()).unwrap_or_else(|_| url.split(['?', '#']).next().unwrap_or("").to_string());
    let segments: Vec<&str> = path.split('/').map(|seg| if is_route_word(seg) { seg } else { "{id}" }).collect();
    let mut t = segments.join("/");
    t.truncate(160);
    Safe(t)
}

/// Fixed route words (`me`, `mailFolders`, `$value`, `v1.0`) as opposed to IDs, addresses
/// and anything encoded.
fn is_route_word(seg: &str) -> bool {
    static VERSION: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^v\d+(\.\d+)?$").expect("regex"));
    static WORD: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^\$?[A-Za-z][A-Za-z.()]{0,23}$").expect("regex"));
    seg.is_empty() || VERSION.is_match(seg) || WORD.is_match(seg)
}

/// A short machine code (an `AADSTS` number, an OAuth error, a binding id) or `"other"`.
pub fn code(raw: &str) -> Safe {
    static CODE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^[A-Za-z0-9_.:-]{1,48}$").expect("regex"));
    if CODE.is_match(raw) && !raw.contains('@') {
        Safe(raw.to_string())
    } else {
        Safe("other".into())
    }
}

/// Sizes as coarse buckets: an exact size can fingerprint a message.
pub fn size_bucket(bytes: u64) -> &'static str {
    match bytes {
        0..1_000 => "<1KB",
        1_000..10_000 => "1-10KB",
        10_000..100_000 => "10-100KB",
        100_000..1_000_000 => "100KB-1MB",
        1_000_000..10_000_000 => "1-10MB",
        _ => ">10MB",
    }
}

/// What kind of failure, without its text.
pub fn error_attrs(e: &AppError) -> Vec<Attr> {
    match e {
        AppError::Db(_) => vec![attr(Key::ErrorKind, "db")],
        AppError::Http(e) if e.is_timeout() => vec![attr(Key::ErrorKind, "timeout")],
        AppError::Http(e) if e.is_connect() => vec![attr(Key::ErrorKind, "connect")],
        AppError::Http(_) => vec![attr(Key::ErrorKind, "http")],
        AppError::Io(_) => vec![attr(Key::ErrorKind, "io")],
        AppError::Json(_) => vec![attr(Key::ErrorKind, "json")],
        AppError::Graph { status, .. } => vec![attr(Key::ErrorKind, "server"), attr(Key::Status, *status)],
        AppError::Auth(msg) => {
            // The AADSTS number or OAuth error name, when there is one; never the text.
            static AUTH_CODE: LazyLock<regex::Regex> =
                LazyLock::new(|| regex::Regex::new(r"(AADSTS\d+|invalid_grant|invalid_client|interaction_required|consent_required|access_denied)").expect("regex"));
            let mut v = vec![attr(Key::ErrorKind, "auth")];
            if let Some(m) = AUTH_CODE.find(msg) {
                v.push(attr(Key::Code, code(m.as_str())));
            }
            v
        }
        AppError::Other(_) => vec![attr(Key::ErrorKind, "other")],
    }
}

// ---------- setup ----------

static ENABLED: AtomicBool = AtomicBool::new(false);

struct Providers {
    tracer: SdkTracerProvider,
    meter: SdkMeterProvider,
    logs: SdkLoggerProvider,
}

static PROVIDERS: Mutex<Option<Providers>> = Mutex::new(None);
static LOGGER: RwLock<Option<SdkLogger>> = RwLock::new(None);

fn endpoint() -> Option<String> {
    testenv::get("PEETMAIL_OTLP_ENDPOINT")
        .or_else(|| option_env!("PEETMAIL_OTLP_ENDPOINT").map(String::from))
        .map(|e| e.trim().trim_end_matches('/').to_string())
        .filter(|e| !e.is_empty())
}

/// `key=value,key=value` (the OTEL_EXPORTER_OTLP_HEADERS format).
pub fn parse_headers(raw: &str) -> HashMap<String, String> {
    raw.split(',')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            let (k, v) = (k.trim(), v.trim());
            (!k.is_empty() && !v.is_empty()).then(|| (k.to_string(), v.to_string()))
        })
        .collect()
}

fn headers() -> HashMap<String, String> {
    testenv::get("PEETMAIL_OTLP_HEADERS").or_else(|| option_env!("PEETMAIL_OTLP_HEADERS").map(String::from)).map(|h| parse_headers(&h)).unwrap_or_default()
}

/// Whether this build sends usage data at all.
pub fn configured() -> bool {
    endpoint().is_some()
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// First 8 hex chars of SHA-256 of the lowercased host name: tells PCs apart without naming them.
fn machine_id() -> String {
    // Windows sets COMPUTERNAME; Linux has no such variable, so read the kernel's host name.
    let host = std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/proc/sys/kernel/hostname").ok())
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    Sha256::digest(host.as_bytes()).iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// One per process, so two processes from one install (an old one lingering through an
/// update) never report as the same series.
fn instance_id(install_id: &str, machine: &str) -> String {
    let started = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or_default();
    format!("{}-{machine}-{}-{started:x}", &install_id[..install_id.len().min(16)], std::process::id())
}

/// Sets up the exporters, if this build has an endpoint and the user hasn't opted out.
/// Call once, after the database opens (it needs the install ID).
pub fn init(install_id: &str, opted_in: bool) {
    let Some(endpoint) = endpoint() else { return };
    if !opted_in {
        log::info!("usage data is off");
        return;
    }
    match build(&endpoint, install_id) {
        Ok(p) => {
            global::set_tracer_provider(p.tracer.clone());
            global::set_meter_provider(p.meter.clone());
            *LOGGER.write().unwrap_or_else(|e| e.into_inner()) = Some(p.logs.logger("peetmail"));
            *PROVIDERS.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
            ENABLED.store(true, Ordering::SeqCst);
        }
        Err(e) => log::error!("telemetry setup failed: {e}"),
    }
}

fn build(endpoint: &str, install_id: &str) -> std::result::Result<Providers, Box<dyn std::error::Error>> {
    let machine = machine_id();
    let resource = Resource::builder_empty()
        .with_service_name("peetmail")
        .with_attributes([
            KeyValue::new("service.namespace", "peetsoftware"),
            KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
            KeyValue::new("deployment.environment.name", option_env!("PEETMAIL_CHANNEL").filter(|c| !c.is_empty()).unwrap_or("dev")),
            KeyValue::new("service.instance.id", instance_id(install_id, &machine)),
            KeyValue::new("peetmail.install.id", install_id.to_string()),
            KeyValue::new("peetmail.machine.id", machine),
            KeyValue::new("os.type", std::env::consts::OS),
        ])
        .build();
    let headers = headers();
    let timeout = Duration::from_secs(5);
    let url = |signal: &str| format!("{endpoint}/v1/{signal}");
    // The exporters' default client aborts the process on Android (android::tls_client).
    #[cfg(target_os = "android")]
    let client = crate::android::tls_blocking_client(timeout)?;

    let spans = SpanExporter::builder().with_http();
    #[cfg(target_os = "android")]
    let spans = spans.with_http_client(client.clone());
    let spans = spans
        .with_protocol(Protocol::HttpJson)
        .with_endpoint(url("traces"))
        .with_headers(headers.clone())
        .with_timeout(timeout)
        .build()?;
    let metrics = MetricExporter::builder().with_http();
    #[cfg(target_os = "android")]
    let metrics = metrics.with_http_client(client.clone());
    let metrics = metrics
        .with_protocol(Protocol::HttpJson)
        .with_endpoint(url("metrics"))
        .with_headers(headers.clone())
        .with_timeout(timeout)
        .build()?;
    let logs = LogExporter::builder().with_http();
    #[cfg(target_os = "android")]
    let logs = logs.with_http_client(client.clone());
    let logs = logs
        .with_protocol(Protocol::HttpJson)
        .with_endpoint(url("logs"))
        .with_headers(headers)
        .with_timeout(timeout)
        .build()?;

    let interval = testenv::get("PEETMAIL_OTLP_METRIC_INTERVAL_MS").and_then(|v| v.parse().ok()).map(Duration::from_millis).unwrap_or(Duration::from_secs(30));
    Ok(Providers {
        tracer: SdkTracerProvider::builder().with_batch_exporter(spans).with_resource(resource.clone()).build(),
        meter: SdkMeterProvider::builder().with_reader(PeriodicReader::builder(metrics).with_interval(interval).build()).with_resource(resource.clone()).build(),
        logs: SdkLoggerProvider::builder().with_batch_exporter(logs).with_resource(resource).build(),
    })
}

/// Turns usage data off now (spans, metrics and logs stop being recorded). Turning it on
/// takes effect at the next start.
pub fn set_opted_in(on: bool) {
    if !on {
        ENABLED.store(false, Ordering::SeqCst);
    }
}

/// Sends what's buffered and stops the exporters. Called on exit and before an update
/// installs; each exporter gives up after its timeout, so an unreachable endpoint can't
/// hold the app open for long.
pub fn shutdown() {
    ENABLED.store(false, Ordering::SeqCst);
    let Some(p) = PROVIDERS.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    let _ = p.tracer.shutdown();
    let _ = p.meter.shutdown();
    let _ = p.logs.shutdown();
}

/// Exports what's buffered now (tests, and before a crash report flush).
pub fn flush() {
    if let Some(p) = PROVIDERS.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = p.tracer.force_flush();
        let _ = p.meter.force_flush();
        let _ = p.logs.force_flush();
    }
}

// ---------- spans ----------

/// Runs `fut` in a span named `name` (a child of the current one), recording its outcome
/// and duration. The duration also goes to the `<name>.duration` histogram (ms).
pub async fn traced<T>(name: &'static str, attrs: Vec<Attr>, fut: impl Future<Output = Result<T>>) -> Result<T> {
    if !enabled() {
        return fut.await;
    }
    let tracer = global::tracer("peetmail");
    let span = tracer.span_builder(name).with_kind(SpanKind::Internal).with_attributes(kv(&attrs)).start_with_context(&tracer, &Context::current());
    let cx = Context::current_with_span(span);
    let started = Instant::now();
    let result = fut.with_context(cx.clone()).await;
    let span = cx.span();
    let mut metric_attrs = attrs;
    match &result {
        Ok(_) => {
            span.set_attribute(KeyValue::new(Key::Outcome.as_str(), "ok"));
            metric_attrs.push(attr(Key::Outcome, "ok"));
        }
        Err(e) => {
            let ea = error_attrs(e);
            span.set_attributes(kv(&ea));
            span.set_attribute(KeyValue::new(Key::Outcome.as_str(), "error"));
            span.set_status(Status::error(match ea.first() {
                Some((_, Val::Str(s))) => *s,
                _ => "error",
            }));
            metric_attrs.push(attr(Key::Outcome, "error"));
            metric_attrs.extend(ea.into_iter().filter(|(k, _)| *k == Key::ErrorKind));
        }
    }
    span.end();
    record_ms(name, started.elapsed().as_secs_f64() * 1000.0, &metric_attrs);
    result
}

/// Adds attributes to the current span (counts known only part-way through).
pub fn annotate(attrs: &[Attr]) {
    if enabled() {
        Context::current().span().set_attributes(kv(attrs));
    }
}

/// A span for something that already happened (a UI action timed in the webview).
pub fn span_done(name: &'static str, took: Duration, attrs: &[Attr]) {
    if !enabled() {
        return;
    }
    let tracer = global::tracer("peetmail");
    let end = SystemTime::now();
    let mut span = tracer.span_builder(name).with_start_time(end - took).with_attributes(kv(attrs)).start(&tracer);
    span.end_with_timestamp(end);
}

// ---------- metrics ----------

static COUNTERS: LazyLock<Mutex<HashMap<&'static str, Counter<u64>>>> = LazyLock::new(Default::default);
static HISTOGRAMS: LazyLock<Mutex<HashMap<&'static str, Histogram<f64>>>> = LazyLock::new(Default::default);
static GAUGES: LazyLock<Mutex<HashMap<&'static str, Gauge<f64>>>> = LazyLock::new(Default::default);

/// Millisecond buckets from 1 ms to 2 minutes (a first sync can take that long).
const MS_BUCKETS: [f64; 16] = [1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0, 10000.0, 30000.0, 60000.0, 120000.0];

/// Byte buckets doubling from 1 KB to 4 GB.
fn byte_buckets() -> Vec<f64> {
    (0..23).map(|i| 1024.0 * 2f64.powi(i)).collect()
}

pub fn count(name: &'static str, n: u64, attrs: &[Attr]) {
    if !enabled() {
        return;
    }
    let mut map = COUNTERS.lock().unwrap_or_else(|e| e.into_inner());
    map.entry(name).or_insert_with(|| global::meter("peetmail").u64_counter(name).build()).add(n, &kv(attrs));
}

pub fn record_ms(name: &'static str, ms: f64, attrs: &[Attr]) {
    histogram(name, "ms", ms, attrs);
}

pub fn record_bytes(name: &'static str, bytes: u64, attrs: &[Attr]) {
    histogram(name, "By", bytes as f64, attrs);
}

fn histogram(name: &'static str, unit: &'static str, value: f64, attrs: &[Attr]) {
    if !enabled() || !value.is_finite() || value < 0.0 {
        return;
    }
    // Histograms are named `<name>.duration` / `<name>.size`, so a span and its timing share a name.
    let full: &'static str = match unit {
        "ms" => METRIC_NAMES.duration(name),
        _ => METRIC_NAMES.size(name),
    };
    let mut map = HISTOGRAMS.lock().unwrap_or_else(|e| e.into_inner());
    map.entry(full)
        .or_insert_with(|| {
            let meter = global::meter("peetmail");
            let b = meter.f64_histogram(full).with_unit(unit);
            (if unit == "ms" { b.with_boundaries(MS_BUCKETS.to_vec()) } else { b.with_boundaries(byte_buckets()) }).build()
        })
        .record(value, &kv(attrs));
}

pub fn gauge(name: &'static str, unit: &'static str, value: f64, attrs: &[Attr]) {
    if !enabled() || !value.is_finite() {
        return;
    }
    let mut map = GAUGES.lock().unwrap_or_else(|e| e.into_inner());
    map.entry(name).or_insert_with(|| global::meter("peetmail").f64_gauge(name).with_unit(unit).build()).record(value, &kv(attrs));
}

/// `<name>.duration` and `<name>.size` as `&'static str`, made once per name.
struct MetricNames(Mutex<HashMap<(&'static str, &'static str), &'static str>>);
static METRIC_NAMES: LazyLock<MetricNames> = LazyLock::new(|| MetricNames(Mutex::new(HashMap::new())));
impl MetricNames {
    fn get(&self, name: &'static str, suffix: &'static str) -> &'static str {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        m.entry((name, suffix)).or_insert_with(|| Box::leak(format!("{name}.{suffix}").into_boxed_str()))
    }
    fn duration(&self, name: &'static str) -> &'static str {
        self.get(name, "duration")
    }
    fn size(&self, name: &'static str) -> &'static str {
        self.get(name, "size")
    }
}

// ---------- logs ----------

/// Exports a log line (info and above, scrubbed) with the current trace context, so Loki
/// lines link to their traces. Called from the app's logger (crash.rs).
pub fn export_log(record: &log::Record<'_>) {
    if !enabled() || record.level() > log::Level::Info {
        return;
    }
    let guard = LOGGER.read().unwrap_or_else(|e| e.into_inner());
    let Some(logger) = guard.as_ref() else { return };
    let (severity, text) = match record.level() {
        log::Level::Error => (Severity::Error, "ERROR"),
        log::Level::Warn => (Severity::Warn, "WARN"),
        _ => (Severity::Info, "INFO"),
    };
    let mut r = logger.create_log_record();
    r.set_severity_number(severity);
    r.set_severity_text(text);
    r.set_body(AnyValue::String(crate::scrub::scrub(&record.args().to_string()).into()));
    r.set_target(code(record.target()).0);
    logger.emit(r);
}

// ---------- process samples ----------

/// Gauges sampled every minute: outbox depth, body cache and database size, accounts by
/// provider, and this process's memory.
pub fn start_sampling(app: tauri::AppHandle) {
    if !enabled() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            sample(&app);
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

fn sample(app: &tauri::AppHandle) {
    use tauri::Manager;
    let state = app.state::<crate::state::AppState>();
    let read = state.db.with(|c| {
        let outbox: i64 = c.query_row("SELECT COUNT(*) FROM outbox WHERE status IN ('pending', 'inflight')", [], |r| r.get(0))?;
        let bodies: i64 = c.query_row("SELECT COALESCE(SUM(size), 0) FROM message_bodies", [], |r| r.get(0))?;
        let pages: i64 = c.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let page_size: i64 = c.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        let mut stmt = c.prepare("SELECT provider, COUNT(*) FROM accounts GROUP BY provider")?;
        let accounts = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((outbox, bodies, pages * page_size, accounts))
    });
    match read {
        Ok((outbox, bodies, db_bytes, accounts)) => {
            gauge("peetmail.outbox.depth", "{op}", outbox as f64, &[]);
            gauge("peetmail.body_cache.size", "By", bodies as f64, &[]);
            gauge("peetmail.db.size", "By", db_bytes as f64, &[]);
            for (provider, n) in accounts {
                let p = if provider == "google" { "google" } else { "microsoft" };
                gauge("peetmail.accounts", "{account}", n as f64, &[attr(Key::Provider, p)]);
            }
        }
        Err(e) => log::warn!("telemetry sample: {e}"),
    }
    if let Some(bytes) = working_set() {
        gauge("process.memory.usage", "By", bytes as f64, &[]);
    }
}

/// Resident set size: the second field of /proc/self/statm, in pages (Android has it too).
#[cfg(any(target_os = "linux", target_os = "android"))]
fn working_set() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    // SAFETY: sysconf has no preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    resident_bytes(&statm, u64::try_from(page).ok()?)
}

#[cfg_attr(windows, allow(dead_code))]
fn resident_bytes(statm: &str, page_size: u64) -> Option<u64> {
    statm.split_whitespace().nth(1)?.parse::<u64>().ok().map(|pages| pages * page_size)
}

#[cfg(windows)]
fn working_set() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut c: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: GetCurrentProcess is a pseudo-handle; `c` is a correctly sized out-parameter.
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    (ok != 0).then_some(c.WorkingSetSize as u64)
}

// ---------- webview ----------

/// Something the webview timed: a keyboard action until the next paint, a long task, or
/// the first paint. Names and ids are validated here; anything else is dropped.
#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UiEvent {
    pub kind: String,
    pub binding: Option<String>,
    pub view: Option<String>,
    pub ms: f64,
}

pub fn record_ui(events: &[UiEvent]) {
    if !enabled() {
        return;
    }
    for e in events.iter().take(100) {
        if !e.ms.is_finite() || e.ms < 0.0 || e.ms > 600_000.0 {
            continue;
        }
        let view = e.view.as_deref().map(code);
        match e.kind.as_str() {
            "action" => {
                let mut attrs = vec![attr(Key::Binding, code(e.binding.as_deref().unwrap_or("")))];
                if let Some(v) = view {
                    attrs.push(attr(Key::View, v));
                }
                span_done("ui.action", Duration::from_secs_f64(e.ms / 1000.0), &attrs);
                record_ms("ui.action", e.ms, &attrs);
            }
            "longTask" => {
                record_ms("ui.long_task", e.ms, &[]);
                count("ui.long_tasks", 1, &[]);
            }
            "firstPaint" => record_ms("ui.first_paint", e.ms, &[]),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resident_memory_comes_from_statm() {
        assert_eq!(resident_bytes("51234 2048 512 10 0 3000 0\n", 4096), Some(2048 * 4096));
        assert_eq!(resident_bytes("", 4096), None);
    }

    #[test]
    fn routes_keep_words_and_lose_ids() {
        let t = |u: &str| route_template(u).0;
        assert_eq!(t("https://graph.microsoft.com/v1.0/me/messages/AAMkADAwATM3ZmYAZS1hNjAwLTRjNDc=/attachments?$top=5"), "/v1.0/me/messages/{id}/attachments");
        assert_eq!(t("https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages/delta?$deltatoken=abc"), "/v1.0/me/mailFolders/inbox/messages/delta");
        assert_eq!(t("https://graph.microsoft.com/v1.0/me/messages/AAMk%2Fx/$value"), "/v1.0/me/messages/{id}/$value");
        assert_eq!(t("https://gmail.googleapis.com/gmail/v1/users/me/messages/18c2f5a3b4d6e7f8?format=full"), "/gmail/v1/users/me/messages/{id}");
        assert_eq!(t("https://www.googleapis.com/calendar/v3/calendars/boris@gmail.com/events/abc123def"), "/calendar/v3/calendars/{id}/events/{id}");
        assert_eq!(t("http://127.0.0.1:4010/v1.0/me/calendarView/delta"), "/v1.0/me/calendarView/delta");
    }

    #[test]
    fn codes_are_short_identifiers_or_other() {
        assert_eq!(code("AADSTS700016").0, "AADSTS700016");
        assert_eq!(code("list.open").0, "list.open");
        assert_eq!(code("priya@fabrikam.io").0, "other");
        assert_eq!(code("Weekend plans").0, "other");
        assert_eq!(code(&"x".repeat(49)).0, "other");
    }

    #[test]
    fn auth_errors_keep_only_their_code() {
        let a = error_attrs(&AppError::Auth("AADSTS50076: Due to a configuration change made by your administrator (boris@contoso.com)".into()));
        assert_eq!(a, vec![attr(Key::ErrorKind, "auth"), attr(Key::Code, code("AADSTS50076"))]);
        let g = error_attrs(&AppError::Graph { status: 429, message: "Too many requests for priya@x.com".into() });
        assert_eq!(g, vec![attr(Key::ErrorKind, "server"), attr(Key::Status, 429u16)]);
    }

    #[test]
    fn sizes_are_bucketed() {
        assert_eq!(size_bucket(0), "<1KB");
        assert_eq!(size_bucket(45_000), "10-100KB");
        assert_eq!(size_bucket(25_000_000), ">10MB");
    }

    #[test]
    fn headers_parse_like_otel_env() {
        let h = parse_headers("Authorization=Basic abc==, X-Scope-OrgID=peet ,bad,=x");
        assert_eq!(h.get("Authorization").map(String::as_str), Some("Basic abc=="));
        assert_eq!(h.get("X-Scope-OrgID").map(String::as_str), Some("peet"));
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn instance_ids_differ_per_process_but_share_the_install() {
        let id = instance_id("0123456789abcdef0123456789abcdef", "deadbeef");
        assert!(id.starts_with("0123456789abcdef-deadbeef-"), "{id}");
        assert!(id.contains(&format!("-{}-", std::process::id())));
    }

    /// The allowlist only works if nothing builds attributes another way.
    #[test]
    fn only_this_module_builds_otel_attributes() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|n| n == "telemetry.rs") || path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for banned in ["KeyValue::new", "set_attribute(", "opentelemetry::"] {
                assert!(!text.contains(banned), "{} uses {banned}; go through telemetry::attr instead", path.display());
            }
        }
    }
}
