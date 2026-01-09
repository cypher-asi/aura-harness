# AURA Keystore — Spec 05

**Status**: Design-ready  
**Builds on**: spec-01-aura.md, spec-02-interactive-runtime.md  
**Goal**: Secure key storage for Swarms to access third-party services

---

## 1) Purpose

Build a secure key management system that allows Swarms to store and use various types of credentials:

* **API Keys** — Access tokens for third-party AI models (Anthropic, OpenAI, etc.), cloud services, and APIs
* **Wallet Keys** — Private keys for cryptocurrency wallets and blockchain interactions
* **SSH Keys** — Authentication keys for remote servers and Git operations
* **Generic Secrets** — Other sensitive credentials (database passwords, service tokens, etc.)

### Key Scope: Per-Swarm

Keys are stored at the **Swarm level**, not per-Agent. All agents within a Swarm share access to the Swarm's keys. This simplifies key management while maintaining isolation between different Swarms.

```
┌─────────────────────────────────────────────────────────────────────────┐
│  SWARM "production"                                                      │
│                                                                          │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │  Shared Keystore                                                 │    │
│  │    • anthropic-api-key                                          │    │
│  │    • openai-api-key                                             │    │
│  │    • eth-wallet                                                 │    │
│  │    • github-deploy-key                                          │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│           │                                                              │
│           │  All agents in swarm can access                             │
│           ▼                                                              │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐               │
│  │ Agent A  │  │ Agent B  │  │ Agent C  │  │ Agent D  │               │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘               │
└─────────────────────────────────────────────────────────────────────────┘
```

### Why This Matters

1. **Simplified Management** — Configure keys once per Swarm, not per Agent
2. **Security Isolation** — Different Swarms (e.g., production vs development) have separate keys
3. **Multi-Provider Support** — Swarm can use different AI providers
4. **Wallet Integration** — Foundation for agents to interact with blockchain and DeFi
5. **Operational Security** — Keys encrypted at rest, audited access, secure rotation

---

## 2) Architecture

### 2.1 Updated Crate Layout

```
aura/
├─ aura-core              # IDs, schemas, hashing (add SwarmId)
├─ aura-store             # RocksDB storage (unchanged)
├─ aura-kernel            # Deterministic kernel (uses keystore)
├─ aura-swarm             # Router, scheduler, workers (owns keystore)
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
│  │   selection  │    │ • Web APIs   │    │ • Protection checks      │  │
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
│  │  │  • list_keys()     • rotate_key()   • unlock_session()      │ │  │
│  │  └─────────────────────────────────────────────────────────────┘ │  │
│  │                             │                                     │  │
│  │        ┌────────────────────┼────────────────────┐               │  │
│  │        ▼                    ▼                    ▼               │  │
│  │  ┌───────────┐      ┌─────────────┐      ┌─────────────────┐   │  │
│  │  │  Local    │      │   Vault     │      │    Session      │   │  │
│  │  │  Backend  │      │   Backend   │      │    Manager      │   │  │
│  │  │ (RocksDB) │      │  (Transit)  │      │ (User Password) │   │  │
│  │  └───────────┘      └─────────────┘      └─────────────────┘   │  │
│  │                             │                                     │  │
│  │                             ▼                                     │  │
│  │                    ┌─────────────────┐                           │  │
│  │                    │  Audit Logger   │                           │  │
│  │                    │  • Access logs  │                           │  │
│  │                    │  • Denials      │                           │  │
│  │                    └─────────────────┘                           │  │
│  └──────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

### 2.3 Trust Model & Threat Analysis

Understanding who can access what is critical for security design:

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        TRUST HIERARCHY                                   │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  LEVEL 0: Infrastructure (Highest Trust)                               │
│    • Cloud provider root accounts                                       │
│    • Physical server access                                             │
│    • Vault unseal key holders                                          │
│                                                                         │
│  LEVEL 1: Swarm Operator                                               │
│    • Configures encryption backend                                      │
│    • Sets protection tiers for key types                               │
│    • Has emergency access to all keys                                   │
│                                                                         │
│  LEVEL 2: User (Swarm Owner)                                           │
│    • Provides password to unlock sessions                              │
│    • Approves high-security key usage                                  │
│    • Can add/rotate/delete keys                                        │
│                                                                         │
│  LEVEL 3: Agents (Within Unlocked Session)                             │
│    • Can USE keys (not see them directly)                              │
│    • Access logged in audit trail                                      │
│    • Subject to protection tier requirements                           │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

**What happens if AURA is compromised?**

| Encryption Backend | Exposure if Compromised | Recovery |
|--------------------|------------------------|----------|
| Local (Master Key) | All keys until master key rotated | Must re-encrypt all |
| Vault (Transit) | Keys accessed during breach window | Revoke token, breach stops |
| User Password Session | Only keys accessed during session | Lock session, breach stops |

---

## 3) Hybrid Protection Model

Keys can have different protection levels based on sensitivity:

### 3.1 Protection Tiers

```
┌─────────────────────────────────────────────────────────────────────────┐
│                     KEY PROTECTION TIERS                                 │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  TIER 1: AlwaysAvailable                                               │
│  ─────────────────────────────────────────────────────────────────────│
│    Keys accessible whenever the Swarm is running.                      │
│    Encrypted with system master key or Vault Transit.                  │
│                                                                         │
│    Use for:                                                             │
│      • Read-only API keys (search, weather, public data)               │
│      • Non-sensitive service tokens                                    │
│                                                                         │
│    Risk: If system compromised, keys exposed until credential rotated  │
│                                                                         │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  TIER 2: SessionUnlock (User Password)                                 │
│  ─────────────────────────────────────────────────────────────────────│
│    Keys only accessible after user enters password to unlock session.  │
│    Session has configurable timeout (default: 4 hours).                │
│                                                                         │
│    Use for:                                                             │
│      • AI provider API keys (Anthropic, OpenAI)                        │
│      • GitHub/GitLab tokens                                            │
│      • Cloud service credentials                                        │
│                                                                         │
│    Risk: Only exposed while session is unlocked                        │
│                                                                         │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  TIER 3: PerUseApproval (Password Each Time)                          │
│  ─────────────────────────────────────────────────────────────────────│
│    User must enter password for EACH use of the key.                   │
│    Shows what the key will be used for before approval.                │
│                                                                         │
│    Use for:                                                             │
│      • Wallet private keys (signing transactions)                      │
│      • Production SSH keys                                             │
│      • High-value credentials                                          │
│                                                                         │
│    Risk: Only exposed for single operation, with user awareness        │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

