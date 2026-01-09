# AURA Keystore — Spec 05

**Status**: Design-ready  
**Builds on**: spec-01-aura.md, spec-02-interactive-runtime.md  
**Goal**: Secure key storage for agents to access third-party services

---

## 1) Purpose

Build a secure key management system that allows agents to store and use various types of credentials:

* **API Keys** — Access tokens for third-party AI models (Anthropic, OpenAI, etc.), cloud services, and APIs
* **Wallet Keys** — Private keys for cryptocurrency wallets and blockchain interactions
* **SSH Keys** — Authentication keys for remote servers and Git operations
* **Generic Secrets** — Other sensitive credentials (database passwords, service tokens, etc.)

### Why This Matters

1. **Agent Autonomy** — Each agent can have its own API keys, enabling independent billing and rate limits
2. **Security Isolation** — Keys are isolated per-agent, preventing cross-agent credential leakage
3. **Multi-Provider Support** — Agents can use different AI providers without shared configuration
4. **Wallet Integration** — Foundation for agents to interact with blockchain and DeFi
5. **Operational Security** — Keys encrypted at rest, audited access, secure rotation

---

## 2) Architecture

### 2.1 Updated Crate Layout

```
aura/
├─ aura-core              # IDs, schemas, hashing (unchanged)
├─ aura-store             # RocksDB storage (unchanged)
├─ aura-kernel            # Deterministic kernel (uses keystore)
├─ aura-swarm             # Router, scheduler, workers (unchanged)
├─ aura-reasoner          # Provider interface (gets keys from keystore)
├─ aura-executor          # Executor trait (unchanged)
├─ aura-tools             # ToolExecutor (SSH tools use keystore)
├─ aura-stats             # Stats collection (unchanged)
├─ aura-keystore          # NEW: Secure key storage and management
├─ aura-terminal          # Terminal UI (unchanged)
├─ aura-cli               # CLI (key management commands)
└─ aura-gateway-ts        # DEPRECATED
```

### 2.2 Component Diagram

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           Key Consumers                                  │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────────────────┐  │
│  │aura-reasoner │    │ aura-tools   │    │     aura-kernel          │  │
│  │              │    │              │    │                          │  │
│  │ • Model APIs │    │ • SSH ops    │    │ • Policy checks          │  │
│  │ • Provider   │    │ • Git auth   │    │ • Key access audit       │  │
│  │   selection  │    │ • Web APIs   │    │ • Rotation triggers      │  │
│  └──────┬───────┘    └──────┬───────┘    └────────────┬─────────────┘  │
│         │                   │                         │                 │
│         └───────────────────┼─────────────────────────┘                 │
│                             ▼                                           │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │                        aura-keystore                              │  │
│  │                                                                   │  │
│  │  ┌─────────────────────────────────────────────────────────────┐ │  │
│  │  │                    KeyStore Trait                            │ │  │
│  │  │  • store_key()     • get_key()      • delete_key()          │ │  │
│  │  │  • list_keys()     • rotate_key()   • check_permission()    │ │  │
│  │  └─────────────────────────────────────────────────────────────┘ │  │
│  │                             │                                     │  │
│  │           ┌─────────────────┼─────────────────┐                  │  │
│  │           ▼                 ▼                 ▼                  │  │
│  │  ┌─────────────┐   ┌─────────────┐   ┌─────────────────────┐   │  │
│  │  │  Encrypted  │   │   Envelope  │   │    Audit Logger     │   │  │
│  │  │   Storage   │   │  Encryption │   │                     │   │  │
│  │  │ (RocksDB)   │   │  (AES-GCM)  │   │  • Access logs      │   │  │
│  │  └─────────────┘   └─────────────┘   │  • Rotation logs    │   │  │
│  │                                       │  • Deletion logs    │   │  │
│  │                                       └─────────────────────┘   │  │
│  └──────────────────────────────────────────────────────────────────┘  │
│                             │                                           │
│                             ▼                                           │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │                     External Systems                              │  │
│  │  • Anthropic API    • OpenAI API     • Blockchain RPC            │  │
│  │  • SSH Servers      • GitHub/GitLab  • Cloud Services            │  │
│  └──────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

### 2.3 Security Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        Key Encryption Flow                               │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│   Plaintext Key                                                         │
│        │                                                                │
│        ▼                                                                │
│   ┌─────────────────────┐                                              │
│   │  Per-Key DEK        │  Data Encryption Key (random per key)        │
│   │  AES-256-GCM        │                                              │
│   └──────────┬──────────┘                                              │
│              │                                                          │
│              ▼                                                          │
│   ┌─────────────────────┐                                              │
│   │  Encrypted Key Blob │  Stored in RocksDB                           │
│   └─────────────────────┘                                              │
│                                                                         │
│   ┌─────────────────────┐                                              │
│   │  Master KEK         │  Key Encryption Key (derived from master)    │
│   │  (wraps DEKs)       │                                              │
│   └──────────┬──────────┘                                              │
│              │                                                          │
│              ▼                                                          │
│   ┌─────────────────────┐                                              │
│   │  Master Secret      │  From env var / file / KMS (future)          │
│   │  AURA_MASTER_KEY    │                                              │
│   └─────────────────────┘                                              │
│                                                                         │
│   Key Derivation: HKDF-SHA256(master_secret, agent_id, key_id)         │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 3) Data Model

### 3.1 Core Types

