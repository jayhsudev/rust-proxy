use bcrypt::{DEFAULT_COST, hash, verify};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{Mutex, Semaphore};

/// Upper bound for cached successful authentications.
const MAX_CACHE_ENTRIES: usize = 4096;

#[derive(Error, Debug)]
pub enum AuthError {
    #[error("Password hashing failed: {0}")]
    HashingError(#[from] bcrypt::BcryptError),
    #[error("Invalid user configuration: {0}")]
    InvalidUserConfig(String),
    #[error("Authentication failed")]
    AuthenticationFailed,
}

/// Tuning knobs for [`AuthManager`].
#[derive(Debug, Clone)]
pub struct AuthOptions {
    /// How long a successful credential check is cached. `Duration::ZERO` disables it.
    pub cache_ttl: Duration,
    /// Max concurrent bcrypt verifications. `0` means "number of CPUs".
    pub max_concurrency: usize,
}

impl Default for AuthOptions {
    fn default() -> Self {
        Self {
            cache_ttl: Duration::from_secs(60),
            max_concurrency: 0,
        }
    }
}

pub struct AuthManager {
    users: HashMap<String, String>,
    /// Caps concurrent bcrypt work so a burst of connections cannot saturate the
    /// blocking pool / CPU.
    limiter: Arc<Semaphore>,
    /// Successful `(user, password)` checks, keyed by a SHA-256 digest.
    cache: Mutex<HashMap<[u8; 32], Instant>>,
    cache_ttl: Duration,
    /// Hash used to keep unknown-username checks as slow as known ones.
    dummy_hash: Option<String>,
}

impl AuthManager {
    /// Passwords may be provided in plaintext (hashed at startup with bcrypt)
    /// or already as a bcrypt hash (`$2a$` / `$2b$` / `$2y$`), which is used as-is.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(users: &HashMap<String, String>) -> Result<Self, AuthError> {
        Self::with_options(users, AuthOptions::default())
    }

    pub fn with_options(
        users: &HashMap<String, String>,
        options: AuthOptions,
    ) -> Result<Self, AuthError> {
        let mut hashed_users = HashMap::with_capacity(users.len());

        for (username, password) in users {
            if username.trim().is_empty() {
                return Err(AuthError::InvalidUserConfig(
                    "username must not be empty".to_string(),
                ));
            }
            if password.is_empty() {
                return Err(AuthError::InvalidUserConfig(format!(
                    "password for user '{}' must not be empty",
                    username
                )));
            }

            let hashed_password = if is_bcrypt_hash(password) {
                password.clone()
            } else {
                hash(password, DEFAULT_COST)?
            };

            hashed_users.insert(username.clone(), hashed_password);
        }

        let max_concurrency = if options.max_concurrency == 0 {
            std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(4)
        } else {
            options.max_concurrency
        };

        let dummy_hash = hashed_users.values().next().cloned();

        Ok(AuthManager {
            users: hashed_users,
            limiter: Arc::new(Semaphore::new(max_concurrency)),
            cache: Mutex::new(HashMap::new()),
            cache_ttl: options.cache_ttl,
            dummy_hash,
        })
    }

    pub fn has_users(&self) -> bool {
        !self.users.is_empty()
    }

    pub fn user_count(&self) -> usize {
        self.users.len()
    }

    /// Verifies a username/password pair.
    ///
    /// Bcrypt comparison runs inside `spawn_blocking` to avoid stalling the Tokio
    /// runtime, and the number of concurrent comparisons is capped by a semaphore.
    /// Successful checks are cached for `cache_ttl` (keyed by a digest, never by
    /// plaintext) so long-lived clients do not pay bcrypt on every connection.
    ///
    /// When no users are configured authentication is disabled and every attempt
    /// succeeds, which mirrors the behaviour of the SOCKS5 / HTTP entry points.
    pub async fn authenticate(&self, username: &str, password: &str) -> Result<bool, AuthError> {
        if self.users.is_empty() {
            return Ok(true);
        }

        let key = cache_key(username, password);
        if self.is_cached(&key).await {
            return Ok(true);
        }

        let known_user = self.users.get(username).cloned();
        let hashed = known_user
            .clone()
            .or_else(|| self.dummy_hash.clone())
            .ok_or(AuthError::AuthenticationFailed)?;

        let _permit = self
            .limiter
            .acquire()
            .await
            .map_err(|_| AuthError::AuthenticationFailed)?;

        let password = password.to_string();
        let verified = tokio::task::spawn_blocking(move || verify(&password, &hashed))
            .await
            .map_err(|_| AuthError::AuthenticationFailed)?
            .map_err(AuthError::HashingError)?;

        // An unknown username is verified against another user's hash purely to
        // equalise timing; it must never count as success.
        let authenticated = verified && known_user.is_some();

        if authenticated {
            self.store_cache(key).await;
        }

        Ok(authenticated)
    }

    async fn is_cached(&self, key: &[u8; 32]) -> bool {
        if self.cache_ttl.is_zero() {
            return false;
        }

        let mut cache = self.cache.lock().await;
        match cache.get(key) {
            Some(expires_at) if *expires_at > Instant::now() => true,
            Some(_) => {
                cache.remove(key);
                false
            }
            None => false,
        }
    }