### 3.2 Protection Type Definition

```rust
/// Protection level for a stored key
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KeyProtection {
    /// Always accessible when Swarm is running
    AlwaysAvailable,
    
    /// Requires user to unlock session with password
    SessionUnlock {
        /// Session timeout in seconds (default: 4 hours)
        session_timeout_secs: u64,
    },
    
    /// Requires user password for each use
    PerUseApproval {
        /// Description shown to user when approval requested
        approval_prompt: String,
    },
}

impl Default for KeyProtection {
    fn default() -> Self {
        // Default to session unlock for most keys
        Self::SessionUnlock {
            session_timeout_secs: 4 * 60 * 60, // 4 hours
        }
    }
}
```

### 3.3 Default Protection by Key Type

```rust
impl KeyType {
    /// Get the recommended default protection for this key type
    pub fn default_protection(&self) -> KeyProtection {
        match self {
            // Low sensitivity - always available
            KeyType::ApiKey => KeyProtection::SessionUnlock {
                session_timeout_secs: 4 * 60 * 60,
            },
            
            // High sensitivity - per-use approval
            KeyType::WalletPrivateKey => KeyProtection::PerUseApproval {
                approval_prompt: "Sign blockchain transaction".into(),
            },
            
            // Medium sensitivity - session unlock
            KeyType::SshPrivateKey => KeyProtection::SessionUnlock {
                session_timeout_secs: 2 * 60 * 60,
            },
            
            KeyType::SshPublicKey => KeyProtection::AlwaysAvailable,
            
            KeyType::Secret => KeyProtection::SessionUnlock {
                session_timeout_secs: 4 * 60 * 60,
            },
            
            KeyType::OAuthToken => KeyProtection::SessionUnlock {
                session_timeout_secs: 4 * 60 * 60,
            },
            
            KeyType::DatabaseCredential => KeyProtection::SessionUnlock {
                session_timeout_secs: 8 * 60 * 60,
            },
        }
    }
}
```

---

## 4) Data Model

### 4.1 Core Types

```rust
// aura-keystore/src/types.rs

use aura_core::SwarmId;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Unique identifier for a Swarm (keys are scoped to Swarm)
pub type SwarmId = [u8; 32];

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
    /// Swarm that owns this key
    pub swarm_id: SwarmId,
    /// Type of key
    pub key_type: KeyType,
    /// Provider/service this key is for
    pub provider: KeyProvider,
    /// Human-readable label
    pub label: String,
    /// Protection level for this key
    pub protection: KeyProtection,
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
    /// Encrypted key material (or Vault reference)
    pub encrypted_value: EncryptedValue,
    /// Version of the encryption scheme
    pub encryption_version: u32,
}

/// Encrypted value - either local blob or Vault reference
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EncryptedValue {
    /// Locally encrypted with AES-GCM
    LocalEncrypted(EncryptedBlob),
    /// Encrypted via Vault Transit
    VaultTransit {
        /// Vault ciphertext (e.g., "vault:v1:8sdF3k...")
        ciphertext: String,
        /// Vault key name used
        key_name: String,
        /// Vault key version
        key_version: u32,
    },
    /// Encrypted with user password-derived key
    PasswordEncrypted {
        /// The encrypted blob
        blob: EncryptedBlob,
        /// Salt for password derivation
        password_salt: [u8; 32],
        /// KDF parameters
        kdf_params: KdfParams,
    },
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

/// KDF parameters for password-based encryption
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KdfParams {
    pub algorithm: KdfAlgorithm,
    pub memory_cost_kb: u32,  // For Argon2
    pub time_cost: u32,       // Iterations
    pub parallelism: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KdfAlgorithm {
    Argon2id,
    Pbkdf2Sha256,
}

impl Default for KdfParams {
    fn default() -> Self {
        Self {
            algorithm: KdfAlgorithm::Argon2id,
            memory_cost_kb: 65536,  // 64 MB
            time_cost: 3,
            parallelism: 4,
        }
    }
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

### 4.2 Request/Response Types

```rust
/// Request to store a new key
#[derive(Debug)]
pub struct StoreKeyRequest {
    /// Swarm storing the key
    pub swarm_id: SwarmId,
    /// Type of key
    pub key_type: KeyType,
    /// Provider/service
    pub provider: KeyProvider,
    /// Human-readable label
    pub label: String,
    /// The key material (will be encrypted)
    pub key_material: KeyMaterial,
    /// Protection level (defaults based on key type if None)
    pub protection: Option<KeyProtection>,
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
    /// Swarm requesting the key
    pub swarm_id: SwarmId,
    /// Key to retrieve
    pub key_id: KeyId,
    /// Reason for access (for audit logging)
    pub access_reason: String,
    /// User password (required for SessionUnlock/PerUseApproval if no active session)
    pub user_password: Option<String>,
}