```rust
// aura-keystore/src/types.rs

use aura_core::AgentId;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Unique identifier for a stored key
pub type KeyId = [u8; 16];

/// Type of key being stored
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyType {
    /// API key for AI model providers
    ApiKey,
    /// Private key for cryptocurrency wallet
    WalletPrivateKey,
    /// SSH private key
    SshPrivateKey,
    /// SSH public key (stored for reference)
    SshPublicKey,
    /// Generic secret/token
    Secret,
    /// OAuth refresh token
    OAuthToken,
    /// Database connection string/password
    DatabaseCredential,
}

/// Provider/service the key is for
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyProvider {
    /// Anthropic Claude API
    Anthropic,
    /// OpenAI API
    OpenAI,
    /// Google AI (Gemini)
    Google,
    /// Mistral AI
    Mistral,
    /// AWS services
    Aws,
    /// Generic SSH
    Ssh,
    /// Ethereum/EVM wallet
    Ethereum,
    /// Solana wallet
    Solana,
    /// Bitcoin wallet
    Bitcoin,
    /// GitHub
    GitHub,
    /// GitLab
    GitLab,
    /// Custom provider
    Custom(String),
}

/// Metadata about a stored key (does not contain the secret)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyMetadata {
    /// Unique key identifier
    pub key_id: KeyId,
    /// Agent that owns this key
    pub agent_id: AgentId,
    /// Type of key
    pub key_type: KeyType,
    /// Provider/service this key is for
    pub provider: KeyProvider,
    /// Human-readable label
    pub label: String,
    /// Creation timestamp (Unix ms)
    pub created_at_ms: u64,
    /// Last rotation timestamp (Unix ms)
    pub rotated_at_ms: Option<u64>,
    /// Expiration timestamp (Unix ms), if applicable
    pub expires_at_ms: Option<u64>,
    /// Last accessed timestamp (Unix ms)
    pub last_accessed_ms: Option<u64>,
    /// Access count
    pub access_count: u64,
    /// Whether the key is currently active
    pub active: bool,
    /// Optional tags for organization
    pub tags: Vec<String>,
    /// Provider-specific metadata (e.g., wallet address, key fingerprint)
    pub extra: serde_json::Value,
}

/// A stored key with its encrypted value
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredKey {
    /// Key metadata
    pub metadata: KeyMetadata,
    /// Encrypted key material
    pub encrypted_value: EncryptedBlob,
    /// Version of the encryption scheme
    pub encryption_version: u32,
}

/// Encrypted blob containing key material
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedBlob {
    /// Ciphertext (AES-256-GCM encrypted)
    pub ciphertext: Vec<u8>,
    /// Nonce/IV for AES-GCM (12 bytes)
    pub nonce: [u8; 12],
    /// Authentication tag (16 bytes)
    pub tag: [u8; 16],
    /// Key derivation salt (for HKDF)
    pub salt: [u8; 32],
}

/// Decrypted key material (zeroized on drop)
#[derive(Clone, Zeroize)]
#[zeroize(drop)]
pub struct KeyMaterial {
    /// The actual key bytes
    value: Vec<u8>,
}

impl KeyMaterial {
    /// Create new key material
    pub fn new(value: Vec<u8>) -> Self {
        Self { value }
    }

    /// Get the key value as bytes
    pub fn as_bytes(&self) -> &[u8] {
        &self.value
    }

    /// Get the key value as a string (for API keys)
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.value).ok()
    }
}

impl std::fmt::Debug for KeyMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyMaterial")
            .field("value", &"[REDACTED]")
            .finish()
    }
}
```

### 3.2 Request/Response Types

```rust
/// Request to store a new key
#[derive(Debug)]
pub struct StoreKeyRequest {
    /// Agent storing the key
    pub agent_id: AgentId,
    /// Type of key
    pub key_type: KeyType,
    /// Provider/service
    pub provider: KeyProvider,
    /// Human-readable label
    pub label: String,
    /// The key material (will be encrypted)
    pub key_material: KeyMaterial,
    /// Optional expiration (Unix ms)
    pub expires_at_ms: Option<u64>,
    /// Optional tags
    pub tags: Vec<String>,
    /// Provider-specific metadata
    pub extra: Option<serde_json::Value>,
}

/// Request to retrieve a key
#[derive(Debug, Clone)]
pub struct GetKeyRequest {
    /// Agent requesting the key
    pub agent_id: AgentId,
    /// Key to retrieve
    pub key_id: KeyId,
    /// Reason for access (for audit logging)
    pub access_reason: String,
}

/// Query for listing keys
#[derive(Debug, Clone, Default)]
pub struct ListKeysQuery {
    /// Filter by agent
    pub agent_id: Option<AgentId>,
    /// Filter by key type
    pub key_type: Option<KeyType>,
    /// Filter by provider
    pub provider: Option<KeyProvider>,
    /// Filter by active status
    pub active: Option<bool>,
    /// Filter by tags (any match)
    pub tags: Option<Vec<String>>,
    /// Maximum results
    pub limit: Option<usize>,
}

/// Response containing a decrypted key
#[derive(Debug)]
pub struct KeyResponse {
    /// Key metadata
    pub metadata: KeyMetadata,
    /// Decrypted key material
    pub material: KeyMaterial,
}

/// Key rotation result
#[derive(Debug)]
pub struct RotationResult {
    /// Old key ID (now inactive)
    pub old_key_id: KeyId,
    /// New key ID (now active)
    pub new_key_id: KeyId,
    /// Rotation timestamp
    pub rotated_at_ms: u64,
}
```

### 3.3 Audit Types

```rust
/// Audit event for key operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyAuditEvent {
    /// Event ID
    pub event_id: [u8; 16],
    /// Timestamp (Unix ms)
    pub timestamp_ms: u64,
    /// Agent that owns the key
    pub agent_id: AgentId,
    /// Key ID (if applicable)
    pub key_id: Option<KeyId>,
    /// Type of operation
    pub operation: KeyOperation,
    /// Whether the operation succeeded
    pub success: bool,
    /// Reason provided (for get operations)
    pub reason: Option<String>,
    /// Error message (if failed)
    pub error: Option<String>,
    /// Additional context
    pub context: serde_json::Value,
}

/// Types of key operations
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyOperation {
    /// Key created
    Create,
    /// Key retrieved (decrypted)
    Access,
    /// Key updated (metadata only)
    Update,
    /// Key rotated
    Rotate,
    /// Key deleted
    Delete,
    /// Key listed (metadata only)
    List,
    /// Access denied
    AccessDenied,
    /// Key expired
    Expire,
}
```

### 3.4 Permission Types

```rust
/// Permission level for key access
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyPermission {
    /// Can only see key exists (metadata.label, type, provider)
    Discover,
    /// Can read full metadata
    ReadMetadata,
    /// Can use the key (decrypt for use)
    Use,
    /// Can manage the key (rotate, update, delete)
    Manage,
    /// Full control (all permissions)
    Admin,
}

/// Access control entry for a key
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyAccessControl {
    /// Key ID
    pub key_id: KeyId,
    /// Owner agent (always has Admin)
    pub owner_agent_id: AgentId,
    /// Delegated permissions to other agents
    pub delegations: Vec<KeyDelegation>,
}

/// Delegated access to a key
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyDelegation {
    /// Agent granted access
    pub agent_id: AgentId,
    /// Permission level
    pub permission: KeyPermission,
    /// Expiration (Unix ms)
    pub expires_at_ms: Option<u64>,
    /// Who granted this delegation
    pub granted_by: AgentId,
    /// When granted (Unix ms)
    pub granted_at_ms: u64,
}
```

---

## 4) Storage Schema

### 4.1 Column Families

Add to `aura-store` (or use dedicated keystore DB):

```rust
// New column families for keystore
const CF_KEYS: &str = "keys";              // Encrypted key storage
const CF_KEY_META: &str = "key_meta";      // Key metadata (searchable)
const CF_KEY_ACL: &str = "key_acl";        // Access control lists
const CF_KEY_AUDIT: &str = "key_audit";    // Audit log
const CF_KEY_INDEX: &str = "key_index";    // Secondary indexes
```

### 4.2 Key Schemas

