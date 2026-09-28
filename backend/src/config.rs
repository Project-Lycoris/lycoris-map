//! 运行配置。
//!
//! 只从环境变量读取；缺失或非法时返回可读错误，**绝不回显完整连接串或密码**。
//! `DATABASE_URL` / `REDIS_URL` 无默认值，避免误连非测试环境；其余参数有安全默认。
//!
//! [`Config`] 不派生 `Debug`，防止连接串经调试输出泄漏。

use std::env;
use std::net::IpAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use axum::http::{HeaderValue, Uri};
use chrono_tz::Tz;

/// 正式本地默认 HTTP 监听端口：与 Web/App 现有 8080 对齐；Rust 成为本地默认后端后无需改客户端代理。
/// 测试/演练 compose 与 Dockerfile 均显式声明端口，不受本默认值影响。
pub const DEFAULT_SERVER_PORT: u16 = 8080;

/// 默认点位可用性时区（与 Java `app.availability-zone` 一致）。
pub const DEFAULT_AVAILABILITY_ZONE: Tz = chrono_tz::Asia::Shanghai;

/// 默认查询缓存命名空间（Rust 独立，不复用 Java 的 `cache:marker:*`）。
pub const DEFAULT_MARKER_CACHE_NAMESPACE: &str = "lycoris:rust:marker";

/// 默认请求体总上限（8 MiB）。multipart 上传显式覆盖 Axum 默认的 2 MiB
/// （`DefaultBodyLimit`）；图片自身的 5 MiB 校验由上传业务逐块累计判断。
pub const REQUEST_BODY_LIMIT_BYTES: usize = 8 * 1024 * 1024;

/// 默认同时执行的图片 CPU 处理任务数（一个并发许可）。
pub const DEFAULT_MEDIA_CONCURRENCY: u32 = 1;

/// 默认服务连接 PostgreSQL 语句超时（20 秒）；超时受控映射 503，不允许无意关闭。
pub const DEFAULT_DB_STATEMENT_TIMEOUT_MS: u64 = 20_000;
/// 默认服务连接 PostgreSQL 锁等待超时（5 秒）；锁等待超时同样受控映射 503。
pub const DEFAULT_DB_LOCK_TIMEOUT_MS: u64 = 5_000;
/// 数据库超时上限（5 分钟），避免接受荒谬值。
pub const MAX_DB_TIMEOUT_MS: u64 = 300_000;
/// 密码哈希默认并发的 CPU 上限（`available_parallelism` clamp 到 `1..=4`）。
pub const PASSWORD_MAX_CONCURRENCY_DEFAULT_CAP: usize = 4;
/// 显式 `PASSWORD_MAX_CONCURRENCY` 的上限。
pub const MAX_PASSWORD_MAX_CONCURRENCY: usize = 32;

/// 默认会话 Cookie 名。批量并行验收可用 `SESSION_COOKIE_NAME` 覆盖为独立名，
/// 避免与 Java 的 `LYCORIS_SESSION` 混用。
pub const DEFAULT_SESSION_COOKIE_NAME: &str = "LYCORIS_SESSION";
/// 默认会话 Redis 命名空间（与 Java Spring Session 的 `lycoris:session` 隔离）。
pub const DEFAULT_SESSION_NAMESPACE: &str = "lycoris:rust:session:v1";
/// 默认注册限流命名空间。
pub const DEFAULT_RATE_LIMIT_NAMESPACE: &str = "lycoris:rust:ratelimit:v1";
/// 默认会话有效期 30 天。
pub const DEFAULT_SESSION_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// 注册限流默认 5 次 / 600 秒。
pub const DEFAULT_REGISTER_RATE_LIMIT_MAX: u32 = 5;
pub const DEFAULT_REGISTER_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(600);
/// BCrypt 默认工作因子，对齐 Java `BCryptPasswordEncoder` 默认值 10。
pub const DEFAULT_BCRYPT_COST: u32 = 10;
/// 管理员重置用户密码时的默认值（与 Java `admin.default-user-password` 一致）。
pub const DEFAULT_ADMIN_USER_PASSWORD: &str = "Lycoris123!";