/// Query for listing keys
#[derive(Debug, Clone, Default)]
pub struct ListKeysQuery {
    /// Filter by swarm
    pub swarm_id: Option<SwarmId>,
    /// Filter by key type
    pub key_type: Option<KeyType>,
    /// Filter by provider
    pub provider: Option<KeyProvider>,
    /// Filter by protection level
    pub protection: Option<KeyProtection>,
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
```

### 4.3 Session Types

```rust
/// An active unlocked session
#[derive(Debug)]
pub struct UnlockedSession {
    /// Session ID
    pub session_id: [u8; 16],
    /// Swarm this session is for
    pub swarm_id: SwarmId,
    /// When the session was created
    pub created_at: std::time::Instant,
    /// When the session expires
    pub expires_at: std::time::Instant,
    /// Derived key encryption key (from password)
    kek: Zeroizing<[u8; 32]>,
}

/// Request to unlock a session
#[derive(Debug)]
pub struct UnlockSessionRequest {
    /// Swarm to unlock
    pub swarm_id: SwarmId,
    /// User's password
    pub password: String,
    /// Optional custom timeout (uses key's default if not specified)
    pub timeout_secs: Option<u64>,
}

/// Request for per-use approval
#[derive(Debug)]
pub struct ApprovalRequest {
    /// Swarm ID
    pub swarm_id: SwarmId,
    /// Key being accessed
    pub key_id: KeyId,
    /// Key label (for display)
    pub key_label: String,
    /// What the key will be used for
    pub usage_description: String,
    /// Approval prompt from key metadata
    pub approval_prompt: String,
}

/// Response to approval request
#[derive(Debug)]
pub struct ApprovalResponse {
    /// Whether approved
    pub approved: bool,
    /// User's password (required if approved)
    pub password: Option<String>,
}
```

---

## 5) Encryption Backends

### 5.1 Backend Trait

```rust
/// Backend for encrypting/decrypting key material
#[async_trait]
pub trait EncryptionBackend: Send + Sync {
    /// Backend name
    fn name(&self) -> &'static str;
    
    /// Encrypt key material
    async fn encrypt(
        &self,
        swarm_id: &SwarmId,
        key_id: &KeyId,
        material: &KeyMaterial,
    ) -> Result<EncryptedValue, KeystoreError>;
    
    /// Decrypt key material
    async fn decrypt(
        &self,
        swarm_id: &SwarmId,
        key_id: &KeyId,
        encrypted: &EncryptedValue,
    ) -> Result<KeyMaterial, KeystoreError>;
    
    /// Check if backend is available
    async fn health_check(&self) -> bool;
}
```

### 5.2 Local Backend (AES-GCM)

```rust
/// Local encryption using AES-256-GCM with master key
pub struct LocalEncryptionBackend {
    master_key: Zeroizing<[u8; 32]>,
}

impl LocalEncryptionBackend {
    pub fn new(config: &LocalEncryptionConfig) -> Result<Self, KeystoreError> {
        let master_key = Self::load_master_key(&config.master_key_source)?;
        Ok(Self { master_key })
    }
    
    fn load_master_key(source: &MasterKeySource) -> Result<Zeroizing<[u8; 32]>, KeystoreError> {
        let bytes: [u8; 32] = match source {
            MasterKeySource::EnvVar(var) => {
                let value = std::env::var(var)
                    .map_err(|_| KeystoreError::MasterKeyNotFound {
                        source: format!("env:{var}"),
                    })?;
                hex::decode(&value)?
                    .try_into()
                    .map_err(|_| KeystoreError::InvalidMasterKey {
                        reason: "expected 32 bytes".into(),
                    })?
            }
            MasterKeySource::File(path) => {
                let content = std::fs::read_to_string(path)?;
                hex::decode(content.trim())?
                    .try_into()
                    .map_err(|_| KeystoreError::InvalidMasterKey {
                        reason: "expected 32 bytes".into(),
                    })?
            }
            _ => return Err(KeystoreError::NotImplemented {
                feature: "this master key source".into(),
            }),
        };
        Ok(Zeroizing::new(bytes))
    }
}

#[async_trait]
impl EncryptionBackend for LocalEncryptionBackend {
    fn name(&self) -> &'static str { "local" }
    