```
keys (encrypted key material):
  Key:   K | agent_id(32) | key_id(16)
  Value: StoredKey (encrypted, CBOR)

key_meta (metadata for queries):
  Key:   M | agent_id(32) | key_id(16)
  Value: KeyMetadata (CBOR)

key_acl (access control):
  Key:   A | key_id(16)
  Value: KeyAccessControl (CBOR)

key_audit (audit log):
  Key:   L | timestamp_ms(u64be) | event_id(16)
  Value: KeyAuditEvent (CBOR)

key_index (secondary indexes):
  Provider index:
    Key:   I | P | provider(padded 32) | agent_id(32) | key_id(16)
    Value: (empty, existence check only)
  
  Type index:
    Key:   I | T | key_type(u8) | agent_id(32) | key_id(16)
    Value: (empty)
  
  Label index (for search):
    Key:   I | L | agent_id(32) | label_lowercase(var) | key_id(16)
    Value: (empty)
```

### 4.3 Encryption Configuration

```rust
/// Encryption configuration for the keystore
#[derive(Debug, Clone)]
pub struct EncryptionConfig {
    /// Master key source
    pub master_key_source: MasterKeySource,
    /// Encryption algorithm version
    pub algorithm_version: u32,
    /// Key derivation iterations (for PBKDF2 if used)
    pub kdf_iterations: u32,
}

/// Source of the master encryption key
#[derive(Debug, Clone)]
pub enum MasterKeySource {
    /// From environment variable
    EnvVar(String),
    /// From file path
    File(PathBuf),
    /// From AWS KMS (future)
    AwsKms { key_id: String },
    /// From HashiCorp Vault (future)
    Vault { path: String },
    /// Derived from passphrase (dev only)
    Passphrase(String),
}

impl Default for EncryptionConfig {
    fn default() -> Self {
        Self {
            master_key_source: MasterKeySource::EnvVar("AURA_MASTER_KEY".into()),
            algorithm_version: 1,
            kdf_iterations: 100_000,
        }
    }
}
```

---

## 5) KeyStore Interface

### 5.1 Core Trait

```rust
// aura-keystore/src/lib.rs

use async_trait::async_trait;
use crate::types::*;
use crate::error::KeystoreError;

/// Secure key storage interface
#[async_trait]
pub trait KeyStore: Send + Sync {
    // === Key Storage ===
    
    /// Store a new key
    ///
    /// # Errors
    ///
    /// Returns an error if encryption fails, storage fails, or a key with
    /// the same label already exists for this agent.
    async fn store_key(&self, request: StoreKeyRequest) -> Result<KeyMetadata, KeystoreError>;
    
    /// Retrieve a key (decrypts the key material)
    ///
    /// # Errors
    ///
    /// Returns an error if the key doesn't exist, the agent doesn't have
    /// permission, or decryption fails.
    async fn get_key(&self, request: GetKeyRequest) -> Result<KeyResponse, KeystoreError>;
    
    /// Get key metadata only (no decryption)
    ///
    /// # Errors
    ///
    /// Returns an error if the key doesn't exist or the agent lacks
    /// `ReadMetadata` permission.
    async fn get_key_metadata(
        &self,
        agent_id: AgentId,
        key_id: KeyId,
    ) -> Result<KeyMetadata, KeystoreError>;
    
    /// List keys matching query (metadata only)
    async fn list_keys(&self, query: ListKeysQuery) -> Result<Vec<KeyMetadata>, KeystoreError>;
    
    /// Update key metadata (not the key material)
    async fn update_key_metadata(
        &self,
        agent_id: AgentId,
        key_id: KeyId,
        update: KeyMetadataUpdate,
    ) -> Result<KeyMetadata, KeystoreError>;
    
    /// Delete a key (secure deletion)
    async fn delete_key(
        &self,
        agent_id: AgentId,
        key_id: KeyId,
        reason: String,
    ) -> Result<(), KeystoreError>;
    
    // === Key Rotation ===
    
    /// Rotate a key (create new, mark old as inactive)
    async fn rotate_key(
        &self,
        agent_id: AgentId,
        key_id: KeyId,
        new_material: KeyMaterial,
    ) -> Result<RotationResult, KeystoreError>;
    
    // === Access Control ===
    
    /// Check if an agent has permission to access a key
    async fn check_permission(
        &self,
        agent_id: AgentId,
        key_id: KeyId,
        required: KeyPermission,
    ) -> Result<bool, KeystoreError>;
    
    /// Grant permission to another agent
    async fn grant_permission(
        &self,
        owner_agent_id: AgentId,
        key_id: KeyId,
        delegation: KeyDelegation,
    ) -> Result<(), KeystoreError>;
    
    /// Revoke permission from an agent
    async fn revoke_permission(
        &self,
        owner_agent_id: AgentId,
        key_id: KeyId,
        target_agent_id: AgentId,
    ) -> Result<(), KeystoreError>;
    
    // === Convenience Methods ===
    
    /// Get an API key for a specific provider
    async fn get_api_key(
        &self,
        agent_id: AgentId,
        provider: KeyProvider,
        reason: String,
    ) -> Result<KeyResponse, KeystoreError>;
    
    /// Get the default/primary key for a provider
    async fn get_default_key(
        &self,
        agent_id: AgentId,
        provider: KeyProvider,
    ) -> Result<Option<KeyMetadata>, KeystoreError>;
    
    // === Audit ===
    
    /// Get audit log for a key
    async fn get_key_audit_log(
        &self,
        agent_id: AgentId,
        key_id: KeyId,
        from_ms: Option<u64>,
        to_ms: Option<u64>,
        limit: Option<usize>,
    ) -> Result<Vec<KeyAuditEvent>, KeystoreError>;
    
    /// Get all audit events for an agent
    async fn get_agent_audit_log(
        &self,
        agent_id: AgentId,
        from_ms: Option<u64>,
        to_ms: Option<u64>,
        limit: Option<usize>,
    ) -> Result<Vec<KeyAuditEvent>, KeystoreError>;
}
```

### 5.2 Metadata Update Type

```rust
/// Fields that can be updated on key metadata
#[derive(Debug, Clone, Default)]
pub struct KeyMetadataUpdate {
    /// New label
    pub label: Option<String>,
    /// New tags
    pub tags: Option<Vec<String>>,
    /// New expiration
    pub expires_at_ms: Option<Option<u64>>,
    /// New active status
    pub active: Option<bool>,
    /// Updated extra metadata
    pub extra: Option<serde_json::Value>,
}
```

---

## 6) Encryption Implementation

### 6.1 Encryption Service