/// 会话/ Cookie 时长上限（10 年），避免 TTL 转 i64 时溢出或误配置成天文数字。
pub const MAX_SESSION_TTL_SECONDS: u64 = 10 * 365 * 24 * 60 * 60;
/// 单条 Redis 命令超时上限（10 秒）。
pub const MAX_REDIS_COMMAND_TIMEOUT_SECONDS: u64 = 10;

/// Cookie `SameSite` 策略；解析时大小写不敏感，非法值回落 `Lax`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSitePolicy {
    Lax,
    Strict,
    None,
}

impl SameSitePolicy {
    pub fn to_cookie(self) -> cookie::SameSite {
        match self {
            SameSitePolicy::Lax => cookie::SameSite::Lax,
            SameSitePolicy::Strict => cookie::SameSite::Strict,
            SameSitePolicy::None => cookie::SameSite::None,
        }
    }
}

/// 运行配置。刻意不实现 `Debug`，避免连接串或密码出现在日志中。
#[derive(Clone)]
pub struct Config {
    pub smtp: Option<crate::email_verification::SmtpConfig>,
    pub email_verification_secret: Option<String>,
    pub database_url: String,
    pub redis_url: String,
    pub server_host: IpAddr,
    pub server_port: u16,
    pub upload_dir: PathBuf,
    /// 图片 CPU 处理的并发许可数（必须为正值，默认 1）。
    pub media_max_concurrency: usize,
    /// 密码哈希（BCrypt）并发许可数；默认按 CPU 并行度 clamp `1..=4`，可显式 `1..=32`。
    pub password_max_concurrency: usize,
    pub cors_allowed_origins: Vec<HeaderValue>,
    /// 写请求 Origin 白名单（与 CORS 分开实施）。未配置时回落到 CORS 白名单。
    pub write_allowed_origins: Vec<HeaderValue>,
    pub db_max_connections: u32,
    pub db_acquire_timeout: Duration,
    pub db_max_lifetime: Duration,
    pub db_idle_timeout: Duration,
    /// 服务连接 PostgreSQL 语句超时；只用于服务池，维护池不继承。
    pub db_statement_timeout: Duration,
    /// 服务连接 PostgreSQL 锁等待超时；只用于服务池，维护池不继承。
    pub db_lock_timeout: Duration,
    pub request_timeout: Duration,
    pub availability_zone: Tz,
    pub marker_cache_enabled: bool,
    pub marker_cache_namespace: String,
    // —— 阶段 2：会话 ——
    pub session_cookie_name: String,
    pub session_cookie_secure: bool,
    pub session_cookie_domain: Option<String>,
    pub session_cookie_same_site: SameSitePolicy,
    pub session_cookie_max_age: Duration,
    pub session_ttl: Duration,
    pub session_namespace: String,
    pub rate_limit_namespace: String,
    /// 单条 Redis 命令超时；依赖卡住时受保护操作返回 503，而不是拖到全局请求超时。
    pub redis_command_timeout: Duration,
    // —— 阶段 2：注册限流与代理 ——
    pub trusted_proxies: Vec<IpAddr>,
    pub register_rate_limit_max: u32,
    pub register_rate_limit_window: Duration,
    // —— 阶段 2：密码与管理员 ——
    pub bcrypt_cost: u32,
    pub admin_default_user_password: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("缺少必需的环境变量 {0}")]
    Missing(&'static str),
    #[error("环境变量 {0} 的值非法")]
    Invalid(&'static str),
}

impl Config {
    /// 测试或嵌入式使用的显式构造，不读取环境变量。
    ///
    /// 安全相关字段给出与生产默认一致的值；测试可在构造后按需覆写公开字段
    /// （例如独立 Cookie 名与随机 Redis 命名空间）。
    pub fn new(database_url: impl Into<String>, redis_url: impl Into<String>) -> Self {
        Self {
            smtp: None,
            email_verification_secret: None,
            database_url: database_url.into(),
            redis_url: redis_url.into(),
            server_host: IpAddr::from([127, 0, 0, 1]),
            server_port: DEFAULT_SERVER_PORT,
            upload_dir: PathBuf::from("uploads"),
            media_max_concurrency: DEFAULT_MEDIA_CONCURRENCY as usize,
            password_max_concurrency: default_password_max_concurrency(),
            cors_allowed_origins: Vec::new(),
            write_allowed_origins: Vec::new(),
            db_max_connections: 10,
            db_acquire_timeout: Duration::from_secs(30),
            db_max_lifetime: Duration::from_secs(1800),
            db_idle_timeout: Duration::from_secs(600),
            db_statement_timeout: Duration::from_millis(DEFAULT_DB_STATEMENT_TIMEOUT_MS),
            db_lock_timeout: Duration::from_millis(DEFAULT_DB_LOCK_TIMEOUT_MS),
            request_timeout: Duration::from_secs(30),
            availability_zone: DEFAULT_AVAILABILITY_ZONE,
            marker_cache_enabled: true,
            marker_cache_namespace: DEFAULT_MARKER_CACHE_NAMESPACE.to_string(),
            session_cookie_name: DEFAULT_SESSION_COOKIE_NAME.to_string(),
            session_cookie_secure: false,
            session_cookie_domain: None,
            session_cookie_same_site: SameSitePolicy::Lax,
            session_cookie_max_age: DEFAULT_SESSION_TTL,
            session_ttl: DEFAULT_SESSION_TTL,
            session_namespace: DEFAULT_SESSION_NAMESPACE.to_string(),
            rate_limit_namespace: DEFAULT_RATE_LIMIT_NAMESPACE.to_string(),
            redis_command_timeout: Duration::from_secs(2),
            trusted_proxies: Vec::new(),
            register_rate_limit_max: DEFAULT_REGISTER_RATE_LIMIT_MAX,
            register_rate_limit_window: DEFAULT_REGISTER_RATE_LIMIT_WINDOW,
            bcrypt_cost: DEFAULT_BCRYPT_COST,
            admin_default_user_password: DEFAULT_ADMIN_USER_PASSWORD.to_string(),
        }
    }

    pub fn from_env() -> Result<Self, ConfigError> {
        let smtp = crate::email_verification::SmtpConfig::from_env()?;
        let email_verification_secret = optional("EMAIL_VERIFICATION_SECRET");
        if smtp.is_some()
            && email_verification_secret
                .as_ref()
                .is_none_or(|key| key.len() < 32)
        {
            return Err(ConfigError::Invalid("EMAIL_VERIFICATION_SECRET"));
        }
        let database_url = required("DATABASE_URL")?;
        // 只校验格式；错误信息不携带连接串本身，避免泄露密码。
        sqlx::postgres::PgConnectOptions::from_str(&database_url)
            .map_err(|_| ConfigError::Invalid("DATABASE_URL"))?;

        let redis_url = required("REDIS_URL")?;
        fred::types::config::Config::from_url(&redis_url)
            .map_err(|_| ConfigError::Invalid("REDIS_URL"))?;

        let server_host = match optional("SERVER_HOST") {
            Some(value) => {
                IpAddr::from_str(value.trim()).map_err(|_| ConfigError::Invalid("SERVER_HOST"))?
            }
            None => IpAddr::from([127, 0, 0, 1]),
        };
        let server_port = parse_or("SERVER_PORT", DEFAULT_SERVER_PORT)?;
        let upload_dir = optional("UPLOAD_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("uploads"));
        let media_max_concurrency = non_zero(
            "MEDIA_MAX_CONCURRENCY",
            parse_or("MEDIA_MAX_CONCURRENCY", DEFAULT_MEDIA_CONCURRENCY)?,
        )? as usize;
        let cors_allowed_origins =
            parse_origins(&optional("CORS_ALLOWED_ORIGINS").unwrap_or_default())?;
        let availability_zone = match optional("APP_AVAILABILITY_ZONE") {
            Some(value) => Tz::from_str(value.trim())
                .map_err(|_| ConfigError::Invalid("APP_AVAILABILITY_ZONE"))?,
            None => DEFAULT_AVAILABILITY_ZONE,
        };
        // 写请求来源白名单独立配置；未配置时回落到 CORS 白名单（两者语义不同，见 auth-design）。
        let write_allowed_origins = match optional("WRITE_ALLOWED_ORIGINS") {
            Some(raw) => parse_origins(&raw)?,
            None => cors_allowed_origins.clone(),
        };

        Ok(Self {
            smtp,
            email_verification_secret,
            database_url,
            redis_url,
            server_host,
            server_port,
            upload_dir,
            media_max_concurrency,
            cors_allowed_origins,
            write_allowed_origins,
            // SQLx 连接池 0 连接会导致 panic，必须为正值。
            db_max_connections: non_zero(
                "DB_MAX_CONNECTIONS",
                parse_or("DB_MAX_CONNECTIONS", 10)?,
            )?,
            db_acquire_timeout: seconds("DB_ACQUIRE_TIMEOUT_SECONDS", 30)?,
            db_max_lifetime: seconds("DB_MAX_LIFETIME_SECONDS", 1800)?,
            db_idle_timeout: seconds("DB_IDLE_TIMEOUT_SECONDS", 600)?,
            db_statement_timeout: bounded_millis(
                "DB_STATEMENT_TIMEOUT_MS",
                DEFAULT_DB_STATEMENT_TIMEOUT_MS,
                MAX_DB_TIMEOUT_MS,
            )?,
            db_lock_timeout: bounded_millis(
                "DB_LOCK_TIMEOUT_MS",
                DEFAULT_DB_LOCK_TIMEOUT_MS,
                MAX_DB_TIMEOUT_MS,
            )?,
            request_timeout: seconds("REQUEST_TIMEOUT_SECONDS", 30)?,
            availability_zone,
            marker_cache_enabled: parse_or("MARKER_CACHE_REDIS_ENABLED", true)?,
            marker_cache_namespace: optional("MARKER_CACHE_NAMESPACE")
                .unwrap_or_else(|| DEFAULT_MARKER_CACHE_NAMESPACE.to_string()),
            session_cookie_name: validated_cookie_name(
                &optional("SESSION_COOKIE_NAME")
                    .unwrap_or_else(|| DEFAULT_SESSION_COOKIE_NAME.to_string()),
            )?,
            session_cookie_secure: parse_bool("SESSION_COOKIE_SECURE", false)?,
            session_cookie_domain: optional("SESSION_COOKIE_DOMAIN")
                .map(|value| validated_cookie_domain(&value))
                .transpose()?,
            session_cookie_same_site: parse_same_site(&optional("SESSION_COOKIE_SAME_SITE"))?,
            session_cookie_max_age: bounded_seconds(
                "SESSION_COOKIE_MAX_AGE_SECONDS",
                30 * 24 * 60 * 60,
                MAX_SESSION_TTL_SECONDS,
            )?,
            session_ttl: bounded_seconds(
                "SESSION_TTL_SECONDS",
                30 * 24 * 60 * 60,
                MAX_SESSION_TTL_SECONDS,
            )?,
            session_namespace: non_empty(
                "SESSION_NAMESPACE",
                optional("SESSION_NAMESPACE").unwrap_or_else(|| DEFAULT_SESSION_NAMESPACE.into()),
            )?,
            rate_limit_namespace: non_empty(
                "RATE_LIMIT_NAMESPACE",
                optional("RATE_LIMIT_NAMESPACE")
                    .unwrap_or_else(|| DEFAULT_RATE_LIMIT_NAMESPACE.into()),
            )?,
            redis_command_timeout: bounded_seconds(
                "REDIS_COMMAND_TIMEOUT_SECONDS",
                2,
                MAX_REDIS_COMMAND_TIMEOUT_SECONDS,
            )?,
            trusted_proxies: parse_ip_list(&optional("TRUSTED_PROXIES").unwrap_or_default())?,
            register_rate_limit_max: non_zero(
                "REGISTER_RATE_LIMIT_MAX",
                parse_or("REGISTER_RATE_LIMIT_MAX", DEFAULT_REGISTER_RATE_LIMIT_MAX)?,
            )?,
            register_rate_limit_window: seconds("REGISTER_RATE_LIMIT_WINDOW_SECONDS", 600)?,
            password_max_concurrency: parse_password_max_concurrency("PASSWORD_MAX_CONCURRENCY")?,
            bcrypt_cost: non_zero("BCRYPT_COST", parse_or("BCRYPT_COST", DEFAULT_BCRYPT_COST)?)?,
            admin_default_user_password: optional("ADMIN_DEFAULT_USER_PASSWORD")
                .unwrap_or_else(|| DEFAULT_ADMIN_USER_PASSWORD.to_string()),
        })
    }
}

fn required(key: &'static str) -> Result<String, ConfigError> {
    match env::var(key) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(ConfigError::Missing(key)),
    }
}

fn optional(key: &str) -> Option<String> {
    match env::var(key) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    }
}