    async fn encrypt(
        &self,
        swarm_id: &SwarmId,
        key_id: &KeyId,
        material: &KeyMaterial,
    ) -> Result<EncryptedValue, KeystoreError> {
        // Generate random salt and nonce
        let mut salt = [0u8; 32];
        let mut nonce = [0u8; 12];
        getrandom::getrandom(&mut salt)?;
        getrandom::getrandom(&mut nonce)?;
        
        // Derive per-key encryption key
        let dek = self.derive_key(swarm_id, key_id, &salt);
        
        // Encrypt with AES-256-GCM
        let cipher = Aes256Gcm::new_from_slice(&dek)?;
        let ciphertext = cipher.encrypt(Nonce::from_slice(&nonce), material.as_bytes())?;
        
        // Split ciphertext and tag
        let (ct, tag_slice) = ciphertext.split_at(ciphertext.len() - 16);
        let mut tag = [0u8; 16];
        tag.copy_from_slice(tag_slice);
        
        Ok(EncryptedValue::LocalEncrypted(EncryptedBlob {
            ciphertext: ct.to_vec(),
            nonce,
            tag,
            salt,
        }))
    }
    
    async fn decrypt(
        &self,
        swarm_id: &SwarmId,
        key_id: &KeyId,
        encrypted: &EncryptedValue,
    ) -> Result<KeyMaterial, KeystoreError> {
        let EncryptedValue::LocalEncrypted(blob) = encrypted else {
            return Err(KeystoreError::WrongEncryptionBackend);
        };
        
        // Derive per-key encryption key
        let dek = self.derive_key(swarm_id, key_id, &blob.salt);
        
        // Reconstruct ciphertext with tag
        let mut ciphertext_with_tag = blob.ciphertext.clone();
        ciphertext_with_tag.extend_from_slice(&blob.tag);
        
        // Decrypt with AES-256-GCM
        let cipher = Aes256Gcm::new_from_slice(&dek)?;
        let plaintext = cipher.decrypt(
            Nonce::from_slice(&blob.nonce),
            ciphertext_with_tag.as_ref(),
        )?;
        
        Ok(KeyMaterial::new(plaintext))
    }
    
    async fn health_check(&self) -> bool { true }
}
```

### 5.3 HashiCorp Vault Transit Backend

The **recommended production backend**. Vault handles all encryption/decryption — AURA never sees the encryption key.

```rust
/// HashiCorp Vault Transit encryption backend
pub struct VaultTransitBackend {
    client: VaultClient,
    key_name: String,
    config: VaultConfig,
}

#[derive(Debug, Clone)]
pub struct VaultConfig {
    /// Vault server address
    pub address: String,
    /// Transit secret engine mount path
    pub transit_mount: String,
    /// Key name in Transit engine
    pub key_name: String,
    /// Authentication method
    pub auth: VaultAuth,
    /// Request timeout
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub enum VaultAuth {
    /// AppRole authentication (recommended for servers)
    AppRole {
        role_id: String,
        secret_id: String,
    },
    /// Token authentication
    Token(String),
    /// Kubernetes service account
    Kubernetes {
        role: String,
        jwt_path: PathBuf,
    },
}

impl VaultTransitBackend {
    pub async fn new(config: VaultConfig) -> Result<Self, KeystoreError> {
        let client = VaultClient::new(&config.address, config.timeout)?;
        
        // Authenticate
        let token = match &config.auth {
            VaultAuth::AppRole { role_id, secret_id } => {
                client.auth_approle(role_id, secret_id).await?
            }
            VaultAuth::Token(token) => token.clone(),
            VaultAuth::Kubernetes { role, jwt_path } => {
                let jwt = std::fs::read_to_string(jwt_path)?;
                client.auth_kubernetes(role, &jwt).await?
            }
        };
        
        client.set_token(&token);
        
        Ok(Self {
            client,
            key_name: config.key_name.clone(),
            config,
        })
    }
}

#[async_trait]
impl EncryptionBackend for VaultTransitBackend {
    fn name(&self) -> &'static str { "vault-transit" }
    
    async fn encrypt(
        &self,
        swarm_id: &SwarmId,
        key_id: &KeyId,
        material: &KeyMaterial,
    ) -> Result<EncryptedValue, KeystoreError> {
        // Add context for additional security (key is bound to swarm+key_id)
        let context = base64::encode(
            [swarm_id.as_slice(), key_id.as_slice()].concat()
        );
        
        let response = self.client
            .post(&format!("{}/encrypt/{}", self.config.transit_mount, self.key_name))
            .json(&serde_json::json!({
                "plaintext": base64::encode(material.as_bytes()),
                "context": context,
            }))
            .send()
            .await?;
        
        let result: VaultEncryptResponse = response.json().await?;
        
        Ok(EncryptedValue::VaultTransit {
            ciphertext: result.data.ciphertext,
            key_name: self.key_name.clone(),
            key_version: result.data.key_version,
        })
    }
    
    async fn decrypt(
        &self,
        swarm_id: &SwarmId,
        key_id: &KeyId,
        encrypted: &EncryptedValue,
    ) -> Result<KeyMaterial, KeystoreError> {
        let EncryptedValue::VaultTransit { ciphertext, .. } = encrypted else {
            return Err(KeystoreError::WrongEncryptionBackend);
        };
        
        let context = base64::encode(
            [swarm_id.as_slice(), key_id.as_slice()].concat()
        );
        
        let response = self.client
            .post(&format!("{}/decrypt/{}", self.config.transit_mount, self.key_name))
            .json(&serde_json::json!({
                "ciphertext": ciphertext,
                "context": context,
            }))
            .send()
            .await?;
        
        let result: VaultDecryptResponse = response.json().await?;
        let plaintext = base64::decode(&result.data.plaintext)?;
        
        Ok(KeyMaterial::new(plaintext))
    }
    