```rust
// aura-keystore/src/encryption.rs

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroize;

/// Service for encrypting/decrypting key material
pub struct EncryptionService {
    master_key: MasterKey,
    config: EncryptionConfig,
}

/// Master key (zeroized on drop)
#[derive(Zeroize)]
#[zeroize(drop)]
struct MasterKey {
    bytes: [u8; 32],
}

impl EncryptionService {
    /// Create encryption service with master key from config
    ///
    /// # Errors
    ///
    /// Returns an error if the master key cannot be loaded from the
    /// configured source.
    pub fn new(config: EncryptionConfig) -> Result<Self, KeystoreError> {
        let master_key = Self::load_master_key(&config.master_key_source)?;
        Ok(Self { master_key, config })
    }
    
    fn load_master_key(source: &MasterKeySource) -> Result<MasterKey, KeystoreError> {
        let bytes: [u8; 32] = match source {
            MasterKeySource::EnvVar(var) => {
                let value = std::env::var(var)
                    .map_err(|_| KeystoreError::MasterKeyNotFound {
                        source: format!("env:{var}"),
                    })?;
                
                // Expect hex-encoded 32 bytes
                let decoded = hex::decode(&value)
                    .map_err(|e| KeystoreError::InvalidMasterKey {
                        reason: format!("invalid hex: {e}"),
                    })?;
                
                decoded.try_into()
                    .map_err(|_| KeystoreError::InvalidMasterKey {
                        reason: "expected 32 bytes".into(),
                    })?
            }
            
            MasterKeySource::File(path) => {
                let content = std::fs::read_to_string(path)
                    .map_err(|e| KeystoreError::MasterKeyNotFound {
                        source: format!("file:{}: {e}", path.display()),
                    })?;
                
                let decoded = hex::decode(content.trim())
                    .map_err(|e| KeystoreError::InvalidMasterKey {
                        reason: format!("invalid hex in file: {e}"),
                    })?;
                
                decoded.try_into()
                    .map_err(|_| KeystoreError::InvalidMasterKey {
                        reason: "expected 32 bytes".into(),
                    })?
            }
            
            MasterKeySource::Passphrase(passphrase) => {
                // Derive key from passphrase using PBKDF2
                // WARNING: Only for development!
                tracing::warn!("using passphrase-derived master key (dev only)");
                
                let salt = b"aura-keystore-dev-salt";
                let mut key = [0u8; 32];
                pbkdf2::pbkdf2_hmac::<Sha256>(
                    passphrase.as_bytes(),
                    salt,
                    100_000,
                    &mut key,
                );
                key
            }
            
            MasterKeySource::AwsKms { .. } | MasterKeySource::Vault { .. } => {
                return Err(KeystoreError::NotImplemented {
                    feature: "KMS/Vault integration".into(),
                });
            }
        };
        
        Ok(MasterKey { bytes })
    }
    
    /// Derive a per-key encryption key
    fn derive_key(
        &self,
        agent_id: &AgentId,
        key_id: &KeyId,
        salt: &[u8; 32],
    ) -> [u8; 32] {
        let hk = Hkdf::<Sha256>::new(Some(salt), &self.master_key.bytes);
        
        // Context includes agent_id and key_id for domain separation
        let mut info = Vec::with_capacity(64);
        info.extend_from_slice(b"aura-keystore-v1:");
        info.extend_from_slice(agent_id);
        info.extend_from_slice(key_id);
        
        let mut derived = [0u8; 32];
        hk.expand(&info, &mut derived)
            .expect("32 bytes is valid for HKDF");
        derived
    }
    
    /// Encrypt key material
    pub fn encrypt(
        &self,
        agent_id: &AgentId,
        key_id: &KeyId,
        material: &KeyMaterial,
    ) -> Result<EncryptedBlob, KeystoreError> {
        // Generate random salt and nonce
        let mut salt = [0u8; 32];
        let mut nonce_bytes = [0u8; 12];
        getrandom::getrandom(&mut salt)?;
        getrandom::getrandom(&mut nonce_bytes)?;
        
        // Derive encryption key
        let derived_key = self.derive_key(agent_id, key_id, &salt);
        
        // Encrypt with AES-256-GCM
        let cipher = Aes256Gcm::new_from_slice(&derived_key)
            .map_err(|e| KeystoreError::EncryptionFailed {
                reason: format!("cipher init: {e}"),
            })?;
        
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, material.as_bytes())
            .map_err(|e| KeystoreError::EncryptionFailed {
                reason: format!("encryption: {e}"),
            })?;
        
        // Extract tag (last 16 bytes of ciphertext in AES-GCM)
        let (ct, tag_slice) = ciphertext.split_at(ciphertext.len() - 16);
        let mut tag = [0u8; 16];
        tag.copy_from_slice(tag_slice);
        
        Ok(EncryptedBlob {
            ciphertext: ct.to_vec(),
            nonce: nonce_bytes,
            tag,
            salt,
        })
    }
    
    /// Decrypt key material
    pub fn decrypt(
        &self,
        agent_id: &AgentId,
        key_id: &KeyId,
        blob: &EncryptedBlob,
    ) -> Result<KeyMaterial, KeystoreError> {
        // Derive encryption key
        let derived_key = self.derive_key(agent_id, key_id, &blob.salt);
        
        // Reconstruct ciphertext with tag
        let mut ciphertext_with_tag = blob.ciphertext.clone();
        ciphertext_with_tag.extend_from_slice(&blob.tag);
        
        // Decrypt with AES-256-GCM
        let cipher = Aes256Gcm::new_from_slice(&derived_key)
            .map_err(|e| KeystoreError::DecryptionFailed {
                reason: format!("cipher init: {e}"),
            })?;
        
        let nonce = Nonce::from_slice(&blob.nonce);
        let plaintext = cipher
            .decrypt(nonce, ciphertext_with_tag.as_ref())
            .map_err(|e| KeystoreError::DecryptionFailed {
                reason: format!("decryption: {e}"),
            })?;
        
        Ok(KeyMaterial::new(plaintext))
    }
}
```

---

## 7) RocksDB Implementation

### 7.1 RocksDB KeyStore

