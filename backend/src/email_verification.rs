//! Email challenges: required TLS, keyed digests, atomic single-use consumption
//! and per-email cooldown shared by registration and password recovery.
use crate::config::{Config, ConfigError};
use fred::{clients::Client, prelude::*};
use hmac::{Hmac, KeyInit, Mac};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor, message::Mailbox,
    transport::smtp::authentication::Credentials,
};
use serde::Deserialize;
use sha2::Sha256;
use std::{future::Future, net::IpAddr, pin::Pin, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

pub const CODE_TTL_SECONDS: u32 = 600;
pub const RESEND_SECONDS: u32 = 60;
pub const COOLDOWN_SECONDS: u32 = 3600;

// No Debug: these values contain authentication credentials.
#[derive(Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from: Mailbox,
}
impl SmtpConfig {
    pub fn from_env() -> Result<Option<Self>, ConfigError> {
        let read = |key: &'static str| {
            std::env::var(key)
                .ok()
                .filter(|s| !s.trim().is_empty())
                .ok_or(ConfigError::Missing(key))
        };
        let Ok(host) = read("SMTP_HOST") else {
            return Ok(None);
        };
        if read("SMTP_SECURITY")? != "starttls" {
            return Err(ConfigError::Invalid("SMTP_SECURITY"));
        }
        let port = read("SMTP_PORT")?
            .parse::<u16>()
            .map_err(|_| ConfigError::Invalid("SMTP_PORT"))?;
        if port == 0 {
            return Err(ConfigError::Invalid("SMTP_PORT"));
        }
        let from = Mailbox::new(
            Some(read("SMTP_FROM_NAME")?),
            read("SMTP_FROM_ADDRESS")?
                .parse()
                .map_err(|_| ConfigError::Invalid("SMTP_FROM_ADDRESS"))?,
        );
        Ok(Some(Self {
            host,
            port,
            username: read("SMTP_USERNAME")?,
            password: read("SMTP_PASSWORD")?,
            from,
        }))
    }
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Register,
    ResetPassword,
}
impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::ResetPassword => "reset_password",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeError {
    Invalid,
    InvalidEmail,
    Limited(u32),
    Locked(u32),
    Unavailable,
}
pub type SendResult<'a> = Pin<Box<dyn Future<Output = Result<(), CodeError>> + Send + 'a>>;
/// Injected transport allows integration tests to capture mail without sending
/// to real recipients; production always constructs the TLS SMTP transport.
pub trait CodeMailer: Send + Sync {
    fn send<'a>(
        &'a self,
        email: &'a str,
        purpose: Purpose,
        code: &'a str,
        chinese: bool,
    ) -> SendResult<'a>;
}
struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}
impl CodeMailer for SmtpMailer {
    fn send<'a>(
        &'a self,
        email: &'a str,
        purpose: Purpose,
        code: &'a str,
        chinese: bool,
    ) -> SendResult<'a> {
        Box::pin(async move {
            let action = match (purpose, chinese) {
                (Purpose::Register, true) => "注册账号",
                (Purpose::ResetPassword, true) => "重置密码",
                (Purpose::Register, false) => "register your account",
                (Purpose::ResetPassword, false) => "reset your password",
            };
            let (subject, body) = if chinese {
                (
                    "Lycoris Maps 邮箱验证码",
                    format!(
                        "你的验证码：{code}\n\n用于{action}，10 分钟内有效，仅可使用一次。\n若不是你本人请求，请忽略这封邮件，不要向任何人透露验证码。\n\nLycoris Maps"
                    ),
                )
            } else {
                (
                    "Lycoris Maps verification code",
                    format!(
                        "Your verification code: {code}\n\nUse this code to {action}. It expires in 10 minutes and can only be used once.\nIf you did not request this email, ignore it. Do not share the code.\n\nLycoris Maps"
                    ),
                )
            };
            let message = Message::builder()
                .from(self.from.clone())
                .to(email.parse().map_err(|_| CodeError::InvalidEmail)?)
                .subject(subject)
                .body(body)
                .map_err(|_| CodeError::Unavailable)?;
            self.transport
                .send(message)
                .await
                .map_err(|_| CodeError::Unavailable)?;
            Ok(())
        })
    }
}