    async fn health_check(&self) -> bool {
        self.client
            .get(&format!("{}/keys/{}", self.config.transit_mount, self.key_name))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}
```

### 5.4 Vault Transit Setup

```hcl
# Vault policy for AURA keystore
path "transit/encrypt/aura-swarm-keys" {
  capabilities = ["update"]
}

path "transit/decrypt/aura-swarm-keys" {
  capabilities = ["update"]
}

# Deny reading the key itself
path "transit/keys/aura-swarm-keys" {
  capabilities = ["read"]  # Metadata only, not the key material
}
```

```bash
# Setup commands
vault secrets enable transit

vault write transit/keys/aura-swarm-keys \
    type=aes256-gcm96 \
    derived=true         # Enables context-based key derivation

vault policy write aura-keystore aura-keystore-policy.hcl

vault write auth/approle/role/aura-keystore \
    token_policies="aura-keystore" \
    token_ttl=1h \
    token_max_ttl=4h
```

---

## 6) Session Management

### 6.1 Session Manager

```rust
/// Manages unlocked sessions for password-protected keys
pub struct SessionManager {
    /// Active sessions by swarm_id
    sessions: DashMap<SwarmId, UnlockedSession>,
    /// Default session timeout
    default_timeout: Duration,
}

impl SessionManager {
    pub fn new(default_timeout: Duration) -> Self {
        Self {
            sessions: DashMap::new(),
            default_timeout,
        }
    }
    
    /// Unlock a session with user password
    pub fn unlock(
        &self,
        request: UnlockSessionRequest,
        password_salt: &[u8; 32],
        kdf_params: &KdfParams,
    ) -> Result<[u8; 16], KeystoreError> {
        // Derive KEK from password
        let kek = derive_kek(&request.password, password_salt, kdf_params)?;
        
        let timeout = request.timeout_secs
            .map(Duration::from_secs)
            .unwrap_or(self.default_timeout);
        
        let now = std::time::Instant::now();
        let mut session_id = [0u8; 16];
        getrandom::getrandom(&mut session_id)?;
        
        let session = UnlockedSession {
            session_id,
            swarm_id: request.swarm_id,
            created_at: now,
            expires_at: now + timeout,
            kek: Zeroizing::new(kek),
        };
        
        self.sessions.insert(request.swarm_id, session);
        
        tracing::info!(
            swarm_id = %hex::encode(request.swarm_id),
            timeout_secs = timeout.as_secs(),
            "session unlocked"
        );
        
        Ok(session_id)
    }
    
    /// Lock a session (explicit logout)
    pub fn lock(&self, swarm_id: &SwarmId) {
        if self.sessions.remove(swarm_id).is_some() {
            tracing::info!(
                swarm_id = %hex::encode(swarm_id),
                "session locked"
            );
        }
    }
    
    /// Get KEK if session is active and not expired
    pub fn get_kek(&self, swarm_id: &SwarmId) -> Option<Zeroizing<[u8; 32]>> {
        let session = self.sessions.get(swarm_id)?;
        
        if std::time::Instant::now() > session.expires_at {
            // Session expired, remove it
            drop(session);
            self.sessions.remove(swarm_id);
            return None;
        }
        
        Some(session.kek.clone())
    }
    
    /// Check if session is active
    pub fn is_unlocked(&self, swarm_id: &SwarmId) -> bool {
        self.get_kek(swarm_id).is_some()
    }
    
    /// Cleanup expired sessions (call periodically)
    pub fn cleanup_expired(&self) {
        let now = std::time::Instant::now();
        self.sessions.retain(|_, session| session.expires_at > now);
    }
}

fn derive_kek(
    password: &str,
    salt: &[u8; 32],
    params: &KdfParams,
) -> Result<[u8; 32], KeystoreError> {
    match params.algorithm {
        KdfAlgorithm::Argon2id => {
            use argon2::{Argon2, Algorithm, Params, Version};
            
            let argon2 = Argon2::new(
                Algorithm::Argon2id,
                Version::V0x13,
                Params::new(
                    params.memory_cost_kb,
                    params.time_cost,
                    params.parallelism,
                    Some(32),
                ).map_err(|e| KeystoreError::KdfFailed {
                    reason: e.to_string(),
                })?,
            );
            
            let mut kek = [0u8; 32];
            argon2.hash_password_into(password.as_bytes(), salt, &mut kek)
                .map_err(|e| KeystoreError::KdfFailed {
                    reason: e.to_string(),
                })?;
            
            Ok(kek)
        }
        KdfAlgorithm::Pbkdf2Sha256 => {
            let mut kek = [0u8; 32];
            pbkdf2::pbkdf2_hmac::<sha2::Sha256>(
                password.as_bytes(),
                salt,
                params.time_cost,
                &mut kek,
            );
            Ok(kek)
        }
    }
}
```

---

## 7) KeyStore Interface

### 7.1 Core Trait

```rust
// aura-keystore/src/lib.rs

use async_trait::async_trait;

/// Secure key storage interface
#[async_trait]
pub trait KeyStore: Send + Sync {
    // === Session Management ===
    