```rust
// aura-keystore/src/rocks_store.rs

use crate::{
    types::*, encryption::EncryptionService, KeyStore, KeystoreError,
};
use aura_core::AgentId;
use async_trait::async_trait;
use rocksdb::{DB, WriteBatch, IteratorMode};
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct RocksKeyStore {
    db: Arc<DB>,
    encryption: EncryptionService,
    audit_enabled: bool,
}

impl RocksKeyStore {
    /// Open or create a keystore database
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be opened or encryption
    /// cannot be initialized.
    pub fn open(path: &Path, config: KeystoreConfig) -> Result<Self, KeystoreError> {
        let mut opts = rocksdb::Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        
        let cfs = [CF_KEYS, CF_KEY_META, CF_KEY_ACL, CF_KEY_AUDIT, CF_KEY_INDEX];
        
        let db = DB::open_cf(&opts, path, cfs)
            .map_err(|e| KeystoreError::StorageError {
                operation: "open",
                source: e.into(),
            })?;
        
        let encryption = EncryptionService::new(config.encryption)?;
        
        Ok(Self {
            db: Arc::new(db),
            encryption,
            audit_enabled: config.audit_enabled,
        })
    }
    
    /// Generate a new key ID
    fn generate_key_id() -> KeyId {
        let mut id = [0u8; 16];
        getrandom::getrandom(&mut id).expect("getrandom failed");
        id
    }
    
    /// Build key prefix for agent's keys
    fn key_prefix(agent_id: &AgentId) -> Vec<u8> {
        let mut prefix = Vec::with_capacity(33);
        prefix.push(b'K');
        prefix.extend_from_slice(agent_id);
        prefix
    }
    
    /// Build full key for a specific key
    fn key_key(agent_id: &AgentId, key_id: &KeyId) -> Vec<u8> {
        let mut key = Self::key_prefix(agent_id);
        key.extend_from_slice(key_id);
        key
    }
    
    /// Log an audit event
    async fn audit_log(&self, event: KeyAuditEvent) -> Result<(), KeystoreError> {
        if !self.audit_enabled {
            return Ok(());
        }
        
        let cf = self.db.cf_handle(CF_KEY_AUDIT)
            .ok_or_else(|| KeystoreError::StorageError {
                operation: "audit_log",
                source: anyhow::anyhow!("missing CF"),
            })?;
        
        let mut key = Vec::with_capacity(25);
        key.push(b'L');
        key.extend_from_slice(&event.timestamp_ms.to_be_bytes());
        key.extend_from_slice(&event.event_id);
        
        let value = serde_cbor::to_vec(&event)
            .map_err(|e| KeystoreError::SerializationFailed { source: e.into() })?;
        
        self.db.put_cf(&cf, key, value)
            .map_err(|e| KeystoreError::StorageError {
                operation: "audit_log",
                source: e.into(),
            })?;
        
        Ok(())
    }
}

#[async_trait]
impl KeyStore for RocksKeyStore {
    async fn store_key(&self, request: StoreKeyRequest) -> Result<KeyMetadata, KeystoreError> {
        let key_id = Self::generate_key_id();
        let now_ms = now_ms();
        
        // Create metadata
        let metadata = KeyMetadata {
            key_id,
            agent_id: request.agent_id,
            key_type: request.key_type,
            provider: request.provider.clone(),
            label: request.label.clone(),
            created_at_ms: now_ms,
            rotated_at_ms: None,
            expires_at_ms: request.expires_at_ms,
            last_accessed_ms: None,
            access_count: 0,
            active: true,
            tags: request.tags.clone(),
            extra: request.extra.unwrap_or(serde_json::Value::Null),
        };
        
        // Encrypt key material
        let encrypted = self.encryption.encrypt(
            &request.agent_id,
            &key_id,
            &request.key_material,
        )?;
        
        // Create stored key
        let stored_key = StoredKey {
            metadata: metadata.clone(),
            encrypted_value: encrypted,
            encryption_version: 1,
        };
        
        // Create default ACL (owner has admin)
        let acl = KeyAccessControl {
            key_id,
            owner_agent_id: request.agent_id,
            delegations: vec![],
        };
        
        // Write atomically
        let mut batch = WriteBatch::default();
        
        let cf_keys = self.db.cf_handle(CF_KEYS).unwrap();
        let cf_meta = self.db.cf_handle(CF_KEY_META).unwrap();
        let cf_acl = self.db.cf_handle(CF_KEY_ACL).unwrap();
        let cf_index = self.db.cf_handle(CF_KEY_INDEX).unwrap();
        
        let key_key = Self::key_key(&request.agent_id, &key_id);
        batch.put_cf(&cf_keys, &key_key, serde_cbor::to_vec(&stored_key)?);
        batch.put_cf(&cf_meta, &key_key, serde_cbor::to_vec(&metadata)?);
        batch.put_cf(&cf_acl, &key_id[..], serde_cbor::to_vec(&acl)?);
        
        // Provider index
        let provider_key = build_provider_index_key(
            &request.provider,
            &request.agent_id,
            &key_id,
        );
        batch.put_cf(&cf_index, provider_key, &[]);
        
        // Type index
        let type_key = build_type_index_key(
            request.key_type,
            &request.agent_id,
            &key_id,
        );
        batch.put_cf(&cf_index, type_key, &[]);
        
        self.db.write(batch)
            .map_err(|e| KeystoreError::StorageError {
                operation: "store_key",
                source: e.into(),
            })?;
        
        // Audit log
        self.audit_log(KeyAuditEvent {
            event_id: Self::generate_key_id(),
            timestamp_ms: now_ms,
            agent_id: request.agent_id,
            key_id: Some(key_id),
            operation: KeyOperation::Create,
            success: true,
            reason: None,
            error: None,
            context: serde_json::json!({
                "label": request.label,
                "provider": format!("{:?}", request.provider),
                "key_type": format!("{:?}", request.key_type),
            }),
        }).await?;
        
        Ok(metadata)
    }
    
    async fn get_key(&self, request: GetKeyRequest) -> Result<KeyResponse, KeystoreError> {
        // Check permission
        let has_permission = self.check_permission(
            request.agent_id,
            request.key_id,
            KeyPermission::Use,
        ).await?;
        
        if !has_permission {
            self.audit_log(KeyAuditEvent {
                event_id: Self::generate_key_id(),
                timestamp_ms: now_ms(),
                agent_id: request.agent_id,
                key_id: Some(request.key_id),
                operation: KeyOperation::AccessDenied,
                success: false,
                reason: Some(request.access_reason.clone()),
                error: Some("permission denied".into()),
                context: serde_json::Value::Null,
            }).await?;
            
            return Err(KeystoreError::PermissionDenied {
                agent_id: hex::encode(request.agent_id),
                key_id: hex::encode(request.key_id),
                required: KeyPermission::Use,
            });
        }
        
        // Load stored key
        let cf_keys = self.db.cf_handle(CF_KEYS).unwrap();
        let key_key = Self::key_key(&request.agent_id, &request.key_id);
        
        let stored_bytes = self.db.get_cf(&cf_keys, &key_key)
            .map_err(|e| KeystoreError::StorageError {
                operation: "get_key",
                source: e.into(),
            })?
            .ok_or_else(|| KeystoreError::KeyNotFound {
                key_id: hex::encode(request.key_id),
            })?;
        
        let stored_key: StoredKey = serde_cbor::from_slice(&stored_bytes)
            .map_err(|e| KeystoreError::DeserializationFailed { source: e.into() })?;
        
        // Check if expired
        if let Some(expires_at) = stored_key.metadata.expires_at_ms {
            if now_ms() > expires_at {
                return Err(KeystoreError::KeyExpired {
                    key_id: hex::encode(request.key_id),
                    expired_at_ms: expires_at,
                });
            }
        }
        
        // Decrypt
        let material = self.encryption.decrypt(
            &request.agent_id,
            &request.key_id,
            &stored_key.encrypted_value,
        )?;
        
        // Update access metadata
        let mut metadata = stored_key.metadata.clone();
        metadata.last_accessed_ms = Some(now_ms());
        metadata.access_count += 1;
        
        // Update metadata in DB
        let cf_meta = self.db.cf_handle(CF_KEY_META).unwrap();
        self.db.put_cf(&cf_meta, &key_key, serde_cbor::to_vec(&metadata)?)
            .map_err(|e| KeystoreError::StorageError {
                operation: "update_access_metadata",
                source: e.into(),
            })?;
        
        // Audit log
        self.audit_log(KeyAuditEvent {
            event_id: Self::generate_key_id(),
            timestamp_ms: now_ms(),
            agent_id: request.agent_id,
            key_id: Some(request.key_id),
            operation: KeyOperation::Access,
            success: true,
            reason: Some(request.access_reason),
            error: None,
            context: serde_json::json!({
                "label": metadata.label,
                "provider": format!("{:?}", metadata.provider),
            }),
        }).await?;
        
        Ok(KeyResponse { metadata, material })
    }
    
    async fn get_api_key(
        &self,
        agent_id: AgentId,
        provider: KeyProvider,
        reason: String,
    ) -> Result<KeyResponse, KeystoreError> {
        // Find the default/active API key for this provider
        let query = ListKeysQuery {
            agent_id: Some(agent_id),
            key_type: Some(KeyType::ApiKey),
            provider: Some(provider.clone()),
            active: Some(true),
            ..Default::default()
        };
        
        let keys = self.list_keys(query).await?;
        
        let key = keys.first().ok_or_else(|| KeystoreError::NoKeyForProvider {
            provider: format!("{provider:?}"),
            agent_id: hex::encode(agent_id),
        })?;
        
        self.get_key(GetKeyRequest {
            agent_id,
            key_id: key.key_id,
            access_reason: reason,
        }).await
    }
    
    async fn check_permission(
        &self,
        agent_id: AgentId,
        key_id: KeyId,
        required: KeyPermission,
    ) -> Result<bool, KeystoreError> {
        let cf_acl = self.db.cf_handle(CF_KEY_ACL).unwrap();
        
        let acl_bytes = self.db.get_cf(&cf_acl, &key_id[..])
            .map_err(|e| KeystoreError::StorageError {
                operation: "check_permission",
                source: e.into(),
            })?
            .ok_or_else(|| KeystoreError::KeyNotFound {
                key_id: hex::encode(key_id),
            })?;
        
        let acl: KeyAccessControl = serde_cbor::from_slice(&acl_bytes)
            .map_err(|e| KeystoreError::DeserializationFailed { source: e.into() })?;
        
        // Owner always has all permissions
        if acl.owner_agent_id == agent_id {
            return Ok(true);
        }
        
        // Check delegations
        let now = now_ms();
        for delegation in &acl.delegations {
            if delegation.agent_id != agent_id {
                continue;
            }
            
            // Check expiration
            if let Some(expires_at) = delegation.expires_at_ms {
                if now > expires_at {
                    continue;
                }
            }
            
            // Check permission level
            if permission_includes(delegation.permission, required) {
                return Ok(true);
            }
        }
        
        Ok(false)
    }
    
    // ... additional method implementations ...
}

/// Check if granted permission includes required permission
fn permission_includes(granted: KeyPermission, required: KeyPermission) -> bool {
    match granted {
        KeyPermission::Admin => true,
        KeyPermission::Manage => matches!(
            required,
            KeyPermission::Discover
                | KeyPermission::ReadMetadata
                | KeyPermission::Use
                | KeyPermission::Manage
        ),
        KeyPermission::Use => matches!(
            required,
            KeyPermission::Discover | KeyPermission::ReadMetadata | KeyPermission::Use
        ),
        KeyPermission::ReadMetadata => matches!(
            required,
            KeyPermission::Discover | KeyPermission::ReadMetadata
        ),
        KeyPermission::Discover => required == KeyPermission::Discover,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time went backwards")
        .as_millis() as u64
}
```