fn parse_or<T>(key: &'static str, default: T) -> Result<T, ConfigError>
where
    T: FromStr,
{
    match optional(key) {
        Some(value) => value
            .trim()
            .parse::<T>()
            .map_err(|_| ConfigError::Invalid(key)),
        None => Ok(default),
    }
}

/// 数值必须为正值（拒绝 0）。
fn non_zero(key: &'static str, value: u32) -> Result<u32, ConfigError> {
    if value == 0 {
        Err(ConfigError::Invalid(key))
    } else {
        Ok(value)
    }
}

fn seconds(key: &'static str, default_secs: u64) -> Result<Duration, ConfigError> {
    let secs: u64 = parse_or(key, default_secs)?;
    if secs == 0 {
        return Err(ConfigError::Invalid(key));
    }
    Ok(Duration::from_secs(secs))
}

/// 正且不超过上限的秒数，避免 TTL 溢出或以荒谬值运行。
fn bounded_seconds(
    key: &'static str,
    default_secs: u64,
    max_secs: u64,
) -> Result<Duration, ConfigError> {
    let secs: u64 = parse_or(key, default_secs)?;
    if secs == 0 || secs > max_secs {
        return Err(ConfigError::Invalid(key));
    }
    Ok(Duration::from_secs(secs))
}

/// 正且不超过上限的毫秒数（用于数据库语句/锁超时）。
fn bounded_millis(
    key: &'static str,
    default_ms: u64,
    max_ms: u64,
) -> Result<Duration, ConfigError> {
    parse_bounded_millis(key, optional(key).as_deref(), default_ms, max_ms)
}

/// 纯解析：缺失取默认；0、超上限或解析溢出（含负数/非数字）都返回 `Invalid`，不回显原值。
fn parse_bounded_millis(
    key: &'static str,
    raw: Option<&str>,
    default_ms: u64,
    max_ms: u64,
) -> Result<Duration, ConfigError> {
    let millis: u64 = match raw {
        Some(value) => value
            .trim()
            .parse()
            .map_err(|_| ConfigError::Invalid(key))?,
        None => default_ms,
    };
    if millis == 0 || millis > max_ms {
        return Err(ConfigError::Invalid(key));
    }
    Ok(Duration::from_millis(millis))
}

/// 密码哈希默认并发：可用 CPU 数，缺失时回退上限值，再 clamp 到 `1..=4`。
fn default_password_max_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(PASSWORD_MAX_CONCURRENCY_DEFAULT_CAP)
        .clamp(1, PASSWORD_MAX_CONCURRENCY_DEFAULT_CAP)
}