    /// Unlock a session for password-protected keys
    async fn unlock_session(
        &self,
        request: UnlockSessionRequest,
    ) -> Result<[u8; 16], KeystoreError>;
    
    /// Lock a session
    async fn lock_session(&self, swarm_id: SwarmId) -> Result<(), KeystoreError>;
    
    /// Check if session is unlocked
    async fn is_session_unlocked(&self, swarm_id: SwarmId) -> bool;
    
    // === Key Storage ===
    
    /// Store a new key
    async fn store_key(&self, request: StoreKeyRequest) -> Result<KeyMetadata, KeystoreError>;
    
    /// Retrieve a key (decrypts the key material)
    /// 
    /// For SessionUnlock keys: requires active session or password in request
    /// For PerUseApproval keys: requires password in request (every time)
    async fn get_key(&self, request: GetKeyRequest) -> Result<KeyResponse, KeystoreError>;
    
    /// Get key metadata only (no decryption, no session required)
    async fn get_key_metadata(
        &self,
        swarm_id: SwarmId,
        key_id: KeyId,
    ) -> Result<KeyMetadata, KeystoreError>;
    
    /// List keys matching query (metadata only)
    async fn list_keys(&self, query: ListKeysQuery) -> Result<Vec<KeyMetadata>, KeystoreError>;
    
    /// Delete a key
    async fn delete_key(
        &self,
        swarm_id: SwarmId,
        key_id: KeyId,
        reason: String,
    ) -> Result<(), KeystoreError>;
    
    /// Rotate a key
    async fn rotate_key(
        &self,
        swarm_id: SwarmId,
        key_id: KeyId,
        new_material: KeyMaterial,
    ) -> Result<KeyMetadata, KeystoreError>;
    
    // === Convenience Methods ===
    
    /// Get an API key for a specific provider
    async fn get_api_key(
        &self,
        swarm_id: SwarmId,
        provider: KeyProvider,
        reason: String,
    ) -> Result<KeyResponse, KeystoreError>;
    
    // === Audit ===
    