---

## 8) Error Types

```rust
// aura-keystore/src/error.rs

use thiserror::Error;

/// Errors that can occur in keystore operations
#[derive(Error, Debug)]
pub enum KeystoreError {
    // === Key Access Errors ===
    
    #[error("key not found: {key_id}")]
    KeyNotFound { key_id: String },
    
    #[error("key expired: {key_id} (expired at {expired_at_ms})")]
    KeyExpired { key_id: String, expired_at_ms: u64 },
    
    #[error("no key found for provider {provider} for agent {agent_id}")]
    NoKeyForProvider { provider: String, agent_id: String },
    
    #[error("permission denied: agent {agent_id} lacks {required:?} permission for key {key_id}")]
    PermissionDenied {
        agent_id: String,
        key_id: String,
        required: KeyPermission,
    },
    
    // === Encryption Errors ===
    
    #[error("master key not found: {source}")]
    MasterKeyNotFound { source: String },
    
    #[error("invalid master key: {reason}")]
    InvalidMasterKey { reason: String },
    
    #[error("encryption failed: {reason}")]
    EncryptionFailed { reason: String },
    
    #[error("decryption failed: {reason}")]
    DecryptionFailed { reason: String },
    
    // === Storage Errors ===
    
    #[error("storage error during {operation}: {source}")]
    StorageError {
        operation: &'static str,
        #[source]
        source: anyhow::Error,
    },
    
    #[error("serialization failed: {source}")]
    SerializationFailed {
        #[source]
        source: anyhow::Error,
    },
    
    #[error("deserialization failed: {source}")]
    DeserializationFailed {
        #[source]
        source: anyhow::Error,
    },
    
    // === Validation Errors ===
    
    #[error("duplicate key label: {label}")]
    DuplicateLabel { label: String },
    
    #[error("invalid key material: {reason}")]
    InvalidKeyMaterial { reason: String },
    
    // === Other ===
    
    #[error("feature not implemented: {feature}")]
    NotImplemented { feature: String },
    
    #[error("random number generation failed: {0}")]
    RandomError(#[from] getrandom::Error),
}
```

---

## 9) Integration with Reasoner

### 9.1 Provider Configuration

```rust
// aura-reasoner/src/config.rs

use aura_keystore::{KeyStore, KeyProvider};

/// Model provider configuration
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Provider to use
    pub provider: KeyProvider,
    /// Model name
    pub model: String,
    /// API base URL (if custom)
    pub base_url: Option<String>,
    /// Key source
    pub key_source: KeySource,
}

/// Where to get the API key
#[derive(Debug, Clone)]
pub enum KeySource {
    /// From agent's keystore
    Keystore,
    /// From environment variable
    EnvVar(String),
    /// Explicit value (for testing only)
    Explicit(String),
}
```

### 9.2 Dynamic API Key Resolution

