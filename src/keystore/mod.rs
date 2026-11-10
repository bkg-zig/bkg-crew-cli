// Encrypted keystore for API keys and provider configurations.
// Uses SQLite with AES-GCM encryption and Argon2 key derivation.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use aes_gcm::aead::rand_core::RngCore;
use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};
use uuid::Uuid;
use zeroize::ZeroizeOnDrop;

/// Master key for encryption, derived from user passphrase using Argon2.
struct MasterKey {
    key: [u8; 32],
}

impl ZeroizeOnDrop for MasterKey {}

impl MasterKey {
    /// Derive a master key from a passphrase and salt.
    fn derive(passphrase: &str, salt: &[u8; 32]) -> Self {
        let mut key = [0u8; 32];
        let argon2 = Argon2::new(
            Algorithm::Argon2id,
            Version::V0x13,
            Params::new(19456, 2, 1, Some(32)).unwrap(),
        );
        argon2
            .hash_password_into(passphrase.as_bytes(), salt, &mut key)
            .expect("Argon2 key derivation failed");
        Self { key }
    }

    /// Get the key as a reference for AES-GCM.
    fn as_key(&self) -> &Key<Aes256Gcm> {
        Key::<Aes256Gcm>::from_slice(&self.key)
    }
}

/// Encrypted keystore for storing API keys and provider configurations.
#[derive(Clone)]
pub struct Keystore {
    pool: SqlitePool,
    cipher: Aes256Gcm,
}

/// Stored API key entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApiKeyEntry {
    pub id: String,
    pub provider: String,
    pub name: String,
    pub api_key: String,
    pub base_url: Option<String>,
    pub metadata: HashMap<String, String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Provider configuration entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub provider_type: String, // "anthropic", "openai", "openrouter", "custom", etc.
    pub base_url: String,
    pub api_key_id: Option<String>, // Reference to ApiKeyEntry
    pub headers: HashMap<String, String>,
    pub models: Vec<String>, // Model names this provider supports
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

static KEYSTORE_INSTANCE: OnceLock<Keystore> = OnceLock::new();

impl Keystore {
    /// Initialize the keystore with a passphrase.
    /// The database is stored at ~/.bkgcrewcli/keystore.db
    pub async fn init(passphrase: &str) -> Result<Self, String> {
        let path = Self::db_path()?;

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create keystore directory: {e}"))?;
        }

        // Generate or load salt
        let salt_path = path.with_extension("salt");
        let salt = if salt_path.exists() {
            std::fs::read(&salt_path).map_err(|e| format!("Failed to read salt: {e}"))?
        } else {
            let mut salt = [0u8; 32];
            OsRng.fill_bytes(&mut salt);
            std::fs::write(&salt_path, &salt).map_err(|e| format!("Failed to write salt: {e}"))?;
            salt.to_vec()
        };

        if salt.len() != 32 {
            return Err("Invalid salt length".to_string());
        }
        let mut salt_array = [0u8; 32];
        salt_array.copy_from_slice(&salt);

        // Derive master key
        let master_key = MasterKey::derive(passphrase, &salt_array);
        let cipher = Aes256Gcm::new(master_key.as_key());

        // Connect to database
        let db_url = format!("sqlite://{}?mode=rwc", path.display());
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await
            .map_err(|e| format!("Failed to connect to keystore database: {e}"))?;

        // Run migrations
        Self::run_migrations(&pool).await?;

