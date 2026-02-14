use bcrypt::{hash, verify, DEFAULT_COST};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AuthError {
    #[error("Password hashing failed: {0}")]
    HashingError(#[from] bcrypt::BcryptError),
    #[error("Authentication failed")]
    AuthenticationFailed,
}

pub struct AuthManager {
    users: HashMap<String, String>,
}

impl AuthManager {
    pub fn new(users: &HashMap<String, String>) -> Result<Self, AuthError> {
        let mut hashed_users = HashMap::new();
        for (username, password) in users {
            let hashed_password = hash(password, DEFAULT_COST)?;
            hashed_users.insert(username.clone(), hashed_password);
        }
        Ok(AuthManager {
            users: hashed_users,
        })
    }

    pub fn has_users(&self) -> bool {
        !self.users.is_empty()
    }

    /// Bcrypt comparison runs inside `spawn_blocking` to avoid stalling the Tokio runtime.
    pub async fn authenticate(&self, username: &str, password: &str) -> Result<bool, AuthError> {
        if self.users.is_empty() {
            return Ok(true);
        }

        match self.users.get(username) {
            Some(hashed_password) => {
                let hashed = hashed_password.clone();
                let pwd = password.to_string();
                let is_valid = tokio::task::spawn_blocking(move || verify(&pwd, &hashed))
                    .await
                    .map_err(|_| AuthError::AuthenticationFailed)??;
                Ok(is_valid)
            }
            None => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_authenticate() {
        let mut users = HashMap::new();
        users.insert("admin".to_string(), "password".to_string());
        users.insert("user1".to_string(), "pass123".to_string());

        let auth_manager = AuthManager::new(&users).unwrap();

        assert!(auth_manager
            .authenticate("admin", "password")
            .await
            .unwrap());
        assert!(auth_manager.authenticate("user1", "pass123").await.unwrap());
        assert!(!auth_manager
            .authenticate("admin", "wrongpass")
            .await
            .unwrap());
        assert!(!auth_manager
            .authenticate("nonexistent", "password")
            .await
            .unwrap());
    }
}
