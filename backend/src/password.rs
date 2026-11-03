//! 密码哈希与历史兼容。
//!
//! - BCrypt 计算在 `spawn_blocking` 中执行，并持有 `OwnedSemaphorePermit`；许可被移动进
//!   阻塞闭包，直到任务真正结束才释放，因此请求超时/取消也不会导致 CPU 无限堆积。
//! - 先检查 UTF-8 字节数不超过 72，再调用普通 `bcrypt::hash/verify`。`bcrypt 0.19.3`
//!   的 `non_truncating_*` 会拒绝恰好 72 字节，不能用。
//! - 历史明文仅在存储值不具有 BCrypt 或 `{...}` 前缀时才允许按明文比较；形如哈希的
//!   字符串绝不回退明文比较。Rust 新哈希为 `$2b$`，可被 Java BCrypt 验证。
//! - 绝不记录原始密码、哈希或 Cookie/标识。

use std::sync::Arc;

use tokio::sync::{OnceCell, OwnedSemaphorePermit, Semaphore};

/// BCrypt 允许的最大 UTF-8 字节数；超过则拒绝（与 Java/BCrypt 规范一致）。
pub const MAX_BCRYPT_BYTES: usize = 72;
/// 新密码最短长度，按 Java `String.length()`（UTF-16 码元数）兼容。
pub const MIN_PASSWORD_UTF16_UNITS: usize = 4;
/// 只用于生成计时对照哈希的固定输入；从不对应任何账号。
const TIMING_REFERENCE_INPUT: &str = "lycoris-timing-reference";

#[derive(Debug, thiserror::Error)]
pub enum HashError {
    #[error("密码超过 72 字节")]
    TooLong,
    #[error("密码哈希失败")]
    Bcrypt(#[from] bcrypt::BcryptError),
    #[error("密码哈希任务异常")]
    Task(#[from] tokio::task::JoinError),
    #[error("密码哈希并发许可不可用")]
    Closed,
}

/// 线程池受限的 BCrypt 执行器。
#[derive(Clone)]
pub struct PasswordHasher {
    permits: Arc<Semaphore>,
    cost: u32,
    /// 与真实哈希同成本的对照值，首次需要时生成。
    timing_reference: Arc<OnceCell<String>>,
}

impl PasswordHasher {
    /// `max_concurrency` 至少为 1；通常取 CPU 并行度，避免 BCrypt 拖垮异步运行时。
    pub fn new(cost: u32, max_concurrency: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max_concurrency.max(1))),
            cost,
            timing_reference: Arc::new(OnceCell::new()),
        }
    }

    pub fn cost(&self) -> u32 {
        self.cost
    }

    /// 生成新的 BCrypt 哈希（`$2b$`）。超过 72 字节直接拒绝。
    pub async fn hash(&self, password: String) -> Result<String, HashError> {
        if password.len() > MAX_BCRYPT_BYTES {
            return Err(HashError::TooLong);
        }
        let permit = self.acquire().await?;
        let cost = self.cost;
        // 许可随闭包移动，阻塞任务真正结束时才 Drop 释放。
        let handle = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            bcrypt::hash(password, cost)
        });
        Ok(handle.await??)
    }

    /// 校验 BCrypt 哈希。密码超过 72 字节或哈希格式非法时按失败处理（与 Java 一致）。
    pub async fn verify(&self, password: String, hash: String) -> bool {
        if password.len() > MAX_BCRYPT_BYTES {
            return false;
        }
        let permit = match self.acquire().await {
            Ok(permit) => permit,
            Err(_) => return false,
        };
        let handle = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            bcrypt::verify(password, &hash).unwrap_or(false)
        });
        handle.await.unwrap_or(false)
    }

    /// 与 Java `matchesPasswordSafely` 等价：编码串只走 BCrypt，历史明文才允许字面比较。
    pub async fn matches(&self, raw: &str, stored: &str) -> bool {
        if stored.trim().is_empty() {
            self.equalize(raw).await;
            return false;
        }
        if is_encoded(stored) {
            self.verify(raw.to_string(), stored.to_string()).await
        } else {
            raw == stored
        }
    }

    /// 没有可比对的哈希时（账号不存在、重复或无密码），仍跑一次同成本 BCrypt。
    ///
    /// 否则“立即失败”与“比对后失败”的耗时差会暴露某个用户名或邮箱是否已注册。
    /// 结果总是被丢弃；对照哈希生成失败时只跳过等时，不影响认证结论。
    pub async fn equalize(&self, raw: &str) {
        if raw.len() > MAX_BCRYPT_BYTES {
            // 与 verify 相同：超长输入在 BCrypt 之前就被拒绝。
            return;
        }
        let reference = self
            .timing_reference
            .get_or_try_init(|| self.hash(TIMING_REFERENCE_INPUT.to_string()))
            .await;
        if let Ok(reference) = reference {
            let _ = self.verify(raw.to_string(), reference.clone()).await;
        }
    }

    async fn acquire(&self) -> Result<OwnedSemaphorePermit, HashError> {
        self.permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| HashError::Closed)
    }
}

/// 是否是 Java 认可的 BCrypt 编码前缀（`$2a$`/`$2b$`/`$2y$`）。
pub fn looks_like_bcrypt(value: &str) -> bool {
    value.starts_with("$2a$") || value.starts_with("$2b$") || value.starts_with("$2y$")
}

/// 存储值是否按“编码凭据”处理；`{...}` 也视为编码，不允许回退明文。
pub fn is_encoded(value: &str) -> bool {
    looks_like_bcrypt(value) || value.starts_with('{')
}