/// 解析显式 `PASSWORD_MAX_CONCURRENCY`；缺失取默认，显式必须是 `1..=32`。
fn parse_password_max_concurrency(key: &'static str) -> Result<usize, ConfigError> {
    parse_password_max_concurrency_value(key, optional(key).as_deref())
}

/// 纯解析：缺失取默认；0、超上限或解析溢出（含负数/非数字）都返回 `Invalid`，不回显原值。
fn parse_password_max_concurrency_value(
    key: &'static str,
    raw: Option<&str>,
) -> Result<usize, ConfigError> {
    let Some(value) = raw else {
        return Ok(default_password_max_concurrency());
    };
    let parsed: usize = value
        .trim()
        .parse()
        .map_err(|_| ConfigError::Invalid(key))?;
    if parsed == 0 || parsed > MAX_PASSWORD_MAX_CONCURRENCY {
        return Err(ConfigError::Invalid(key));
    }
    Ok(parsed)
}

/// Cookie 名必须是合法的 RFC 6265 token，避免运行期才产生非法响应头。
fn validated_cookie_name(raw: &str) -> Result<String, ConfigError> {
    let key = "SESSION_COOKIE_NAME";
    let name = raw.trim();
    if name.is_empty() || !name.bytes().all(is_cookie_token_byte) {
        return Err(ConfigError::Invalid(key));
    }
    Ok(name.to_string())
}