    /// Get audit log for a swarm
    async fn get_audit_log(
        &self,
        swarm_id: SwarmId,
        from_ms: Option<u64>,
        to_ms: Option<u64>,
        limit: Option<usize>,
    ) -> Result<Vec<KeyAuditEvent>, KeystoreError>;
}
```

### 7.2 Implementation

```rust
pub struct SwarmKeyStore {
    /// Storage backend (RocksDB)
    db: Arc<DB>,
    /// Encryption backend (Local or Vault)
    encryption: Arc<dyn EncryptionBackend>,
    /// Session manager for password-protected keys
    sessions: SessionManager,
    /// Audit enabled
    audit_enabled: bool,
    /// Swarm's password salt (for session unlock)
    password_salt: [u8; 32],
    /// KDF parameters
    kdf_params: KdfParams,
}

#[async_trait]
impl KeyStore for SwarmKeyStore {
    async fn get_key(&self, request: GetKeyRequest) -> Result<KeyResponse, KeystoreError> {
        // Load metadata first
        let metadata = self.get_key_metadata(request.swarm_id, request.key_id).await?;
        
        // Check protection requirements
        match &metadata.protection {
            KeyProtection::AlwaysAvailable => {
                // No additional auth needed
            }
            
            KeyProtection::SessionUnlock { .. } => {
                // Check for active session or password
                if !self.sessions.is_unlocked(&request.swarm_id) {
                    if let Some(password) = &request.user_password {
                        // Unlock session with provided password
                        self.unlock_session(UnlockSessionRequest {
                            swarm_id: request.swarm_id,
                            password: password.clone(),
                            timeout_secs: None,
                        }).await?;
                    } else {
                        return Err(KeystoreError::SessionRequired {
                            key_label: metadata.label.clone(),
                        });
                    }
                }
            }
            
            KeyProtection::PerUseApproval { approval_prompt } => {
                // Always require password for per-use approval
                let password = request.user_password.as_ref()
                    .ok_or_else(|| KeystoreError::ApprovalRequired {
                        key_label: metadata.label.clone(),
                        prompt: approval_prompt.clone(),
                    })?;
                
                // Verify password (derive KEK and try to decrypt)
                // This ensures the password is correct before proceeding
            }
        }
        
        // Load encrypted key
        let stored_key = self.load_stored_key(&request.swarm_id, &request.key_id)?;
        
        // Decrypt based on encryption type
        let material = match &stored_key.encrypted_value {
            EncryptedValue::LocalEncrypted(_) | EncryptedValue::VaultTransit { .. } => {
                self.encryption.decrypt(
                    &request.swarm_id,
                    &request.key_id,
                    &stored_key.encrypted_value,
                ).await?
            }
            
            EncryptedValue::PasswordEncrypted { blob, password_salt, kdf_params } => {
                // Get KEK from session or derive from password
                let kek = self.sessions.get_kek(&request.swarm_id)
                    .or_else(|| {
                        request.user_password.as_ref().and_then(|p| {
                            derive_kek(p, password_salt, kdf_params).ok()
                                .map(Zeroizing::new)
                        })
                    })
                    .ok_or(KeystoreError::SessionRequired {
                        key_label: metadata.label.clone(),
                    })?;
                
                // Decrypt with KEK
                decrypt_with_kek(&kek, blob)?
            }
        };
        
        // Update access metadata
        self.update_access_metadata(&request.swarm_id, &request.key_id).await?;
        
        // Audit log
        self.audit_log(KeyAuditEvent {
            swarm_id: request.swarm_id,
            key_id: Some(request.key_id),
            operation: KeyOperation::Access,
            success: true,
            reason: Some(request.access_reason),
            ..Default::default()
        }).await?;
        
        Ok(KeyResponse { metadata, material })
    }
}
```

---

## 8) Storage Schema

### 8.1 Column Families

```rust
const CF_KEYS: &str = "keys";              // Encrypted key storage
const CF_KEY_META: &str = "key_meta";      // Key metadata
const CF_SWARM_CONFIG: &str = "swarm_cfg"; // Per-swarm configuration
const CF_KEY_AUDIT: &str = "key_audit";    // Audit log
const CF_KEY_INDEX: &str = "key_index";    // Secondary indexes
```

### 8.2 Key Schemas

```
keys (encrypted key material):
  Key:   K | swarm_id(32) | key_id(16)
  Value: StoredKey (CBOR)

key_meta (metadata for queries):
  Key:   M | swarm_id(32) | key_id(16)
  Value: KeyMetadata (CBOR)

swarm_cfg (per-swarm configuration):
  Key:   C | swarm_id(32)
  Value: SwarmKeystoreConfig (CBOR)
    - password_salt: [u8; 32]
    - kdf_params: KdfParams
    - default_protection: KeyProtection
    - encryption_backend: BackendType

key_audit (audit log):
  Key:   A | swarm_id(32) | timestamp_ms(u64be) | event_id(16)
  Value: KeyAuditEvent (CBOR)

key_index (secondary indexes):
  Provider index:
    Key:   I | P | swarm_id(32) | provider(hash) | key_id(16)
  Type index:
    Key:   I | T | swarm_id(32) | key_type(u8) | key_id(16)
```

---

## 9) CLI Integration

### 9.1 Key Management Commands

```bash
# Unlock session before working with keys
$ aura keys unlock
Enter password to unlock keystore: ********
✓ Session unlocked (expires in 4 hours)

# Add keys (requires unlocked session for SessionUnlock keys)
$ aura keys add \
    --type api \
    --provider anthropic \
    --label anthropic-main \
    --protection session-unlock \
    --stdin
Enter API key: [hidden input]
✓ Key stored: anthropic-main (7f3a...)

# Add a wallet key with per-use approval
$ aura keys add \
    --type wallet \
    --provider ethereum \
    --label main-wallet \
    --protection per-use-approval \
    --approval-prompt "Sign Ethereum transaction" \
    --file ./wallet.key
✓ Key stored: main-wallet (8e1d...)

# List keys (shows protection level)
$ aura keys list
┌─────────────────────────────────────────────────────────────────────────┐
│ STORED KEYS (Swarm: production)                                         │
├──────────┬────────────────┬────────┬───────────┬─────────────┬─────────┤
│ ID       │ Label          │ Type   │ Provider  │ Protection  │ Status  │
├──────────┼────────────────┼────────┼───────────┼─────────────┼─────────┤
│ 7f3a...  │ anthropic-main │ api    │ anthropic │ session     │ active  │
│ 2b9c...  │ openai-backup  │ api    │ openai    │ session     │ active  │
│ 8e1d...  │ main-wallet    │ wallet │ ethereum  │ per-use     │ active  │
│ 4c2f...  │ deploy-key     │ ssh    │ github    │ session     │ active  │
└──────────┴────────────────┴────────┴───────────┴─────────────┴─────────┘

# Lock session when done
$ aura keys lock
✓ Session locked

# Configure Vault backend
$ aura keys config set-backend vault \
    --address https://vault.example.com \
    --transit-mount transit \
    --key-name aura-swarm-keys \
    --auth-method approle
```

### 9.2 Interactive Approval Flow

When an agent needs a per-use approval key:

```
┌─────────────────────────────────────────────────────────────────────────┐
│  🔐 KEY ACCESS APPROVAL REQUIRED                                         │
│                                                                          │
│  An agent is requesting access to a protected key:                      │
│                                                                          │
│  Key:      main-wallet (Ethereum)                                       │
│  Purpose:  Sign Ethereum transaction                                    │
│  Details:  Transfer 0.5 ETH to 0x742d...F3a2                           │
│                                                                          │
│  Enter password to approve: ********                                    │
│                                                                          │
│  [Approve]  [Deny]                                                      │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 10) Configuration

### 10.1 Environment Variables

```bash
# Encryption backend selection
AURA_KEYSTORE_BACKEND=vault  # or "local"

# Local backend configuration
AURA_MASTER_KEY=<hex-encoded 32 bytes>
# OR
AURA_MASTER_KEY_FILE=/path/to/master.key

# Vault backend configuration
AURA_VAULT_ADDR=https://vault.example.com
AURA_VAULT_TRANSIT_MOUNT=transit
AURA_VAULT_KEY_NAME=aura-swarm-keys
AURA_VAULT_AUTH_METHOD=approle
AURA_VAULT_ROLE_ID=<role-id>
AURA_VAULT_SECRET_ID=<secret-id>

# Session defaults
AURA_SESSION_TIMEOUT_SECS=14400  # 4 hours
AURA_KDF_MEMORY_KB=65536         # 64 MB for Argon2
AURA_KDF_TIME_COST=3
AURA_KDF_PARALLELISM=4
```

### 10.2 Swarm Configuration

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmKeystoreConfig {
    /// Swarm ID
    pub swarm_id: SwarmId,
    /// Password salt for this swarm
    pub password_salt: [u8; 32],
    /// KDF parameters
    pub kdf_params: KdfParams,
    /// Encryption backend to use
    pub backend: BackendConfig,
    /// Default protection for new keys
    pub default_protection: KeyProtection,
    /// Whether audit logging is enabled
    pub audit_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackendConfig {
    Local {
        master_key_source: MasterKeySource,
    },
    Vault {
        address: String,
        transit_mount: String,
        key_name: String,
        auth: VaultAuthConfig,
    },
}
```

---

## 11) Crate Structure

```
aura-keystore/
├── Cargo.toml
├── src/
│   ├── lib.rs                    # Public API exports
│   ├── types.rs                  # Core types
│   ├── error.rs                  # KeystoreError
│   ├── store.rs                  # KeyStore trait
│   ├── swarm_store.rs            # SwarmKeyStore implementation
│   ├── backend/
│   │   ├── mod.rs                # EncryptionBackend trait
│   │   ├── local.rs              # LocalEncryptionBackend
│   │   └── vault.rs              # VaultTransitBackend
│   ├── session.rs                # SessionManager
│   ├── audit.rs                  # Audit logging
│   └── kdf.rs                    # Key derivation functions
└── tests/
    ├── local_backend_tests.rs
    ├── session_tests.rs
    └── integration_tests.rs
```

### Cargo.toml

```toml
[package]
name = "aura-keystore"
version = "0.1.0"
edition = "2021"
description = "Secure key storage for AURA OS swarms"

[dependencies]
# Async
tokio = { version = "1.41", features = ["rt", "sync", "time"] }
async-trait = "0.1"

# Serialization
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
serde_cbor = "0.11"

# Encryption
aes-gcm = "0.10"
hkdf = "0.12"
sha2 = "0.10"
argon2 = "0.5"
pbkdf2 = "0.12"

# Secure memory
zeroize = { version = "1.8", features = ["derive"] }

# Random
getrandom = "0.2"

# Storage
rocksdb = "0.22"

# HTTP client (for Vault)
reqwest = { version = "0.12", features = ["json"] }

# Concurrency
dashmap = "6.0"

# Encoding
hex = "0.4"
base64 = "0.22"

# Error handling
thiserror = "2.0"
anyhow = "1.0"

# Logging
tracing = "0.1"

# Internal
aura-core = { path = "../aura-core" }

[dev-dependencies]
tokio = { version = "1.41", features = ["rt-multi-thread", "macros"] }
tempfile = "3.0"
wiremock = "0.6"  # For mocking Vault
```

---

## 12) Implementation Checklist

### Phase 0: Crate Setup
- [ ] Create `aura-keystore` crate
- [ ] Add `SwarmId` to `aura-core`
- [ ] Define error types
- [ ] Verify clippy/fmt pass

### Phase 1: Core Types
- [ ] Define `KeyType`, `KeyProvider`, `KeyProtection`
- [ ] Define `KeyMetadata`, `StoredKey`, `EncryptedValue`
- [ ] Define `KeyMaterial` with zeroize
- [ ] Define session types

### Phase 2: Encryption Backends
- [ ] Implement `EncryptionBackend` trait
- [ ] Implement `LocalEncryptionBackend`
- [ ] Implement `VaultTransitBackend`
- [ ] Add backend health checks

### Phase 3: Session Management
- [ ] Implement `SessionManager`
- [ ] Implement Argon2id KDF
- [ ] Add session timeout cleanup
- [ ] Add session tests

### Phase 4: KeyStore Implementation
- [ ] Implement `SwarmKeyStore`
- [ ] Implement protection tier enforcement
- [ ] Add RocksDB storage
- [ ] Add audit logging

### Phase 5: CLI Integration
- [ ] Add `keys unlock` / `keys lock` commands
- [ ] Add `keys add` with protection options
- [ ] Add `keys list` showing protection
- [ ] Add `keys config` for backend setup

### Phase 6: Vault Integration
- [ ] Add Vault client
- [ ] Implement AppRole auth
- [ ] Test with real Vault instance
- [ ] Document Vault setup

### Phase 7: Testing
- [ ] Unit tests for each backend
- [ ] Session management tests
- [ ] Protection tier tests
- [ ] Integration tests

---

## 13) Summary

`aura-keystore` provides secure, flexible credential storage for AURA Swarms:

| Feature | Description |
|---------|-------------|
| **Key Scope** | Per-Swarm (shared by all agents in swarm) |
| **Protection Tiers** | AlwaysAvailable, SessionUnlock, PerUseApproval |
| **Encryption Backends** | Local (AES-GCM) or HashiCorp Vault Transit |
| **User Password** | Session-based unlock with Argon2id KDF |
| **Audit** | Full operation logging |

The hybrid model provides:
- **Convenience** for low-sensitivity keys (always available)
- **Session-based security** for API keys (unlock once, use many)
- **Maximum security** for wallet keys (approve each use)
- **Production-grade encryption** via Vault Transit (encryption key never leaves Vault)