```rust
// aura-reasoner/src/anthropic.rs

use aura_keystore::{KeyStore, KeyProvider};

impl AnthropicProvider {
    /// Create provider that resolves API key from keystore
    pub fn with_keystore(
        keystore: Arc<dyn KeyStore>,
        agent_id: AgentId,
        config: ProviderConfig,
    ) -> Self {
        Self {
            keystore: Some(keystore),
            agent_id: Some(agent_id),
            config,
            cached_key: RwLock::new(None),
        }
    }
    
    /// Get the API key, resolving from keystore if needed
    async fn get_api_key(&self) -> anyhow::Result<String> {
        match &self.config.key_source {
            KeySource::Keystore => {
                let keystore = self.keystore.as_ref()
                    .ok_or_else(|| anyhow::anyhow!("keystore not configured"))?;
                let agent_id = self.agent_id
                    .ok_or_else(|| anyhow::anyhow!("agent_id not set"))?;
                
                let response = keystore.get_api_key(
                    agent_id,
                    KeyProvider::Anthropic,
                    "model completion".into(),
                ).await?;
                
                response.material.as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow::anyhow!("API key is not valid UTF-8"))
            }
            KeySource::EnvVar(var) => {
                std::env::var(var)
                    .map_err(|_| anyhow::anyhow!("env var {var} not set"))
            }
            KeySource::Explicit(key) => Ok(key.clone()),
        }
    }
}
```

---

## 10) CLI Integration

### 10.1 Key Management Commands

```rust
// aura-cli/src/commands/keys.rs

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum KeysCommand {
    /// Add a new key
    Add {
        /// Key type (api, wallet, ssh, secret)
        #[arg(short, long)]
        r#type: String,
        
        /// Provider (anthropic, openai, ethereum, etc.)
        #[arg(short, long)]
        provider: String,
        
        /// Human-readable label
        #[arg(short, long)]
        label: String,
        
        /// Read key from stdin (secure)
        #[arg(long)]
        stdin: bool,
        
        /// Read key from file
        #[arg(long)]
        file: Option<PathBuf>,
        
        /// Optional tags
        #[arg(long)]
        tags: Option<Vec<String>>,
    },
    
    /// List stored keys
    List {
        /// Filter by type
        #[arg(short, long)]
        r#type: Option<String>,
        
        /// Filter by provider
        #[arg(short, long)]
        provider: Option<String>,
        
        /// Show inactive keys
        #[arg(long)]
        all: bool,
    },
    
    /// Show key details (metadata only)
    Show {
        /// Key ID or label
        key: String,
    },
    
    /// Delete a key
    Delete {
        /// Key ID or label
        key: String,
        
        /// Skip confirmation
        #[arg(long)]
        force: bool,
    },
    
    /// Rotate a key
    Rotate {
        /// Key ID or label
        key: String,
        
        /// Read new key from stdin
        #[arg(long)]
        stdin: bool,
        
        /// Read new key from file
        #[arg(long)]
        file: Option<PathBuf>,
    },
    
    /// Test a key (verify it works)
    Test {
        /// Key ID or label
        key: String,
    },
    
    /// View audit log for a key
    Audit {
        /// Key ID or label (optional, shows all if not specified)
        key: Option<String>,
        
        /// Number of entries
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
}

// CLI output example for `keys list`:
// ┌─────────────────────────────────────────────────────────────────────────┐
// │ STORED KEYS                                                             │
// ├─────────────────────────────────────────────────────────────────────────┤
// │ ID       │ Label          │ Type   │ Provider  │ Status  │ Last Used   │
// ├──────────┼────────────────┼────────┼───────────┼─────────┼─────────────┤
// │ 7f3a...  │ anthropic-main │ api    │ anthropic │ active  │ 2 mins ago  │
// │ 2b9c...  │ openai-backup  │ api    │ openai    │ active  │ 1 hour ago  │
// │ 8e1d...  │ eth-wallet-1   │ wallet │ ethereum  │ active  │ never       │
// │ 4c2f...  │ deploy-key     │ ssh    │ github    │ active  │ 3 days ago  │
// └─────────────────────────────────────────────────────────────────────────┘
```

### 10.2 Secure Key Input

```rust
// aura-cli/src/input.rs

use std::io::{self, BufRead, Write};

/// Read a key securely (no echo)
pub fn read_key_secure(prompt: &str) -> io::Result<String> {
    // Use rpassword for secure input
    print!("{prompt}");
    io::stdout().flush()?;
    
    let key = rpassword::read_password()?;
    
    if key.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "key cannot be empty",
        ));
    }
    
    Ok(key)
}

/// Read a key from stdin (for piped input)
pub fn read_key_stdin() -> io::Result<String> {
    let stdin = io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

/// Read a key from file
pub fn read_key_file(path: &Path) -> io::Result<String> {
    let content = std::fs::read_to_string(path)?;
    Ok(content.trim().to_string())
}
```

---

## 11) Crate Structure

```
aura-keystore/
├── Cargo.toml
├── src/
│   ├── lib.rs                    # Public API exports
│   ├── types.rs                  # KeyMetadata, StoredKey, etc.
│   ├── error.rs                  # KeystoreError with thiserror
│   ├── encryption.rs             # EncryptionService (AES-GCM)
│   ├── store.rs                  # KeyStore trait
│   ├── rocks_store.rs            # RocksDB implementation
│   ├── audit.rs                  # Audit logging
│   └── utils.rs                  # Helper functions
└── tests/
    ├── encryption_tests.rs
    ├── store_tests.rs
    └── integration_tests.rs
```

### Cargo.toml

```toml
[package]
name = "aura-keystore"
version = "0.1.0"
edition = "2021"
description = "Secure key storage for AURA OS agents"
license = "MIT"
rust-version = "1.75"

[dependencies]
# Async runtime
tokio = { version = "1.41", features = ["rt", "sync"] }
async-trait = "0.1"

# Serialization
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
serde_cbor = "0.11"

# Encryption
aes-gcm = "0.10"
hkdf = "0.12"
sha2 = "0.10"
pbkdf2 = "0.12"

# Secure memory
zeroize = { version = "1.8", features = ["derive"] }

# Random
getrandom = "0.2"

# Storage
rocksdb = "0.22"

# Encoding
hex = "0.4"

# Error handling
thiserror = "2.0"
anyhow = "1.0"

# Logging
tracing = "0.1"

# Internal crates
aura-core = { path = "../aura-core" }

[dev-dependencies]
tokio = { version = "1.41", features = ["rt-multi-thread", "macros"] }
tempfile = "3.0"

[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
all = "warn"
pedantic = "warn"
nursery = "warn"
module_name_repetitions = "allow"
```

---

## 12) Security Considerations

### 12.1 Encryption

| Aspect | Implementation |
|--------|----------------|
| Algorithm | AES-256-GCM (authenticated encryption) |
| Key derivation | HKDF-SHA256 with agent+key context |
| Nonces | Random 12-byte, never reused |
| Master key | From env/file, not stored in code |

