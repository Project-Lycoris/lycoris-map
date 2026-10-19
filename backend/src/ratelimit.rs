//! 速率限制：注册按客户端 IP，登录按“IP+账号”和“IP”双计数，均使用 Redis 原子计数。
//!
//! 注册与 Java 一致，默认按客户端 IP 计数，默认 5 次 / 600 秒。计数用 Lua
//! `INCR` + 首次 `PEXPIRE` 原子完成；Redis 故障时返回 [`RateLimitOutcome::Unavailable`]，
//! 绝不回退到内存无上限放行。
//!
//! 登录采用“原子预占”：校验密码前在一个 Lua 中读取两个计数器，任一已达上限即拒绝；
//! 否则两个都自增，占用本次尝试的名额（并发突发也无法越过上限）。凭据错误保持预占，
//! 成功登录则删除“IP+账号”计数并把 IP 计数减一，因此成功不消耗额度。

use std::net::IpAddr;
use std::time::Duration;

use fred::clients::Client;
use fred::prelude::*;
use sha2::{Digest, Sha256};

/// 注册计数：ARGV `[window_ms]`；返回 `{自增后的计数, 剩余 TTL(ms)}`。
const INCR_SCRIPT: &str = r#"
local count = redis.call('INCR', KEYS[1])
if count == 1 then
  redis.call('PEXPIRE', KEYS[1], ARGV[1])
end
return {count, redis.call('PTTL', KEYS[1])}
"#;

/// 登录原子预占：KEYS `[acct, ip]`，ARGV `[max_acct, max_ip, window_ms]`；
/// 返回 `{是否触发, 剩余 TTL(ms)}`。任一已达上限则只读返回、不自增；否则两个都 `INCR`
/// （首次自增时 `PEXPIRE`）并返回允许。这样每个进行中的尝试都先占一个名额，杜绝突发绕过。
const LOGIN_ACQUIRE_SCRIPT: &str = r#"
local acct = tonumber(redis.call('GET', KEYS[1]) or '0')
local ip = tonumber(redis.call('GET', KEYS[2]) or '0')
local max_acct = tonumber(ARGV[1])
local max_ip = tonumber(ARGV[2])
if acct >= max_acct or ip >= max_ip then
  local ttl = 0
  if acct >= max_acct then ttl = redis.call('PTTL', KEYS[1]) end
  if ip >= max_ip then
    local t = redis.call('PTTL', KEYS[2])
    if t > ttl then ttl = t end
  end
  return {1, ttl}
end
local a = redis.call('INCR', KEYS[1])
if a == 1 then redis.call('PEXPIRE', KEYS[1], ARGV[3]) end
local b = redis.call('INCR', KEYS[2])
if b == 1 then redis.call('PEXPIRE', KEYS[2], ARGV[3]) end
return {0, 0}
"#;

/// 成功登录后的额度归还不消耗额度：KEYS `[acct, ip]`；删除“IP+账号”计数，
/// IP 计数在大于 0 时原子 `DECR`（不把不存在的 key 变成 -1，也不清除 TTL）。
const LOGIN_RELEASE_SCRIPT: &str = r#"
redis.call('DEL', KEYS[1])
local current = tonumber(redis.call('GET', KEYS[2]) or '0')
if current > 0 then
  redis.call('DECR', KEYS[2])
end
return 1
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitOutcome {
    Allowed,
    /// 达到上限；`retry_after_seconds` 为触发上限计数器的剩余 TTL（向上取整，至少 1 秒）。
    Limited {
        retry_after_seconds: i64,
    },
    Unavailable,
}

/// 由毫秒 TTL 换算剩余秒数：向上取整，至少 1。
fn retry_after_seconds(ttl_ms: i64) -> i64 {
    (ttl_ms.max(1) + 999) / 1000
}

