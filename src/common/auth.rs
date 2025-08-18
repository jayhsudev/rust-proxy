use bcrypt::{hash, verify, DEFAULT_COST};
use std::collections::HashMap;
use thiserror::Error;

/// Authentication errors
#[derive(Error, Debug)]
pub enum AuthError {
    #[error("Password hashing failed: {0}")]
    HashingError(#[from] bcrypt::BcryptError),
    #[error("Authentication failed")]
    AuthenticationFailed,
}

/// Authentication manager
pub struct AuthManager {
    /// Store mapping of usernames to hashed passwords
    users: HashMap<String, String>,
}

impl AuthManager {
    /// Create new authentication manager
    pub fn new(users: &HashMap<String, String>) -> Result<Self, AuthError> {
        let mut hashed_users = HashMap::new();

        for (username, password) in users {
            let hashed_password = Self::hash_password(password)?;
            hashed_users.insert(username.clone(), hashed_password);
        }

        Ok(AuthManager {
            users: hashed_users,
        })
    }

    /// Check if there are user configurations
    pub fn has_users(&self) -> bool {
        !self.users.is_empty()
    }

    /// Verify username and password
    pub fn authenticate(&self, username: &str, password: &str) -> Result<bool, AuthError> {
        match self.users.get(username) {
            Some(hashed_password) => {
                let is_valid = verify(password, hashed_password)?;
                if is_valid {
                    Ok(true)
                } else {
                    Err(AuthError::AuthenticationFailed)
                }
            }
            None => Err(AuthError::AuthenticationFailed),
        }
    }

    /// Hash password
    fn hash_password(password: &str) -> Result<String, AuthError> {
        let hashed = hash(password, DEFAULT_COST)?;
        Ok(hashed)
    }
}

/// Authentication result
#[allow(dead_code)]
pub enum AuthResult {
    /// Authentication successful
    Success,
    /// Authentication failed
    Failed,
    /// Need more data
    NeedMoreData,
}

/// Supported authentication types
#[allow(dead_code)]
pub enum AuthType {
    /// No authentication
    None,
    /// Basic authentication (username + password)
    Basic,
    /// SOCKS5 authentication
    Socks5,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_authenticate() {
        let mut users = HashMap::new();
        users.insert("admin".to_string(), "password".to_string());
        users.insert("user1".to_string(), "pass123".to_string());

        let auth_manager = AuthManager::new(&users).unwrap();

        // Test successful authentication
        assert!(auth_manager.authenticate("admin", "password").unwrap());
        assert!(auth_manager.authenticate("user1", "pass123").unwrap());

        // Test failed authentication
        assert!(auth_manager.authenticate("admin", "wrongpass").is_err());
        assert!(auth_manager
            .authenticate("nonexistent", "password")
            .is_err());
    }
}