const RESERVE: &str = r#"
local locked = redis.call('PTTL', KEYS[2])
if locked > 0 then return {2, locked} end
local resend = redis.call('PTTL', KEYS[4])
if resend > 0 then return {1, resend} end
for i = 5, 7 do
 local max = ({5, 30, 200})[i-4]
 if tonumber(redis.call('GET', KEYS[i]) or '0') >= max then return {1, redis.call('PTTL', KEYS[i])} end
end
for i = 5, 7 do
 local n = redis.call('INCR', KEYS[i])
 if n == 1 then redis.call('PEXPIRE', KEYS[i], i == 7 and 86400000 or 3600000) end
end
redis.call('SET', KEYS[4], '1', 'PX', 60000)
redis.call('HSET', KEYS[1], 'digest', ARGV[1], 'nonce', ARGV[2], 'ready', '0')
redis.call('PEXPIRE', KEYS[1], 600000)
return {0, 0}
"#;
const REFUND: &str = r#"
local current = tonumber(redis.call('GET', KEYS[1]) or '0')
if current > 0 then redis.call('DECR', KEYS[1]) end
return 1
"#;
const FINISH: &str = r#"
if redis.call('HGET', KEYS[1], 'nonce') ~= ARGV[1] then return 0 end
if ARGV[2] == '1' then
 redis.call('HSET', KEYS[1], 'ready', '1')
 redis.call('PEXPIRE', KEYS[1], 600000)
else redis.call('DEL', KEYS[1]) end
return 1
"#;
const CONSUME: &str = r#"
local locked = redis.call('PTTL', KEYS[2])
if locked > 0 then return {2, locked} end
if redis.call('HGET', KEYS[1], 'ready') ~= '1' then return {3, 0} end
if redis.call('HGET', KEYS[1], 'digest') == ARGV[1] then
 redis.call('DEL', KEYS[1], KEYS[3])
 return {0, 0}
end
local failures = redis.call('INCR', KEYS[3])
if failures == 1 then redis.call('PEXPIRE', KEYS[3], 3600000) end
if failures >= 5 then
 redis.call('SET', KEYS[2], '1', 'PX', 3600000)
 redis.call('DEL', KEYS[1], KEYS[3])
 return {2, 3600000}
end
return {3, 0}
"#;