/// 规范化账号标识（trim + 小写）后取 sha256 前 16 位十六进制，避免明文标识写入 Redis key。
pub fn identity_key(identity: &str) -> String {
    let normalized = identity.trim().to_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    let mut hex = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[derive(Clone)]
pub struct RegisterRateLimiter {
    redis: Client,
    namespace: String,
    max_attempts: i64,
    window_ms: i64,
    command_timeout: Duration,
}

impl RegisterRateLimiter {
    pub fn new(
        redis: Client,
        namespace: impl Into<String>,
        max_attempts: u32,
        window: Duration,
        command_timeout: Duration,
    ) -> Self {
        Self {
            redis,
            namespace: namespace.into(),
            max_attempts: i64::from(max_attempts.max(1)),
            window_ms: i64::try_from(window.as_millis().max(1)).unwrap_or(i64::MAX),
            command_timeout,
        }
    }

    pub async fn try_acquire(&self, ip: IpAddr) -> RateLimitOutcome {
        let key = format!("{}:{}", self.namespace, ip);
        // 依赖卡住时按不可用处理，绝不无上限放行。
        let count = tokio::time::timeout(
            self.command_timeout,
            self.redis.eval::<(i64, i64), _, _, _>(
                INCR_SCRIPT,
                vec![key],
                vec![self.window_ms.to_string()],
            ),
        )
        .await;
        match count {
            Ok(Ok((count, _))) if count <= self.max_attempts => RateLimitOutcome::Allowed,
            Ok(Ok((_, ttl_ms))) => RateLimitOutcome::Limited {
                retry_after_seconds: retry_after_seconds(ttl_ms),
            },
            Ok(Err(_)) | Err(_) => RateLimitOutcome::Unavailable,
        }
    }
}

/// 登录限流：同一窗口内分别按“客户端 IP + 规范化账号”和“客户端 IP”统计尝试次数。
///
/// 两个计数器独立：账号维度避免他人故意输错锁死真实用户（不做只按账号的全局锁），
/// IP 维度限制单来源的横向猜解。校验密码前原子预占名额，并发突发无法越过上限；
/// 凭据错误保持占用，成功登录归还额度。
#[derive(Clone)]
pub struct LoginRateLimiter {
    redis: Client,
    namespace: String,
    max_per_account: i64,
    max_per_ip: i64,
    window_ms: i64,
    command_timeout: Duration,
}

impl LoginRateLimiter {
    pub fn new(
        redis: Client,
        namespace: impl Into<String>,
        max_per_account: u32,
        max_per_ip: u32,
        window: Duration,
        command_timeout: Duration,
    ) -> Self {
        Self {
            redis,
            namespace: namespace.into(),
            max_per_account: i64::from(max_per_account.max(1)),
            max_per_ip: i64::from(max_per_ip.max(1)),
            window_ms: i64::try_from(window.as_millis().max(1)).unwrap_or(i64::MAX),
            command_timeout,
        }
    }

    fn ip_key(&self, ip: IpAddr) -> String {
        format!("{}:login:ip:{}", self.namespace, ip)
    }

    fn account_key(&self, ip: IpAddr, identity: &str) -> String {
        format!(
            "{}:login:acct:{}:{}",
            self.namespace,
            ip,
            identity_key(identity)
        )
    }

    /// 校验密码前原子预占一个名额：任一计数器已达上限则 `Limited`（不自增），
    /// 否则两个计数器都 `INCR` 并返回 `Allowed`。Redis 故障时 `Unavailable`（fail closed）。
    pub async fn acquire(&self, ip: IpAddr, identity: &str) -> RateLimitOutcome {
        let result = tokio::time::timeout(
            self.command_timeout,
            self.redis.eval::<(i64, i64), _, _, _>(
                LOGIN_ACQUIRE_SCRIPT,
                vec![self.account_key(ip, identity), self.ip_key(ip)],
                vec![
                    self.max_per_account.to_string(),
                    self.max_per_ip.to_string(),
                    self.window_ms.to_string(),
                ],
            ),
        )
        .await;
        match result {
            Ok(Ok((0, _))) => RateLimitOutcome::Allowed,
            Ok(Ok((_, ttl_ms))) => RateLimitOutcome::Limited {
                retry_after_seconds: retry_after_seconds(ttl_ms),
            },
            Ok(Err(_)) | Err(_) => RateLimitOutcome::Unavailable,
        }
    }

    /// 成功登录后归还额度：删除“IP+账号”计数并把 IP 计数减一（不为负、保留 TTL）。
    /// 失败只记录，不影响已通过的登录。
    pub async fn release(&self, ip: IpAddr, identity: &str) {
        let result = tokio::time::timeout(
            self.command_timeout,
            self.redis.eval::<i64, _, _, _>(
                LOGIN_RELEASE_SCRIPT,
                vec![self.account_key(ip, identity), self.ip_key(ip)],
                Vec::<String>::new(),
            ),
        )
        .await;
        if !matches!(result, Ok(Ok(_))) {
            tracing::warn!(
                target: "lycoris_backend::ratelimit",
                "登录成功但归还限流额度失败"
            );
        }
    }
}

/// 解析用于限流的客户端 IP。
///
/// 默认使用实际连接 IP；仅当连接来自显式配置的可信代理时才读取
/// `X-Forwarded-For` 第一段，避免调用方自选限流身份。坏 IP 回退连接 IP。
pub fn resolve_client_ip(
    connect_ip: Option<IpAddr>,
    headers: &axum::http::HeaderMap,
    trusted_proxies: &[IpAddr],
) -> IpAddr {
    let fallback = connect_ip.unwrap_or(IpAddr::from([0, 0, 0, 0]));
    let trusted = connect_ip.is_some_and(|ip| trusted_proxies.contains(&ip));
    if !trusted {
        return fallback;
    }
    let Some(forwarded) = headers.get("x-forwarded-for") else {
        return fallback;
    };
    let Ok(raw) = forwarded.to_str() else {
        return fallback;
    };
    raw.split(',')
        .next()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .and_then(|part| part.parse::<IpAddr>().ok())
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::{identity_key, resolve_client_ip};
    use axum::http::{HeaderMap, HeaderValue};
    use std::net::IpAddr;

    fn headers(xff: Option<&str>) -> HeaderMap {
        let mut map = HeaderMap::new();
        if let Some(value) = xff {
            map.insert("x-forwarded-for", HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn untrusted_connection_ignores_forwarded_header() {
        let connect = IpAddr::from([10, 0, 0, 5]);
        let trusted: [IpAddr; 0] = [];
        assert_eq!(
            resolve_client_ip(Some(connect), &headers(Some("1.2.3.4")), &trusted),
            connect
        );
    }

    #[test]
    fn trusted_proxy_uses_first_forwarded_ip() {
        let connect = IpAddr::from([10, 0, 0, 5]);
        let trusted = [connect];
        assert_eq!(
            resolve_client_ip(Some(connect), &headers(Some("1.2.3.4, 10.0.0.1")), &trusted),
            IpAddr::from([1, 2, 3, 4])
        );
    }

    #[test]
    fn malformed_forwarded_falls_back_to_connection() {
        let connect = IpAddr::from([10, 0, 0, 5]);
        let trusted = [connect];
        assert_eq!(
            resolve_client_ip(Some(connect), &headers(Some("not-an-ip")), &trusted),
            connect
        );
    }

    #[test]
    fn identity_key_normalizes_and_hashes() {
        // trim + 小写后同口径：用户名与邮箱共用一个键片段。
        assert_eq!(identity_key(" Alice "), identity_key("alice"));
        assert_eq!(
            identity_key("ALICE@Example.com"),
            identity_key(" alice@example.com ")
        );
        assert_eq!(
            identity_key("alice").len(),
            16,
            "取 sha256 前 8 字节 = 16 位十六进制"
        );
        assert_ne!(identity_key("alice"), identity_key("bob"));
        assert!(
            identity_key("alice")
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
            "键片段必须是十六进制"
        );
    }
}