/// Cookie 域不能包含控制字符或分隔符，避免响应头注入或非法字节。
fn validated_cookie_domain(raw: &str) -> Result<String, ConfigError> {
    let key = "SESSION_COOKIE_DOMAIN";
    let domain = raw.trim();
    if domain.is_empty() {
        return Err(ConfigError::Invalid(key));
    }
    if domain
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || matches!(c, ';' | ',' | '='))
    {
        return Err(ConfigError::Invalid(key));
    }
    Ok(domain.to_string())
}

/// RFC 6265 `cookie-octet`/token 允许的字符集。
fn is_cookie_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// 解析逗号分隔的 CORS 凭据白名单。
///
/// 每项必须是 `http`/`https` 源，且不含路径、查询、片段或用户名密码；
/// 拒绝 `*` 与 `null`。空白名单表示不放行任何跨域来源（明确行为，不是默认放开）。
/// 校验通过后规范化为 `scheme://authority`，与浏览器发送的 `Origin` 对齐。
fn parse_origins(raw: &str) -> Result<Vec<HeaderValue>, ConfigError> {
    let key = "CORS_ALLOWED_ORIGINS";
    let mut origins = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if part == "*" || part.eq_ignore_ascii_case("null") {
            return Err(ConfigError::Invalid(key));
        }
        // `http::Uri` 不承载片段，`#` 会被并入 authority，需显式拒绝。
        if part.contains('#') {
            return Err(ConfigError::Invalid(key));
        }

        let uri: Uri = part.parse().map_err(|_| ConfigError::Invalid(key))?;
        let scheme = uri.scheme_str().ok_or(ConfigError::Invalid(key))?;
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
            return Err(ConfigError::Invalid(key));
        }
        let authority = uri.authority().ok_or(ConfigError::Invalid(key))?;
        // 拒绝 userinfo（用户名/密码）。
        if authority.as_str().contains('@') {
            return Err(ConfigError::Invalid(key));
        }
        // 仅允许根路径（可省略），不得包含路径、查询或片段。
        if let Some(path_and_query) = uri.path_and_query()
            && (path_and_query.path() != "/" || path_and_query.query().is_some())
        {
            return Err(ConfigError::Invalid(key));
        }

        let normalized = format!("{scheme}://{authority}");
        origins.push(HeaderValue::from_str(&normalized).map_err(|_| ConfigError::Invalid(key))?);
    }
    Ok(origins)
}