    async fn store_cache(&self, key: [u8; 32]) {
        if self.cache_ttl.is_zero() {
            return;
        }

        let mut cache = self.cache.lock().await;

        if cache.len() >= MAX_CACHE_ENTRIES {
            let now = Instant::now();
            cache.retain(|_, expires_at| *expires_at > now);

            if cache.len() >= MAX_CACHE_ENTRIES {
                cache.clear();
            }
        }

        cache.insert(key, Instant::now() + self.cache_ttl);
    }

    #[cfg(test)]
    pub(crate) async fn cached_entries(&self) -> usize {
        self.cache.lock().await.len()
    }
}

fn cache_key(username: &str, password: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(username.as_bytes());
    hasher.update([0u8]);
    hasher.update(password.as_bytes());
    hasher.finalize().into()
}

fn is_bcrypt_hash(value: &str) -> bool {
    value.len() == 60
        && (value.starts_with("$2a$") || value.starts_with("$2b$") || value.starts_with("$2y$"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_authenticate() {
        let mut users = HashMap::new();
        users.insert("admin".to_string(), "password".to_string());
        users.insert("user1".to_string(), "pass123".to_string());

        let auth_manager = AuthManager::new(&users).unwrap();
        assert!(auth_manager.has_users());
        assert_eq!(auth_manager.user_count(), 2);

        assert!(
            auth_manager
                .authenticate("admin", "password")
                .await
                .unwrap()
        );
        assert!(auth_manager.authenticate("user1", "pass123").await.unwrap());
        assert!(
            !auth_manager
                .authenticate("admin", "wrongpass")
                .await
                .unwrap()
        );
        assert!(
            !auth_manager
                .authenticate("nonexistent", "password")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn test_no_users_accepts_everything() {
        let auth_manager = AuthManager::new(&HashMap::new()).unwrap();
        assert!(!auth_manager.has_users());
        assert!(
            auth_manager
                .authenticate("anyone", "anything")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn test_prehashed_password() {
        let hash = hash("prehashed", DEFAULT_COST).unwrap();
        let mut users = HashMap::new();
        users.insert("bob".to_string(), hash);

        let auth_manager = AuthManager::new(&users).unwrap();
        assert!(auth_manager.authenticate("bob", "prehashed").await.unwrap());
        assert!(!auth_manager.authenticate("bob", "other").await.unwrap());
    }

    #[test]
    fn test_rejects_invalid_users() {
        let mut users = HashMap::new();
        users.insert("   ".to_string(), "password".to_string());
        assert!(AuthManager::new(&users).is_err());

        let mut users = HashMap::new();
        users.insert("alice".to_string(), String::new());
        assert!(AuthManager::new(&users).is_err());
    }

    #[tokio::test]
    async fn test_successful_auth_is_cached() {
        let mut users = HashMap::new();
        users.insert("alice".to_string(), "secret".to_string());

        let auth_manager = AuthManager::new(&users).unwrap();
        assert_eq!(auth_manager.cached_entries().await, 0);

        assert!(auth_manager.authenticate("alice", "secret").await.unwrap());
        assert_eq!(auth_manager.cached_entries().await, 1);

        // Cached and still valid.
        assert!(auth_manager.authenticate("alice", "secret").await.unwrap());

        // Failures are never cached.
        assert!(!auth_manager.authenticate("alice", "wrong").await.unwrap());
        assert_eq!(auth_manager.cached_entries().await, 1);

        // Different password gets its own cache entry only when successful.
        let other = AuthManager::with_options(
            &users,
            AuthOptions {
                cache_ttl: Duration::ZERO,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(other.authenticate("alice", "secret").await.unwrap());
        assert_eq!(other.cached_entries().await, 0);
    }

    #[tokio::test]
    async fn test_unknown_user_is_never_accepted() {
        let mut users = HashMap::new();
        users.insert("alice".to_string(), "secret".to_string());

        let auth_manager = AuthManager::with_options(
            &users,
            AuthOptions {
                cache_ttl: Duration::ZERO,
                max_concurrency: 1,
            },
        )
        .unwrap();

        for _ in 0..3 {
            assert!(!auth_manager.authenticate("ghost", "secret").await.unwrap());
        }
        assert_eq!(auth_manager.cached_entries().await, 0);
    }

    #[tokio::test]
    async fn test_concurrent_authentication() {
        let mut users = HashMap::new();
        users.insert("alice".to_string(), "secret".to_string());

        let auth_manager = Arc::new(
            AuthManager::with_options(
                &users,
                AuthOptions {
                    cache_ttl: Duration::ZERO,
                    max_concurrency: 2,
                },
            )
            .unwrap(),
        );

        let mut handles = Vec::new();
        for _ in 0..8 {
            let manager = auth_manager.clone();
            handles.push(tokio::spawn(async move {
                manager.authenticate("alice", "secret").await.unwrap()
            }));
        }

        for handle in handles {
            assert!(handle.await.unwrap());
        }
    }
}