        let keystore = Self { pool, cipher };
        KEYSTORE_INSTANCE.set(keystore.clone()).ok();
        Ok(keystore)
    }

    /// Get the database path.
    fn db_path() -> Result<PathBuf, String> {
        dirs::home_dir()
            .map(|d| d.join(".bkgcrewcli").join("keystore.db"))
            .ok_or_else(|| "Cannot determine home directory".to_string())
    }

    /// Run database migrations.
    async fn run_migrations(pool: &SqlitePool) -> Result<(), String> {
        // Create api_keys table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS api_keys (
                id TEXT PRIMARY KEY,
                provider TEXT NOT NULL,
                name TEXT NOT NULL,
                api_key_encrypted BLOB NOT NULL,
                nonce BLOB NOT NULL,
                base_url TEXT,
                metadata TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(pool)
        .await
        .map_err(|e| format!("Failed to create api_keys table: {e}"))?;

        // Create providers table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS providers (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                provider_type TEXT NOT NULL,
                base_url TEXT NOT NULL,
                api_key_id TEXT,
                headers TEXT NOT NULL DEFAULT '{}',
                models TEXT NOT NULL DEFAULT '[]',
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY (api_key_id) REFERENCES api_keys (id) ON DELETE SET NULL
            )
            "#,
        )
        .execute(pool)
        .await
        .map_err(|e| format!("Failed to create providers table: {e}"))?;

        // Create indexes
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_api_keys_provider ON api_keys(provider)")
            .execute(pool)
            .await
            .map_err(|e| format!("Failed to create index: {e}"))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_providers_type ON providers(provider_type)")
            .execute(pool)
            .await
            .map_err(|e| format!("Failed to create index: {e}"))?;

        Ok(())
    }

    /// Encrypt a string value.
    fn encrypt(&self, plaintext: &str) -> Result<(Vec<u8>, Vec<u8>), String> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|e| format!("Encryption failed: {e}"))?;
        Ok((ciphertext, nonce.to_vec()))
    }

    /// Decrypt a string value.
    fn decrypt(&self, ciphertext: &[u8], nonce: &[u8]) -> Result<String, String> {
        let nonce = Nonce::from_slice(nonce);
        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| format!("Decryption failed: {e}"))?;
        String::from_utf8(plaintext).map_err(|e| format!("Invalid UTF-8 after decryption: {e}"))
    }

    /// Store an API key.
    pub async fn store_api_key(
        &self,
        provider: &str,
        name: &str,
        api_key: &str,
        base_url: Option<&str>,
        metadata: HashMap<String, String>,
    ) -> Result<String, String> {
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        let metadata_json = serde_json::to_string(&metadata).unwrap_or_else(|_| "{}".to_string());

        let (encrypted_key, nonce) = self.encrypt(api_key)?;

        sqlx::query(
            r#"
            INSERT INTO api_keys (id, provider, name, api_key_encrypted, nonce, base_url, metadata, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&id)
        .bind(provider)
        .bind(name)
        .bind(&encrypted_key)
        .bind(&nonce)
        .bind(base_url)
        .bind(&metadata_json)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| format!("Failed to store API key: {e}"))?;

        Ok(id)
    }

    /// Get an API key by ID (decrypted).
    pub async fn get_api_key(&self, id: &str) -> Result<Option<ApiKeyEntry>, String> {
        let row = sqlx::query(
            r#"
            SELECT id, provider, name, api_key_encrypted, nonce, base_url, metadata, created_at, updated_at
            FROM api_keys WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("Failed to get API key: {e}"))?;

        let Some(row) = row else { return Ok(None) };

        let encrypted_key: Vec<u8> = row.try_get("api_key_encrypted").map_err(|e| format!("Failed to read encrypted key: {e}"))?;
        let nonce: Vec<u8> = row.try_get("nonce").map_err(|e| format!("Failed to read nonce: {e}"))?;
        let api_key = self.decrypt(&encrypted_key, &nonce)?;

        let metadata: String = row.try_get("metadata").map_err(|e| format!("Failed to read metadata: {e}"))?;
        let metadata_map: HashMap<String, String> = serde_json::from_str(&metadata).unwrap_or_default();

        Ok(Some(ApiKeyEntry {
            id: row.try_get("id").unwrap(),
            provider: row.try_get("provider").unwrap(),
            name: row.try_get("name").unwrap(),
            api_key,
            base_url: row.try_get("base_url").ok(),
            metadata: metadata_map,
            created_at: chrono::DateTime::parse_from_rfc3339(&row.try_get::<String, _>("created_at").unwrap())
                .unwrap()
                .with_timezone(&chrono::Utc),
            updated_at: chrono::DateTime::parse_from_rfc3339(&row.try_get::<String, _>("updated_at").unwrap())
                .unwrap()
                .with_timezone(&chrono::Utc),
        }))
    }

    /// List all API keys (without decrypting the keys).
    pub async fn list_api_keys(&self) -> Result<Vec<ApiKeyEntry>, String> {
        let rows = sqlx::query(
            r#"
            SELECT id, provider, name, base_url, metadata, created_at, updated_at
            FROM api_keys ORDER BY created_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("Failed to list API keys: {e}"))?;

        let mut keys = Vec::new();
        for row in rows {
            let metadata: String = row.try_get("metadata").unwrap_or_default();
            let metadata_map: HashMap<String, String> = serde_json::from_str(&metadata).unwrap_or_default();

            keys.push(ApiKeyEntry {
                id: row.try_get("id").unwrap(),
                provider: row.try_get("provider").unwrap(),
                name: row.try_get("name").unwrap(),
                api_key: "***".to_string(), // Masked
                base_url: row.try_get("base_url").ok(),
                metadata: metadata_map,
                created_at: chrono::DateTime::parse_from_rfc3339(&row.try_get::<String, _>("created_at").unwrap())
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                updated_at: chrono::DateTime::parse_from_rfc3339(&row.try_get::<String, _>("updated_at").unwrap())
                    .unwrap()
                    .with_timezone(&chrono::Utc),
            });
        }

        Ok(keys)
    }

    /// Delete an API key.
    pub async fn delete_api_key(&self, id: &str) -> Result<bool, String> {
        let result = sqlx::query("DELETE FROM api_keys WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| format!("Failed to delete API key: {e}"))?;

        Ok(result.rows_affected() > 0)
    }

    /// Store a provider configuration.
    pub async fn store_provider(
        &self,
        name: &str,
        provider_type: &str,
        base_url: &str,
        api_key_id: Option<&str>,
        headers: HashMap<String, String>,
        models: Vec<String>,
        enabled: bool,
    ) -> Result<String, String> {
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        let headers_json = serde_json::to_string(&headers).unwrap_or_else(|_| "{}".to_string());
        let models_json = serde_json::to_string(&models).unwrap_or_else(|_| "[]".to_string());

        sqlx::query(
            r#"
            INSERT INTO providers (id, name, provider_type, base_url, api_key_id, headers, models, enabled, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&id)
        .bind(name)
        .bind(provider_type)
        .bind(base_url)
        .bind(api_key_id)
        .bind(&headers_json)
        .bind(&models_json)
        .bind(enabled as i64)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| format!("Failed to store provider: {e}"))?;

        Ok(id)
    }

    /// Get a provider by ID.
    pub async fn get_provider(&self, id: &str) -> Result<Option<ProviderConfig>, String> {
        let row = sqlx::query(
            r#"
            SELECT id, name, provider_type, base_url, api_key_id, headers, models, enabled, created_at, updated_at
            FROM providers WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("Failed to get provider: {e}"))?;

        let Some(row) = row else { return Ok(None) };

        let headers: String = row.try_get("headers").unwrap_or_else(|_| "{}".to_string());
        let headers_map: HashMap<String, String> = serde_json::from_str(&headers).unwrap_or_default();

        let models: String = row.try_get("models").unwrap_or_else(|_| "[]".to_string());
        let models_vec: Vec<String> = serde_json::from_str(&models).unwrap_or_default();

        Ok(Some(ProviderConfig {
            id: row.try_get("id").unwrap(),
            name: row.try_get("name").unwrap(),
            provider_type: row.try_get("provider_type").unwrap(),
            base_url: row.try_get("base_url").unwrap(),
            api_key_id: row.try_get("api_key_id").ok().flatten(),
            headers: headers_map,
            models: models_vec,
            enabled: row.try_get::<i64, _>("enabled").unwrap_or(1) == 1,
            created_at: chrono::DateTime::parse_from_rfc3339(&row.try_get::<String, _>("created_at").unwrap())
                .unwrap()
                .with_timezone(&chrono::Utc),
            updated_at: chrono::DateTime::parse_from_rfc3339(&row.try_get::<String, _>("updated_at").unwrap())
                .unwrap()
                .with_timezone(&chrono::Utc),
        }))
    }

    /// Get a provider by name.
    pub async fn get_provider_by_name(&self, name: &str) -> Result<Option<ProviderConfig>, String> {
        let row = sqlx::query(
            r#"
            SELECT id, name, provider_type, base_url, api_key_id, headers, models, enabled, created_at, updated_at
            FROM providers WHERE name = ?
            "#,
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("Failed to get provider by name: {e}"))?;

        let Some(row) = row else { return Ok(None) };

        let headers: String = row.try_get("headers").unwrap_or_else(|_| "{}".to_string());
        let headers_map: HashMap<String, String> = serde_json::from_str(&headers).unwrap_or_default();

        let models: String = row.try_get("models").unwrap_or_else(|_| "[]".to_string());
        let models_vec: Vec<String> = serde_json::from_str(&models).unwrap_or_default();

        Ok(Some(ProviderConfig {
            id: row.try_get("id").unwrap(),
            name: row.try_get("name").unwrap(),
            provider_type: row.try_get("provider_type").unwrap(),
            base_url: row.try_get("base_url").unwrap(),
            api_key_id: row.try_get("api_key_id").ok().flatten(),
            headers: headers_map,
            models: models_vec,
            enabled: row.try_get::<i64, _>("enabled").unwrap_or(1) == 1,
            created_at: chrono::DateTime::parse_from_rfc3339(&row.try_get::<String, _>("created_at").unwrap())
                .unwrap()
                .with_timezone(&chrono::Utc),
            updated_at: chrono::DateTime::parse_from_rfc3339(&row.try_get::<String, _>("updated_at").unwrap())
                .unwrap()
                .with_timezone(&chrono::Utc),
        }))
    }

    /// List all providers.
    pub async fn list_providers(&self) -> Result<Vec<ProviderConfig>, String> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, provider_type, base_url, api_key_id, headers, models, enabled, created_at, updated_at
            FROM providers ORDER BY name
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("Failed to list providers: {e}"))?;

        let mut providers = Vec::new();
        for row in rows {
            let headers: String = row.try_get("headers").unwrap_or_else(|_| "{}".to_string());
            let headers_map: HashMap<String, String> = serde_json::from_str(&headers).unwrap_or_default();

            let models: String = row.try_get("models").unwrap_or_else(|_| "[]".to_string());
            let models_vec: Vec<String> = serde_json::from_str(&models).unwrap_or_default();

            providers.push(ProviderConfig {
                id: row.try_get("id").unwrap(),
                name: row.try_get("name").unwrap(),
                provider_type: row.try_get("provider_type").unwrap(),
                base_url: row.try_get("base_url").unwrap(),
                api_key_id: row.try_get("api_key_id").ok().flatten(),
                headers: headers_map,
                models: models_vec,
                enabled: row.try_get::<i64, _>("enabled").unwrap_or(1) == 1,
                created_at: chrono::DateTime::parse_from_rfc3339(&row.try_get::<String, _>("created_at").unwrap())
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                updated_at: chrono::DateTime::parse_from_rfc3339(&row.try_get::<String, _>("updated_at").unwrap())
                    .unwrap()
                    .with_timezone(&chrono::Utc),
            });
        }

        Ok(providers)
    }

    /// Update a provider.
    pub async fn update_provider(&self, id: &str, updates: ProviderUpdate) -> Result<bool, String> {
        let now = chrono::Utc::now().to_rfc3339();

        let mut query = "UPDATE providers SET updated_at = ?".to_string();
        let mut binds: Vec<String> = vec![now];

        if let Some(name) = updates.name {
            query.push_str(", name = ?");
            binds.push(name);
        }
        if let Some(provider_type) = updates.provider_type {
            query.push_str(", provider_type = ?");
            binds.push(provider_type);
        }
        if let Some(base_url) = updates.base_url {
            query.push_str(", base_url = ?");
            binds.push(base_url);
        }
        if let Some(api_key_id) = updates.api_key_id {
            query.push_str(", api_key_id = ?");
            binds.push(api_key_id);
        }
        if let Some(headers) = updates.headers {
            let headers_json = serde_json::to_string(&headers).unwrap_or_else(|_| "{}".to_string());
            query.push_str(", headers = ?");
            binds.push(headers_json);
        }
        if let Some(models) = updates.models {
            let models_json = serde_json::to_string(&models).unwrap_or_else(|_| "[]".to_string());
            query.push_str(", models = ?");
            binds.push(models_json);
        }
        if let Some(enabled) = updates.enabled {
            query.push_str(", enabled = ?");
            binds.push(if enabled { "1" } else { "0" }.to_string());
        }

        query.push_str(" WHERE id = ?");
        binds.push(id.to_string());

        let mut q = sqlx::query(&query);
        for bind in binds {
            q = q.bind(bind);
        }

        let result = q.execute(&self.pool)
            .await
            .map_err(|e| format!("Failed to update provider: {e}"))?;

        Ok(result.rows_affected() > 0)
    }

    /// Delete a provider.
    pub async fn delete_provider(&self, id: &str) -> Result<bool, String> {
        let result = sqlx::query("DELETE FROM providers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| format!("Failed to delete provider: {e}"))?;

        Ok(result.rows_affected() > 0)
    }

    /// Get the global keystore instance.
    pub fn global() -> Option<&'static Keystore> {
        KEYSTORE_INSTANCE.get()
    }
}

/// Update structure for provider modifications.
#[derive(Default)]
pub struct ProviderUpdate {
    pub name: Option<String>,
    pub provider_type: Option<String>,
    pub base_url: Option<String>,
    pub api_key_id: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub models: Option<Vec<String>>,
    pub enabled: Option<bool>,
}

/// Initialize the keystore from environment variable or prompt.
pub async fn init_keystore() -> Result<Keystore, String> {
    // Try to get passphrase from environment
    let passphrase = std::env::var("BKG_KEYSTORE_PASSPHRASE")
        .or_else(|_| std::env::var("BKG_KEYSTORE_PASSWORD"))
        .map_err(|_| "Keystore passphrase not set. Set BKG_KEYSTORE_PASSPHRASE environment variable.".to_string())?;

    Keystore::init(&passphrase).await
}