fn parse_bool(key: &'static str, default: bool) -> Result<bool, ConfigError> {
    match optional(key) {
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(ConfigError::Invalid(key)),
        },
        None => Ok(default),
    }
}

/// `SameSite` 大小写不敏感；非法值回落 `Lax`（与 Java `SessionCookieConfig` 一致）。
fn parse_same_site(raw: &Option<String>) -> Result<SameSitePolicy, ConfigError> {
    let Some(value) = raw else {
        return Ok(SameSitePolicy::Lax);
    };
    Ok(match value.trim().to_ascii_lowercase().as_str() {
        "strict" => SameSitePolicy::Strict,
        "none" => SameSitePolicy::None,
        _ => SameSitePolicy::Lax,
    })
}

/// 逗号分隔的 IP 列表；每项必须是合法 `IpAddr`，拒绝主机名（避免把不可信来源当代理）。
fn parse_ip_list(raw: &str) -> Result<Vec<IpAddr>, ConfigError> {
    let key = "TRUSTED_PROXIES";
    let mut out = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        out.push(IpAddr::from_str(part).map_err(|_| ConfigError::Invalid(key))?);
    }
    Ok(out)
}

fn non_empty(key: &'static str, value: String) -> Result<String, ConfigError> {
    let value = value.trim().to_string();
    if value.is_empty() {
        Err(ConfigError::Invalid(key))
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        Config, ConfigError, DEFAULT_DB_LOCK_TIMEOUT_MS, DEFAULT_DB_STATEMENT_TIMEOUT_MS,
        MAX_DB_TIMEOUT_MS, MAX_PASSWORD_MAX_CONCURRENCY, PASSWORD_MAX_CONCURRENCY_DEFAULT_CAP,
        default_password_max_concurrency, non_zero, parse_bounded_millis, parse_origins,
        parse_password_max_concurrency_value, validated_cookie_domain, validated_cookie_name,
    };

    #[test]
    fn origins_accept_http_and_normalize() {
        let origins = parse_origins("https://app.example.com, http://localhost:3000/").unwrap();
        assert_eq!(origins.len(), 2);
        assert_eq!(origins[0].to_str().unwrap(), "https://app.example.com");
        assert_eq!(origins[1].to_str().unwrap(), "http://localhost:3000");
    }

    #[test]
    fn origins_reject_invalid_forms() {
        for bad in [
            "*",
            "null",
            "NULL",
            "ftp://app.example.com",
            "app.example.com",
            "https://app.example.com/path",
            "https://app.example.com/?q=1",
            "https://user:pass@app.example.com",
            "https://app.example.com#fragment",
        ] {
            assert!(parse_origins(bad).is_err(), "{bad} 应被拒绝");
        }
        assert!(parse_origins("").unwrap().is_empty());
        assert!(parse_origins("  ,  ").unwrap().is_empty());
    }

    #[test]
    fn cookie_name_must_be_rfc6265_token() {
        assert_eq!(
            validated_cookie_name("LYCORIS_SESSION").unwrap(),
            "LYCORIS_SESSION"
        );
        assert_eq!(
            validated_cookie_name(" lycoris-rust_session.v1 ").unwrap(),
            "lycoris-rust_session.v1"
        );
        for bad in ["", "   ", "bad name", "bad;name", "bad\nname", "sess=ion"] {
            assert!(validated_cookie_name(bad).is_err(), "{bad:?} 应被拒绝");
        }
    }

    #[test]
    fn cookie_domain_rejects_control_and_separators() {
        assert_eq!(
            validated_cookie_domain("example.com").unwrap(),
            "example.com"
        );
        for bad in [
            "",
            " ",
            "exa mple.com",
            "example.com;path=/",
            "a,b",
            "a=b",
            "x\ty",
        ] {
            assert!(validated_cookie_domain(bad).is_err(), "{bad:?} 应被拒绝");
        }
    }

    #[test]
    fn zero_db_max_connections_rejected() {
        assert_eq!(non_zero("DB_MAX_CONNECTIONS", 1).unwrap(), 1);
        assert!(matches!(
            non_zero("DB_MAX_CONNECTIONS", 0),
            Err(ConfigError::Invalid("DB_MAX_CONNECTIONS"))
        ));
    }

    #[test]
    fn db_timeouts_accept_bounds_and_reject_zero_or_overflow() {
        // 缺失取默认。
        assert_eq!(
            parse_bounded_millis(
                "DB_STATEMENT_TIMEOUT_MS",
                None,
                DEFAULT_DB_STATEMENT_TIMEOUT_MS,
                MAX_DB_TIMEOUT_MS,
            )
            .unwrap(),
            Duration::from_millis(DEFAULT_DB_STATEMENT_TIMEOUT_MS)
        );
        // 下界与上界都接受。
        assert_eq!(
            parse_bounded_millis(
                "DB_STATEMENT_TIMEOUT_MS",
                Some("1"),
                20_000,
                MAX_DB_TIMEOUT_MS
            )
            .unwrap(),
            Duration::from_millis(1)
        );
        assert_eq!(
            parse_bounded_millis(
                "DB_LOCK_TIMEOUT_MS",
                Some("300000"),
                DEFAULT_DB_LOCK_TIMEOUT_MS,
                MAX_DB_TIMEOUT_MS,
            )
            .unwrap(),
            Duration::from_millis(MAX_DB_TIMEOUT_MS)
        );
        // 0、超上限、溢出（u64 无法容纳）、负数、非数字都拒绝，且不含原值。
        for bad in ["0", "300001", "18446744073709551616", "-1", "abc", "20.5"] {
            let error = parse_bounded_millis(
                "DB_STATEMENT_TIMEOUT_MS",
                Some(bad),
                DEFAULT_DB_STATEMENT_TIMEOUT_MS,
                MAX_DB_TIMEOUT_MS,
            )
            .expect_err(bad);
            assert!(
                matches!(error, ConfigError::Invalid("DB_STATEMENT_TIMEOUT_MS")),
                "{bad} 应报 Invalid"
            );
            assert!(
                !error.to_string().contains(bad),
                "错误信息不得回显原值: {error}"
            );
        }
    }

    #[test]
    fn password_max_concurrency_default_and_bounds() {
        // 默认按 CPU clamp 到 1..=4，两端都健全。
        let default = default_password_max_concurrency();
        assert!((1..=PASSWORD_MAX_CONCURRENCY_DEFAULT_CAP).contains(&default));
        assert_eq!(
            parse_password_max_concurrency_value("PASSWORD_MAX_CONCURRENCY", None).unwrap(),
            default
        );

        // 显式上下界 1 与 32 接受。
        assert_eq!(
            parse_password_max_concurrency_value("PASSWORD_MAX_CONCURRENCY", Some("1")).unwrap(),
            1
        );
        assert_eq!(
            parse_password_max_concurrency_value("PASSWORD_MAX_CONCURRENCY", Some(" 32 ")).unwrap(),
            MAX_PASSWORD_MAX_CONCURRENCY
        );

        // 0、超上限、溢出、负数、非数字都拒绝。
        for bad in ["0", "33", "18446744073709551616", "-1", "many", "1.5"] {
            let error = parse_password_max_concurrency_value("PASSWORD_MAX_CONCURRENCY", Some(bad))
                .expect_err(bad);
            assert!(
                matches!(error, ConfigError::Invalid("PASSWORD_MAX_CONCURRENCY")),
                "{bad} 应报 Invalid"
            );
            assert!(
                !error.to_string().contains(bad),
                "错误信息不得回显原值: {error}"
            );
        }
    }

    #[test]
    fn config_new_matches_env_defaults() {
        let config = Config::new("postgres://u:p@127.0.0.1/lycoris", "redis://127.0.0.1:6379");
        assert_eq!(
            config.db_statement_timeout,
            Duration::from_millis(DEFAULT_DB_STATEMENT_TIMEOUT_MS)
        );
        assert_eq!(
            config.db_lock_timeout,
            Duration::from_millis(DEFAULT_DB_LOCK_TIMEOUT_MS)
        );
        assert_eq!(
            config.password_max_concurrency,
            default_password_max_concurrency()
        );
        assert_eq!(config.media_max_concurrency, 1, "媒体并发语义不变");
    }
}