#[derive(Clone)]
pub struct EmailCodes {
    redis: Client,
    namespace: String,
    secret: Option<Arc<str>>,
    timeout: Duration,
    mailer: Option<Arc<dyn CodeMailer>>,
    slots: Arc<Semaphore>,
}
impl EmailCodes {
    pub fn new(redis: Client, config: &Config) -> Result<Self, ConfigError> {
        let mailer = config
            .smtp
            .as_ref()
            .map(|smtp| {
                let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&smtp.host)
                    .map_err(|_| ConfigError::Invalid("SMTP_HOST"))?
                    .port(smtp.port)
                    .credentials(Credentials::new(
                        smtp.username.clone(),
                        smtp.password.clone(),
                    ))
                    .timeout(Some(Duration::from_secs(10)))
                    .build();
                Ok::<Arc<dyn CodeMailer>, ConfigError>(Arc::new(SmtpMailer {
                    transport,
                    from: smtp.from.clone(),
                }))
            })
            .transpose()?;
        Ok(Self {
            redis,
            namespace: format!("{}:email:v1", config.session_namespace),
            secret: config.email_verification_secret.as_deref().map(Arc::from),
            timeout: config.redis_command_timeout,
            mailer,
            slots: Arc::new(Semaphore::new(2)),
        })
    }
    pub fn with_mailer(mut self, mailer: Arc<dyn CodeMailer>) -> Self {
        self.mailer = Some(mailer);
        self
    }
    pub fn available(&self) -> bool {
        self.secret.is_some() && self.mailer.is_some()
    }
    fn digest(&self, parts: &[&str]) -> Result<String, CodeError> {
        let secret = self.secret.as_ref().ok_or(CodeError::Unavailable)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
            .map_err(|_| CodeError::Unavailable)?;
        for part in parts {
            mac.update(&(part.len() as u64).to_be_bytes());
            mac.update(part.as_bytes());
        }
        Ok(mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }
    fn keys(&self, email: &str, purpose: Purpose) -> Result<Vec<String>, CodeError> {
        let base = format!("{}:{}", self.namespace, self.digest(&["email", email])?);
        Ok(vec![
            format!("{base}:{}", purpose.as_str()),
            format!("{base}:locked"),
            format!("{base}:failures"),
            format!("{base}:resend"),
            format!("{base}:sends"),
        ])
    }
    async fn eval(
        &self,
        script: &str,
        keys: Vec<String>,
        args: Vec<String>,
    ) -> Result<(i64, i64), CodeError> {
        let result = tokio::time::timeout(
            self.timeout,
            self.redis.eval::<(i64, i64), _, _, _>(script, keys, args),
        )
        .await
        .map_err(|_| CodeError::Unavailable)?
        .map_err(|_| CodeError::Unavailable)?;
        match result.0 {
            0 => Ok(result),
            1 => Err(CodeError::Limited(seconds(result.1))),
            2 => Err(CodeError::Locked(seconds(result.1))),
            _ => Err(CodeError::Invalid),
        }
    }
    /// Reserve the challenge and all send limits in one Redis transaction.
    pub async fn reserve(
        &self,
        email: &str,
        purpose: Purpose,
        binding: &str,
        ip: IpAddr,
        code: &str,
    ) -> Result<String, CodeError> {
        let mut keys = self.keys(email, purpose)?;
        keys.push(format!(
            "{}:ip:{}",
            self.namespace,
            self.digest(&["ip", &crate::ratelimit::client_bucket(ip)])?
        ));
        keys.push(self.budget_key());
        let nonce = uuid::Uuid::new_v4().to_string();
        self.eval(
            RESERVE,
            keys,
            vec![
                self.digest(&["code", email, purpose.as_str(), binding, code])?,
                nonce.clone(),
            ],
        )
        .await?;
        Ok(nonce)
    }
    pub async fn finish(
        &self,
        email: &str,
        purpose: Purpose,
        nonce: &str,
        accepted: bool,
    ) -> Result<(), CodeError> {
        let keys = self.keys(email, purpose)?;
        tokio::time::timeout(
            self.timeout,
            self.redis.eval::<i64, _, _, _>(
                FINISH,
                vec![keys[0].clone()],
                vec![
                    nonce.to_string(),
                    if accepted { "1" } else { "0" }.to_string(),
                ],
            ),
        )
        .await
        .map_err(|_| CodeError::Unavailable)?
        .map_err(|_| CodeError::Unavailable)?;
        Ok(())
    }
    pub async fn send(
        &self,
        email: &str,
        purpose: Purpose,
        binding: &str,
        ip: IpAddr,
        chinese: bool,
    ) -> Result<(), CodeError> {
        let mailer = self.mailer.as_ref().ok_or(CodeError::Unavailable)?;
        let _slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| CodeError::Unavailable)?;
        let code = random_code()?;
        let nonce = self.reserve(email, purpose, binding, ip, &code).await?;
        let sent = tokio::time::timeout(
            Duration::from_secs(15),
            mailer.send(email, purpose, &code, chinese),
        )
        .await;
        // The site-wide budget models mail actually handed to SMTP. A rejected
        // delivery returns its slot so an outage cannot exhaust registration and
        // recovery for everyone. Per-email and per-source counters stay spent, and a
        // timeout keeps its slot because the server may still have accepted the mail.
        if matches!(sent, Ok(Err(_))) {
            self.refund_budget().await;
        }
        let sent = sent
            .map_err(|_| CodeError::Unavailable)
            .and_then(|result| result);
        self.finish(email, purpose, &nonce, sent.is_ok()).await?;
        sent
    }
    fn budget_key(&self) -> String {
        format!("{}:send-budget", self.namespace)
    }
    async fn refund_budget(&self) {
        let refunded = tokio::time::timeout(
            self.timeout,
            self.redis
                .eval::<i64, _, _, _>(REFUND, vec![self.budget_key()], Vec::<String>::new()),
        )
        .await;
        if !matches!(refunded, Ok(Ok(_))) {
            tracing::warn!(
                target: "lycoris_backend::email_verification",
                "验证码发送失败后归还全局发信额度失败"
            );
        }
    }
    pub async fn consume(
        &self,
        email: &str,
        purpose: Purpose,
        binding: &str,
        code: &str,
    ) -> Result<(), CodeError> {
        if code.is_empty() || code.len() > 64 {
            return Err(CodeError::Invalid);
        }
        let keys = self.keys(email, purpose)?;
        self.eval(
            CONSUME,
            keys[..3].to_vec(),
            vec![self.digest(&["code", email, purpose.as_str(), binding, code])?],
        )
        .await?;
        Ok(())
    }
}
fn seconds(ms: i64) -> u32 {
    ((ms.max(1) + 999) / 1000).min(i64::from(u32::MAX)) as u32
}
pub fn normalize_email(email: &str) -> Result<String, CodeError> {
    let email = email.trim().to_lowercase();
    if email.len() > 254
        || email.contains(['\r', '\n'])
        || email.parse::<lettre::Address>().is_err()
    {
        return Err(CodeError::InvalidEmail);
    }
    Ok(email)
}
fn random_code() -> Result<String, CodeError> {
    let mut code = String::with_capacity(6);
    while code.len() < 6 {
        let mut byte = [0u8];
        getrandom::fill(&mut byte).map_err(|_| CodeError::Unavailable)?;
        if byte[0] < 250 {
            code.push(char::from(b'0' + byte[0] % 10));
        }
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_email_and_generates_six_digits() {
        assert_eq!(
            normalize_email("  TEST@Example.com ").unwrap(),
            "test@example.com"
        );
        for email in ["no-at", "a@example.com\r\nBcc: x@example.com", ""] {
            assert!(normalize_email(email).is_err());
        }
        for _ in 0..100 {
            let code = random_code().unwrap();
            assert_eq!(code.len(), 6);
            assert!(code.bytes().all(|b| b.is_ascii_digit()));
        }
    }
}

#[cfg(test)]
mod redis_tests {
    use super::*;
    async fn store() -> EmailCodes {
        let url =
            std::env::var("TEST_REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:56379".into());
        assert!(
            url.starts_with("redis://127.0.0.1:"),
            "only synthetic loopback Redis"
        );
        let client =
            fred::types::Builder::from_config(fred::types::config::Config::from_url(&url).unwrap())
                .build()
                .unwrap();
        client.init().await.unwrap();
        let mut config = Config::new("unused", &url);
        config.session_namespace = format!("email-test:{}", uuid::Uuid::new_v4());
        config.email_verification_secret = Some("synthetic-test-key-at-least-32-bytes".into());
        EmailCodes::new(client, &config).unwrap()
    }
    async fn issue(store: &EmailCodes, purpose: Purpose, code: &str) {
        let nonce = store
            .reserve(
                "test@example.test",
                purpose,
                "binding",
                "127.0.0.1".parse().unwrap(),
                code,
            )
            .await
            .unwrap();
        store
            .finish("test@example.test", purpose, &nonce, true)
            .await
            .unwrap();
    }
    async fn expire(store: &EmailCodes, key: &str) {
        store.redis.pexpire::<i64, _>(key, 1, None).await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    #[tokio::test]
    async fn fifth_error_locks_both_purposes_resend_cannot_reset_and_cooldown_expires() {
        let store = store().await;
        issue(&store, Purpose::Register, "123456").await;
        let keys = store.keys("test@example.test", Purpose::Register).unwrap();
        let digest: String = store.redis.hget(&keys[0], "digest").await.unwrap();
        assert!(!digest.contains("123456"));
        for _ in 0..4 {
            assert_eq!(
                store
                    .consume("test@example.test", Purpose::Register, "binding", "000000")
                    .await,
                Err(CodeError::Invalid)
            );
        }
        expire(&store, &keys[3]).await;
        issue(&store, Purpose::ResetPassword, "654321").await;
        assert_eq!(
            store
                .consume(
                    "test@example.test",
                    Purpose::ResetPassword,
                    "binding",
                    "000000"
                )
                .await,
            Err(CodeError::Locked(3600))
        );
        let ttl: i64 = store.redis.pttl(&keys[1]).await.unwrap();
        assert!((3_590_000..=3_600_000).contains(&ttl));
        assert!(matches!(
            store
                .reserve(
                    "test@example.test",
                    Purpose::Register,
                    "binding",
                    "127.0.0.2".parse().unwrap(),
                    "111111"
                )
                .await,
            Err(CodeError::Locked(_))
        ));
        assert!(matches!(
            store
                .consume("test@example.test", Purpose::Register, "binding", "123456")
                .await,
            Err(CodeError::Locked(_))
        ));
        expire(&store, &keys[1]).await;
        expire(&store, &keys[3]).await;
        issue(&store, Purpose::Register, "111111").await;
        assert_eq!(
            store
                .consume("test@example.test", Purpose::Register, "binding", "000000")
                .await,
            Err(CodeError::Invalid)
        );
        assert_eq!(
            store
                .consume("test@example.test", Purpose::Register, "binding", "111111")
                .await,
            Ok(())
        );
    }
    #[tokio::test]
    async fn code_is_single_use_under_concurrency_and_bound_to_email_purpose_and_account_version() {
        let store = store().await;
        issue(&store, Purpose::Register, "123456").await;
        assert_eq!(
            store
                .consume("other@example.test", Purpose::Register, "binding", "123456")
                .await,
            Err(CodeError::Invalid)
        );
        assert_eq!(
            store
                .consume(
                    "test@example.test",
                    Purpose::ResetPassword,
                    "binding",
                    "123456"
                )
                .await,
            Err(CodeError::Invalid)
        );
        assert_eq!(
            store
                .consume(
                    "test@example.test",
                    Purpose::Register,
                    "different-account-version",
                    "123456"
                )
                .await,
            Err(CodeError::Invalid)
        );
        let mut jobs = Vec::new();
        for _ in 0..10 {
            let store = store.clone();
            jobs.push(tokio::spawn(async move {
                store
                    .consume("test@example.test", Purpose::Register, "binding", "123456")
                    .await
            }));
        }
        let mut accepted = 0;
        for job in jobs {
            if job.await.unwrap().is_ok() {
                accepted += 1;
            }
        }
        assert_eq!(accepted, 1);
    }
    #[tokio::test]
    async fn undelivered_expired_and_superseded_challenges_are_not_usable() {
        let store = store().await;
        let nonce = store
            .reserve(
                "test@example.test",
                Purpose::Register,
                "binding",
                "127.0.0.1".parse().unwrap(),
                "123456",
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .consume("test@example.test", Purpose::Register, "binding", "123456")
                .await,
            Err(CodeError::Invalid)
        );
        store
            .finish("test@example.test", Purpose::Register, &nonce, false)
            .await
            .unwrap();
        assert_eq!(
            store
                .consume("test@example.test", Purpose::Register, "binding", "123456")
                .await,
            Err(CodeError::Invalid)
        );
        let keys = store.keys("test@example.test", Purpose::Register).unwrap();
        assert!(matches!(
            store
                .reserve(
                    "test@example.test",
                    Purpose::Register,
                    "binding",
                    "127.0.0.1".parse().unwrap(),
                    "123456"
                )
                .await,
            Err(CodeError::Limited(_))
        ));
        expire(&store, &keys[3]).await;
        let next = store
            .reserve(
                "test@example.test",
                Purpose::Register,
                "binding",
                "127.0.0.1".parse().unwrap(),
                "654321",
            )
            .await
            .unwrap();
        store
            .finish("test@example.test", Purpose::Register, &nonce, true)
            .await
            .unwrap();
        assert_eq!(
            store
                .consume("test@example.test", Purpose::Register, "binding", "654321")
                .await,
            Err(CodeError::Invalid)
        );
        store
            .finish("test@example.test", Purpose::Register, &next, true)
            .await
            .unwrap();
        expire(&store, &keys[0]).await;
        assert_eq!(
            store
                .consume("test@example.test", Purpose::Register, "binding", "654321")
                .await,
            Err(CodeError::Invalid)
        );
    }
    #[tokio::test]
    async fn hourly_email_budget_survives_resends_and_different_ips() {
        let store = store().await;
        let keys = store.keys("test@example.test", Purpose::Register).unwrap();
        for _ in 0..5 {
            issue(&store, Purpose::Register, "123456").await;
            expire(&store, &keys[3]).await;
        }
        assert!(matches!(
            store
                .reserve(
                    "test@example.test",
                    Purpose::ResetPassword,
                    "binding",
                    "127.0.0.2".parse().unwrap(),
                    "123456"
                )
                .await,
            Err(CodeError::Limited(3500..=3600))
        ));
    }
}