/// UTF-16 码元长度，用于兼容 Java 的最短长度规则。
pub fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

/// 是否满足 BCrypt 的字节上限。
pub fn within_bcrypt_limit(value: &str) -> bool {
    value.len() <= MAX_BCRYPT_BYTES
}

#[cfg(test)]
mod tests {
    use super::{is_encoded, looks_like_bcrypt, utf16_len, within_bcrypt_limit};
    use base64::Engine as _;

    /// Public synthetic vectors generated with Java 21 / Spring Security Crypto 6.5.7.
    /// （password 列为 base64(UTF-8)）。cost 4 仅测试，生产 10。
    const JAVA_VECTORS: &[(&str, &str)] = &[
        (
            "c3ludGhldGljLXBhc3N3b3Jk",
            "$2a$04$u2qdf8QkuekWvHmMdtdTaOFH9gC6mJZLEuDaxjRnJ9FFrBAe7jU6C",
        ),
        (
            "5ZCI5oiQ5a+G56CB8J+Mt2NhZsOp",
            "$2a$04$AJtrfbeEBSaAGrfKkpolOOqWXln8cJhsk2WYSwrpHf8NxMJ7OH3ju",
        ),
        (
            "YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE=",
            "$2a$04$srP7wITHEbKn35m3e2fpouq7vlH61yENUvI100zuCGN3MD2WMlKzm",
        ),
        (
            "YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFh",
            "$2a$04$NH3Jmh7ovHCmheJq89Zs9eHufbQA2lAD04VxDy90Eyu5xahoKTtSi",
        ),
        (
            "5aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW9",
            "$2a$04$5hzKzShdnwBW2yAI8sDwreZr.gc71d9VMb.aOs9k2e2GD7T0B3JkW",
        ),
        (
            "YWJjAGRlZg==",
            "$2a$04$IUrV/EEV3YhSQCVF.m0iR..Z6X/qaTtMFq4Y7xW2sYYj737UkcoG6",
        ),
    ];

    fn decode(input: &str) -> String {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(input)
            .expect("向量 base64 解码失败");
        String::from_utf8(bytes).expect("向量应为 UTF-8")
    }

    #[tokio::test]
    async fn java_vectors_verify_with_truncating_api() {
        let hasher = super::PasswordHasher::new(4, 1);
        for (encoded, hash) in JAVA_VECTORS {
            let password = decode(encoded);
            assert!(
                hasher.verify(password.clone(), (*hash).to_string()).await,
                "Java 向量应能用普通 verify 校验: {encoded}"
            );
        }
    }

    #[tokio::test]
    async fn rust_hash_is_two_b_and_round_trips() {
        let hasher = super::PasswordHasher::new(4, 1);
        let hash = hasher.hash("合成密码-🔷".to_string()).await.unwrap();
        assert!(hash.starts_with("$2b$"), "Rust 新哈希应为 $2b$: {hash}");
        assert!(hasher.verify("合成密码-🔷".to_string(), hash).await);
    }

    #[tokio::test]
    async fn missing_credentials_still_spend_one_hash() {
        let hasher = super::PasswordHasher::new(4, 1);
        assert!(hasher.timing_reference.get().is_none());
        // 没有存储密码时仍失败，但已跑过一次同成本 BCrypt。
        assert!(!hasher.matches("anything", "").await);
        let reference = hasher
            .timing_reference
            .get()
            .expect("对照哈希已生成")
            .clone();
        assert!(reference.starts_with("$2b$04$"), "对照哈希与配置成本一致");
        // 对照值复用，不会每次请求都重新生成。
        hasher.equalize("another").await;
        assert_eq!(hasher.timing_reference.get(), Some(&reference));
    }

    #[tokio::test]
    async fn rejects_passwords_over_72_bytes() {
        let hasher = super::PasswordHasher::new(4, 1);
        let bytes73 = "a".repeat(73);
        assert!(matches!(
            hasher.hash(bytes73.clone()).await,
            Err(super::HashError::TooLong)
        ));
        // 恰好 72 字节允许。
        assert!(hasher.hash("a".repeat(72)).await.is_ok());
        // 73 字节的 Unicode（按字节）也拒绝。
        let unicode73 = "好".repeat(24).to_string();
        assert_eq!(unicode73.len(), 72);
        assert!(hasher.hash(unicode73).await.is_ok());
        let unicode_long = format!("{}A", "好".repeat(24));
        assert!(unicode_long.len() > 72);
        assert!(matches!(
            hasher.hash(unicode_long).await,
            Err(super::HashError::TooLong)
        ));
    }

    #[test]
    fn encoded_prefix_detection_matches_java() {
        assert!(looks_like_bcrypt("$2a$04$abc"));
        assert!(looks_like_bcrypt("$2b$10$abc"));
        assert!(looks_like_bcrypt("$2y$10$abc"));
        // Java 不把 $2x$ 视作 bcrypt，需回退明文比较。
        assert!(!looks_like_bcrypt("$2x$10$abc"));
        assert!(is_encoded("$2a$malformed"));
        assert!(is_encoded("{bcrypt}$2a$10$abc"));
        assert!(!is_encoded("legacy-password"));
    }

    #[test]
    fn utf16_length_matches_java_semantics() {
        assert_eq!(utf16_len("abcd"), 4);
        // emoji 是代理对，UTF-16 长度为 2。
        assert_eq!(utf16_len("a🔷"), 3);
        assert!(within_bcrypt_limit(&"a".repeat(72)));
        assert!(!within_bcrypt_limit(&"a".repeat(73)));
    }
}