### 12.2 Memory Security

```rust
// All sensitive data uses zeroize
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Zeroize, ZeroizeOnDrop)]
struct SensitiveData {
    key: Vec<u8>,
}

// Drop implementation zeros memory
impl Drop for KeyMaterial {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}
```

### 12.3 Access Control

- **Per-agent isolation** — Keys are scoped to agent_id
- **Permission levels** — Discover → ReadMetadata → Use → Manage → Admin
- **Delegation** — Owner can grant limited access to other agents
- **Expiration** — Keys and delegations can have expiration times

### 12.4 Audit Trail

All operations are logged:
- Key creation/deletion
- Key access (with reason)
- Permission changes
- Access denials
- Key rotation

### 12.5 Future Enhancements

| Feature | Status | Notes |
|---------|--------|-------|
| AWS KMS integration | Planned | Use KMS for master key |
| HashiCorp Vault | Planned | External secret management |
| HSM support | Future | Hardware security modules |
| Key escrow | Future | Backup/recovery support |
| MPC signing | Future | Multi-party wallet keys |

---

## 13) Implementation Checklist

### Phase 0: Crate Setup
- [ ] Create `aura-keystore` crate with `Cargo.toml`
- [ ] Add to workspace `Cargo.toml`
- [ ] Create `src/lib.rs` with module structure
- [ ] Create `src/error.rs` with `thiserror` error types
- [ ] Verify `cargo fmt` and `cargo clippy` pass

### Phase 1: Core Types
- [ ] Define `KeyType` and `KeyProvider` enums
- [ ] Define `KeyMetadata` struct
- [ ] Define `StoredKey` and `EncryptedBlob`
- [ ] Define `KeyMaterial` with zeroize
- [ ] Define request/response types
- [ ] Add comprehensive doc comments

### Phase 2: Encryption
- [ ] Implement `EncryptionService`
- [ ] Implement master key loading (env/file)
- [ ] Implement HKDF key derivation
- [ ] Implement AES-256-GCM encrypt/decrypt
- [ ] Add encryption round-trip tests
- [ ] Verify zeroize behavior

### Phase 3: Storage
- [ ] Define `KeyStore` trait
- [ ] Add column families to storage schema
- [ ] Implement `RocksKeyStore`
- [ ] Implement `store_key`, `get_key`
- [ ] Implement `list_keys`, `delete_key`
- [ ] Implement secondary indexes

### Phase 4: Access Control
- [ ] Define permission types
- [ ] Implement `check_permission`
- [ ] Implement `grant_permission`, `revoke_permission`
- [ ] Add permission tests

### Phase 5: Audit
- [ ] Define `KeyAuditEvent`
- [ ] Implement audit logging
- [ ] Implement audit query methods
- [ ] Add audit tests

### Phase 6: Integration
- [ ] Integrate with `aura-reasoner` for API keys
- [ ] Add keystore to `aura-cli`
- [ ] Implement key management commands
- [ ] Add secure input handling

### Phase 7: Testing
- [ ] Unit tests for encryption
- [ ] Unit tests for storage operations
- [ ] Integration tests for full flow
- [ ] Permission tests
- [ ] Audit tests
- [ ] Run full CI checks

---

## 14) Acceptance Criteria

### Code Quality (Must Pass)
- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test --all` passes
- [ ] No `unwrap()` or `expect()` in production paths
- [ ] All public items documented
- [ ] All sensitive data properly zeroized

### Security (Must Have)
- [ ] Keys encrypted at rest with AES-256-GCM
- [ ] Master key not hardcoded
- [ ] Per-key encryption with unique DEK
- [ ] Key material zeroized on drop
- [ ] All access logged in audit trail
- [ ] Permission checks enforced

### Functional (Must Have)
- [ ] Store API keys for multiple providers
- [ ] Store wallet private keys
- [ ] Store SSH keys
- [ ] List keys with filtering
- [ ] Delete keys securely
- [ ] Rotate keys
- [ ] CLI commands for key management

### Should Have
- [ ] Key expiration support
- [ ] Permission delegation
- [ ] Default key per provider
- [ ] Audit log queries
- [ ] Key testing/validation

### Nice to Have
- [ ] Import from environment variables
- [ ] Export keys (encrypted)
- [ ] Key backup/restore
- [ ] AWS KMS integration
- [ ] Vault integration

---

## 15) Usage Examples

### 15.1 Storing an API Key

```rust
// Store Anthropic API key for an agent
let keystore: Arc<dyn KeyStore> = /* ... */;

let metadata = keystore.store_key(StoreKeyRequest {
    agent_id,
    key_type: KeyType::ApiKey,
    provider: KeyProvider::Anthropic,
    label: "anthropic-main".into(),
    key_material: KeyMaterial::new(b"sk-ant-...".to_vec()),
    expires_at_ms: None,
    tags: vec!["production".into()],
    extra: None,
}).await?;

println!("Stored key: {}", hex::encode(metadata.key_id));
```

### 15.2 Using an API Key

```rust
// In reasoner, when making API call
let key_response = keystore.get_api_key(
    agent_id,
    KeyProvider::Anthropic,
    "model completion".into(),
).await?;

let api_key = key_response.material.as_str()
    .ok_or_else(|| anyhow::anyhow!("invalid key"))?;

// Use api_key for API call
// key_response.material is zeroized on drop
```

### 15.3 CLI Usage

```bash
# Add an API key (reads from stdin)
echo "sk-ant-api03-..." | aura keys add \
    --type api \
    --provider anthropic \
    --label anthropic-main \
    --stdin

# Add from file
aura keys add \
    --type ssh \
    --provider github \
    --label deploy-key \
    --file ~/.ssh/id_ed25519

# List keys
aura keys list

# List only API keys
aura keys list --type api

# Show key details
aura keys show anthropic-main

# Rotate a key
echo "sk-ant-new-key..." | aura keys rotate anthropic-main --stdin

# Delete a key
aura keys delete anthropic-main

# View audit log
aura keys audit anthropic-main --limit 50
```

---

## 16) Summary

`aura-keystore` provides secure credential storage for AURA agents:

| Feature | Description |
|---------|-------------|
| **Key Types** | API keys, wallet keys, SSH keys, generic secrets |
| **Providers** | Anthropic, OpenAI, Google, AWS, Ethereum, Solana, SSH, custom |
| **Encryption** | AES-256-GCM with HKDF-derived per-key encryption |
| **Storage** | RocksDB with encrypted blobs |
| **Access Control** | Per-agent isolation, permission delegation |
| **Audit** | Full operation logging |

The system enables:
- Each agent to have its own API keys for different providers
- Secure wallet key storage for blockchain interactions
- SSH key management for remote operations
- Complete audit trail of all key access
- Permission delegation between agents
