use crate::{
    auth,
    db::{self, InstanceAccessProfile, VultrAccount},
    router::{Request, Response},
    tools,
};
use aes_gcm_siv::{
    aead::{Aead, KeyInit},
    Aes256GcmSiv, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const API_ACCESS_URL: &str = "https://console.vultr.com/user/apiaccess/";
const VULTR_API_BASE: &str = "https://api.vultr.com/v2";
const CATALOG_TTL_SECS: u64 = 300;
const REMOTE_OUTPUT_LIMIT: usize = 24_000;

#[derive(Debug, Clone, Serialize)]
pub struct VultrConnectionStatus {
    pub status: String,
    pub connected: bool,
    pub label: Option<String>,
    pub api_key_last4: Option<String>,
    pub verified_at: Option<i64>,
    pub last_error: Option<String>,
    pub api_access_url: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstanceAccessProfileView {
    pub instance_id: String,
    pub instance_label: String,
    pub host: String,
    pub port: i64,
    pub username: String,
    pub auth_mode: String,
    pub public_key: String,
    pub has_secret: bool,
    pub ssh_state: String,
    pub last_verified_at: Option<i64>,
    pub last_error: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct VultrCatalogEntry {
    pub id: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VultrInstanceSummary {
    pub id: String,
    pub label: String,
    pub region: String,
    pub plan: String,
    pub os: String,
    pub main_ip: String,
    pub status: String,
    pub power_status: String,
    pub server_status: String,
    pub ssh_state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VultrInstanceDetail {
    pub id: String,
    pub label: String,
    pub region: String,
    pub plan: String,
    pub os: String,
    pub os_id: Option<u64>,
    pub main_ip: String,
    pub internal_ip: String,
    pub status: String,
    pub power_status: String,
    pub server_status: String,
    pub vcpu_count: Option<u64>,
    pub ram_mb: Option<u64>,
    pub disk_gb: Option<u64>,
    pub date_created: Option<String>,
    pub ssh_state: String,
    pub access_profile: Option<InstanceAccessProfileView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteCommandResult {
    pub instance_id: String,
    pub host: String,
    pub port: i64,
    pub username: String,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub timed_out: bool,
    pub output: String,
}

#[derive(Debug)]
pub(crate) enum VultrError {
    MissingCredential,
    MissingEncryptionKey,
    InvalidKey(String),
    RateLimited(String),
    NotFound(String),
    InvalidInput(String),
    SshProfileMissing(String),
    SshAuthFailed(String),
    Unsupported(String),
    Network(String),
    Api(String),
}

impl std::fmt::Display for VultrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingCredential => write!(
                f,
                "Vultr access is not connected. Open {API_ACCESS_URL} and paste an API key into Settings > Vultr."
            ),
            Self::MissingEncryptionKey => write!(
                f,
                "Server is missing VULTR_CREDENTIAL_ENCRYPTION_KEY. Configure a 32-byte base64 key before saving Vultr credentials."
            ),
            Self::InvalidKey(msg)
            | Self::RateLimited(msg)
            | Self::NotFound(msg)
            | Self::InvalidInput(msg)
            | Self::SshProfileMissing(msg)
            | Self::SshAuthFailed(msg)
            | Self::Unsupported(msg)
            | Self::Network(msg)
            | Self::Api(msg) => write!(f, "{msg}"),
        }
    }
}

impl VultrError {
    fn code(&self) -> &'static str {
        match self {
            Self::MissingCredential => "missing",
            Self::MissingEncryptionKey => "server_misconfigured",
            Self::InvalidKey(_) => "invalid",
            Self::RateLimited(_) => "rate_limited",
            Self::NotFound(_) => "not_found",
            Self::InvalidInput(_) => "invalid_input",
            Self::SshProfileMissing(_) => "ssh_missing",
            Self::SshAuthFailed(_) => "ssh_failed",
            Self::Unsupported(_) => "unsupported",
            Self::Network(_) => "network_error",
            Self::Api(_) => "api_error",
        }
    }

    fn status_code(&self) -> u16 {
        match self {
            Self::MissingCredential | Self::SshProfileMissing(_) => 400,
            Self::InvalidKey(_) => 401,
            Self::RateLimited(_) => 429,
            Self::NotFound(_) => 404,
            Self::InvalidInput(_) => 400,
            Self::SshAuthFailed(_) => 401,
            Self::Unsupported(_) => 501,
            Self::MissingEncryptionKey | Self::Network(_) | Self::Api(_) => 500,
        }
    }
}

#[derive(Debug, Deserialize)]
struct PutIntegrationInput {
    api_key: String,
    #[serde(default)]
    label: String,
}

#[derive(Debug, Deserialize)]
struct VerifyIntegrationInput {
    #[serde(default)]
    api_key: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DeployInstanceInput {
    pub label: String,
    pub region: String,
    pub plan: String,
    pub os_id: u64,
    #[serde(default)]
    pub ssh_key_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PowerActionInput {
    action: String,
}

#[derive(Debug, Deserialize)]
struct ImportSshKeyInput {
    name: String,
    public_key: String,
}

#[derive(Debug, Deserialize)]
struct SaveAccessProfileInput {
    #[serde(default)]
    instance_label: String,
    host: String,
    #[serde(default = "default_ssh_port")]
    port: i64,
    #[serde(default = "default_ssh_username")]
    username: String,
    auth_mode: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    private_key: String,
    #[serde(default)]
    public_key: String,
}

#[derive(Debug, Deserialize)]
struct TestAccessProfileInput {
    #[serde(default)]
    command: String,
}

#[derive(Debug, Deserialize)]
struct VultrListResponse<T> {
    #[serde(default)]
    instances: Vec<T>,
    #[serde(default)]
    regions: Vec<T>,
    #[serde(default)]
    plans: Vec<T>,
    #[serde(default)]
    os: Vec<T>,
    #[serde(default)]
    ssh_keys: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct ApiMeta {
    #[serde(default)]
    total: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct InstanceApiRecord {
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    region: String,
    #[serde(default)]
    plan: String,
    #[serde(default)]
    os: String,
    #[serde(default)]
    os_id: Option<u64>,
    #[serde(default)]
    main_ip: String,
    #[serde(default)]
    internal_ip: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    power_status: String,
    #[serde(default)]
    server_status: String,
    #[serde(default)]
    vcpu_count: Option<u64>,
    #[serde(default)]
    ram: Option<u64>,
    #[serde(default)]
    disk: Option<u64>,
    #[serde(default)]
    date_created: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InstanceDetailEnvelope {
    instance: InstanceApiRecord,
}

#[derive(Debug, Deserialize)]
struct AccountEnvelope {
    account: Value,
}

#[derive(Debug, Default, Deserialize)]
struct RegionApiRecord {
    id: String,
    #[serde(default)]
    city: String,
    #[serde(default)]
    country: String,
}

#[derive(Debug, Default, Deserialize)]
struct PlanApiRecord {
    id: String,
    #[serde(default)]
    vcpu_count: Option<u64>,
    #[serde(default)]
    ram: Option<u64>,
    #[serde(default)]
    disk: Option<u64>,
    #[serde(default)]
    monthly_cost: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
struct OsApiRecord {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    family: String,
}

#[derive(Debug, Default, Deserialize)]
struct SshKeyApiRecord {
    id: String,
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct SshKeyDetailEnvelope {
    ssh_key: SshKeyApiRecord,
}

#[derive(Debug, Deserialize)]
struct ApiErrorEnvelope {
    #[serde(default)]
    error: String,
}

#[derive(Debug, Clone)]
struct CachedCatalog {
    fetched_at: Instant,
    entries: Vec<VultrCatalogEntry>,
}

fn default_ssh_port() -> i64 {
    22
}

fn default_ssh_username() -> String {
    "root".to_string()
}

fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn cache_map() -> &'static Mutex<HashMap<String, CachedCatalog>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedCatalog>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn json_error(status: u16, error: impl Into<String>, code: Option<&str>) -> Response {
    let error = error.into();
    Response::json(
        json!({
            "ok": false,
            "error": error,
            "code": code,
        })
        .to_string(),
    )
    .with_status(status)
}

fn server_error(error: impl ToString) -> Response {
    json_error(500, error.to_string(), Some("server_error"))
}

fn current_user(req: &Request) -> Result<db::User, Response> {
    match auth::current_user(req) {
        Ok(Some(user)) => Ok(user),
        Ok(None) => Err(json_error(401, "unauthenticated", Some("unauthenticated"))),
        Err(error) => Err(server_error(error)),
    }
}

fn secret_cipher() -> Result<Aes256GcmSiv, VultrError> {
    let raw = std::env::var("VULTR_CREDENTIAL_ENCRYPTION_KEY")
        .map_err(|_| VultrError::MissingEncryptionKey)?;
    let key_bytes = B64
        .decode(raw.trim())
        .map_err(|_| VultrError::MissingEncryptionKey)?;
    if key_bytes.len() != 32 {
        return Err(VultrError::MissingEncryptionKey);
    }
    Aes256GcmSiv::new_from_slice(&key_bytes).map_err(|_| VultrError::MissingEncryptionKey)
}

fn encrypt_secret(secret: &str) -> Result<String, VultrError> {
    let cipher = secret_cipher()?;
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), secret.as_bytes())
        .map_err(|_| VultrError::Api("Could not encrypt secret".into()))?;
    Ok(format!("v1:{}:{}", B64.encode(nonce), B64.encode(ciphertext)))
}

fn decrypt_secret(ciphertext: &str) -> Result<String, VultrError> {
    let mut parts = ciphertext.splitn(3, ':');
    let version = parts.next().unwrap_or_default();
    let nonce_b64 = parts.next().unwrap_or_default();
    let cipher_b64 = parts.next().unwrap_or_default();
    if version != "v1" || nonce_b64.is_empty() || cipher_b64.is_empty() {
        return Err(VultrError::Api("Stored secret format is invalid".into()));
    }
    let nonce_bytes = B64
        .decode(nonce_b64)
        .map_err(|_| VultrError::Api("Stored secret nonce is invalid".into()))?;
    if nonce_bytes.len() != 12 {
        return Err(VultrError::Api("Stored secret nonce has invalid length".into()));
    }
    let ciphertext_bytes = B64
        .decode(cipher_b64)
        .map_err(|_| VultrError::Api("Stored secret ciphertext is invalid".into()))?;
    let cipher = secret_cipher()?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce_bytes), ciphertext_bytes.as_ref())
        .map_err(|_| VultrError::Api("Could not decrypt stored secret".into()))?;
    String::from_utf8(plaintext)
        .map_err(|_| VultrError::Api("Stored secret contains invalid UTF-8".into()))
}

fn api_key_last4(api_key: &str) -> String {
    let trimmed = api_key.trim();
    let chars = trimmed.chars().collect::<Vec<_>>();
    let start = chars.len().saturating_sub(4);
    chars[start..].iter().collect()
}

fn account_status(account: Option<&VultrAccount>) -> VultrConnectionStatus {
    match account {
        Some(account) => {
            let status = if account.last_error.as_deref().unwrap_or_default().is_empty() {
                "connected"
            } else {
                classify_account_error(account.last_error.as_deref())
            };
            VultrConnectionStatus {
                status: status.to_string(),
                connected: status == "connected",
                label: if account.label.trim().is_empty() {
                    None
                } else {
                    Some(account.label.clone())
                },
                api_key_last4: Some(account.api_key_last4.clone()),
                verified_at: account.verified_at,
                last_error: account.last_error.clone(),
                api_access_url: API_ACCESS_URL,
            }
        }
        None => VultrConnectionStatus {
            status: "missing".into(),
            connected: false,
            label: None,
            api_key_last4: None,
            verified_at: None,
            last_error: None,
            api_access_url: API_ACCESS_URL,
        },
    }
}

fn classify_account_error(last_error: Option<&str>) -> &'static str {
    let error = last_error.unwrap_or_default().to_lowercase();
    if error.contains("rate limit") {
        "rate_limited"
    } else if error.contains("invalid") || error.contains("revoked") || error.contains("unauthorized") {
        "invalid"
    } else {
        "invalid"
    }
}

fn profile_view(profile: &InstanceAccessProfile) -> InstanceAccessProfileView {
    let ssh_state = if profile.encrypted_secret.is_empty() {
        "ssh_missing"
    } else if profile.last_error.as_deref().unwrap_or_default().is_empty() {
        "ssh_ready"
    } else {
        "ssh_failed"
    };
    InstanceAccessProfileView {
        instance_id: profile.instance_id.clone(),
        instance_label: profile.instance_label.clone(),
        host: profile.host.clone(),
        port: profile.port,
        username: profile.username.clone(),
        auth_mode: profile.auth_mode.clone(),
        public_key: profile.public_key.clone(),
        has_secret: !profile.encrypted_secret.is_empty(),
        ssh_state: ssh_state.to_string(),
        last_verified_at: profile.last_verified_at,
        last_error: profile.last_error.clone(),
        updated_at: profile.updated_at,
    }
}

fn instance_ssh_state(
    profiles: &HashMap<String, InstanceAccessProfile>,
    instance_id: &str,
) -> String {
    match profiles.get(instance_id) {
        Some(profile) if profile.encrypted_secret.is_empty() => "ssh_missing".into(),
        Some(profile) if profile.last_error.as_deref().unwrap_or_default().is_empty() => {
            "ssh_ready".into()
        }
        Some(_) => "ssh_failed".into(),
        None => "ssh_missing".into(),
    }
}

fn ensure_connected_account(user_id: i64) -> Result<(VultrAccount, String), VultrError> {
    let account = db::auth_store()
        .get_vultr_account(user_id)
        .map_err(|e| VultrError::Api(e.to_string()))?
        .ok_or(VultrError::MissingCredential)?;
    let api_key = decrypt_secret(&account.encrypted_api_key)?;
    Ok((account, api_key))
}

fn vultr_request(method: &str, path: &str, api_key: &str, body: Option<Value>) -> Result<Value, VultrError> {
    let url = format!("{VULTR_API_BASE}{path}");
    let agent = ureq::AgentBuilder::new()
        .timeout_read(Duration::from_secs(30))
        .timeout_write(Duration::from_secs(30))
        .timeout_connect(Duration::from_secs(10))
        .build();
    let mut req = agent
        .request(method, &url)
        .set("Authorization", &format!("Bearer {}", api_key.trim()))
        .set("Accept", "application/json");
    if body.is_some() {
        req = req.set("Content-Type", "application/json");
    }
    let response = match body {
        Some(payload) => req.send_string(&payload.to_string()),
        None => req.call(),
    };
    match response {
        Ok(resp) => {
            let text = resp
                .into_string()
                .map_err(|e| VultrError::Api(format!("Could not read Vultr response: {e}")))?;
            if text.trim().is_empty() {
                Ok(json!({}))
            } else {
                serde_json::from_str::<Value>(&text)
                    .map_err(|e| VultrError::Api(format!("Could not decode Vultr response: {e}")))
            }
        }
        Err(ureq::Error::Status(code, resp)) => {
            let raw = resp
                .into_string()
                .unwrap_or_else(|_| String::from("unknown Vultr API error"));
            let message = serde_json::from_str::<ApiErrorEnvelope>(&raw)
                .ok()
                .map(|body| body.error)
                .filter(|text| !text.trim().is_empty())
                .unwrap_or(raw);
            match code {
                401 | 403 => Err(VultrError::InvalidKey(format!(
                    "Vultr API key is invalid, revoked, or unauthorized: {message}"
                ))),
                404 => Err(VultrError::NotFound(format!("Vultr resource not found: {message}"))),
                429 => Err(VultrError::RateLimited(format!("Vultr rate limit hit: {message}"))),
                _ => Err(VultrError::Api(format!("Vultr API error ({code}): {message}"))),
            }
        }
        Err(ureq::Error::Transport(err)) => {
            Err(VultrError::Network(format!("Could not reach Vultr API: {err}")))
        }
    }
}

fn verify_api_key(api_key: &str) -> Result<(), VultrError> {
    let value = vultr_request("GET", "/account", api_key, None)?;
    let _: AccountEnvelope = serde_json::from_value(value)
        .map_err(|e| VultrError::Api(format!("Unexpected Vultr account response: {e}")))?;
    Ok(())
}

fn cached_catalog(key: &str) -> Option<Vec<VultrCatalogEntry>> {
    let map = cache_map().lock().unwrap();
    let cached = map.get(key)?;
    if cached.fetched_at.elapsed() > Duration::from_secs(CATALOG_TTL_SECS) {
        return None;
    }
    Some(cached.entries.clone())
}

fn store_catalog(key: &str, entries: Vec<VultrCatalogEntry>) {
    cache_map().lock().unwrap().insert(
        key.to_string(),
        CachedCatalog {
            fetched_at: Instant::now(),
            entries,
        },
    );
}

fn list_regions_api(api_key: &str) -> Result<Vec<VultrCatalogEntry>, VultrError> {
    if let Some(entries) = cached_catalog("regions") {
        return Ok(entries);
    }
    let value = vultr_request("GET", "/regions", api_key, None)?;
    let list: VultrListResponse<RegionApiRecord> = serde_json::from_value(value)
        .map_err(|e| VultrError::Api(format!("Unexpected regions payload: {e}")))?;
    let entries = list
        .regions
        .into_iter()
        .map(|region| VultrCatalogEntry {
            id: region.id.clone(),
            label: region.id,
            description: [region.city, region.country]
                .into_iter()
                .filter(|part| !part.trim().is_empty())
                .collect::<Vec<_>>()
                .join(", "),
        })
        .collect::<Vec<_>>();
    store_catalog("regions", entries.clone());
    Ok(entries)
}

fn list_plans_api(api_key: &str) -> Result<Vec<VultrCatalogEntry>, VultrError> {
    if let Some(entries) = cached_catalog("plans") {
        return Ok(entries);
    }
    let value = vultr_request("GET", "/plans", api_key, None)?;
    let list: VultrListResponse<PlanApiRecord> = serde_json::from_value(value)
        .map_err(|e| VultrError::Api(format!("Unexpected plans payload: {e}")))?;
    let entries = list
        .plans
        .into_iter()
        .map(|plan| VultrCatalogEntry {
            id: plan.id.clone(),
            label: plan.id,
            description: format!(
                "{} vCPU, {} MB RAM, {} GB disk{}",
                plan.vcpu_count.unwrap_or(0),
                plan.ram.unwrap_or(0),
                plan.disk.unwrap_or(0),
                plan.monthly_cost
                    .map(|cost| format!(", ${cost:.2}/mo"))
                    .unwrap_or_default()
            ),
        })
        .collect::<Vec<_>>();
    store_catalog("plans", entries.clone());
    Ok(entries)
}

fn list_os_api(api_key: &str) -> Result<Vec<VultrCatalogEntry>, VultrError> {
    if let Some(entries) = cached_catalog("os") {
        return Ok(entries);
    }
    let value = vultr_request("GET", "/os", api_key, None)?;
    let list: VultrListResponse<OsApiRecord> = serde_json::from_value(value)
        .map_err(|e| VultrError::Api(format!("Unexpected OS payload: {e}")))?;
    let entries = list
        .os
        .into_iter()
        .map(|os| VultrCatalogEntry {
            id: os.id.to_string(),
            label: os.name,
            description: os.family,
        })
        .collect::<Vec<_>>();
    store_catalog("os", entries.clone());
    Ok(entries)
}

fn list_ssh_keys_api(api_key: &str) -> Result<Vec<VultrCatalogEntry>, VultrError> {
    let value = vultr_request("GET", "/ssh-keys", api_key, None)?;
    let list: VultrListResponse<SshKeyApiRecord> = serde_json::from_value(value)
        .map_err(|e| VultrError::Api(format!("Unexpected SSH key payload: {e}")))?;
    Ok(list
        .ssh_keys
        .into_iter()
        .map(|key| VultrCatalogEntry {
            id: key.id,
            label: key.name,
            description: "Account SSH key".into(),
        })
        .collect())
}

fn create_ssh_key_api(api_key: &str, name: &str, public_key: &str) -> Result<VultrCatalogEntry, VultrError> {
    let value = vultr_request(
        "POST",
        "/ssh-keys",
        api_key,
        Some(json!({ "name": name, "ssh_key": public_key })),
    )?;
    let created: SshKeyDetailEnvelope = serde_json::from_value(value)
        .map_err(|e| VultrError::Api(format!("Unexpected create SSH key payload: {e}")))?;
    Ok(VultrCatalogEntry {
        id: created.ssh_key.id,
        label: created.ssh_key.name,
        description: "Account SSH key".into(),
    })
}

fn list_instances_api(
    api_key: &str,
    profiles: &HashMap<String, InstanceAccessProfile>,
) -> Result<Vec<VultrInstanceSummary>, VultrError> {
    let value = vultr_request("GET", "/instances", api_key, None)?;
    let list: VultrListResponse<InstanceApiRecord> = serde_json::from_value(value)
        .map_err(|e| VultrError::Api(format!("Unexpected instances payload: {e}")))?;
    Ok(list
        .instances
        .into_iter()
        .map(|instance| VultrInstanceSummary {
            ssh_state: instance_ssh_state(profiles, &instance.id),
            id: instance.id,
            label: instance.label,
            region: instance.region,
            plan: instance.plan,
            os: instance.os,
            main_ip: instance.main_ip,
            status: instance.status,
            power_status: instance.power_status,
            server_status: instance.server_status,
        })
        .collect())
}

fn get_instance_api(
    api_key: &str,
    instance_id: &str,
    profile: Option<InstanceAccessProfile>,
) -> Result<VultrInstanceDetail, VultrError> {
    let value = vultr_request("GET", &format!("/instances/{instance_id}"), api_key, None)?;
    let detail: InstanceDetailEnvelope = serde_json::from_value(value)
        .map_err(|e| VultrError::Api(format!("Unexpected instance detail payload: {e}")))?;
    let ssh_state = profile
        .as_ref()
        .map(profile_view)
        .map(|view| view.ssh_state.clone())
        .unwrap_or_else(|| "ssh_missing".into());
    Ok(VultrInstanceDetail {
        id: detail.instance.id,
        label: detail.instance.label,
        region: detail.instance.region,
        plan: detail.instance.plan,
        os: detail.instance.os,
        os_id: detail.instance.os_id,
        main_ip: detail.instance.main_ip,
        internal_ip: detail.instance.internal_ip,
        status: detail.instance.status,
        power_status: detail.instance.power_status,
        server_status: detail.instance.server_status,
        vcpu_count: detail.instance.vcpu_count,
        ram_mb: detail.instance.ram,
        disk_gb: detail.instance.disk,
        date_created: detail.instance.date_created,
        ssh_state,
        access_profile: profile.as_ref().map(profile_view),
    })
}

fn deploy_instance_api(api_key: &str, input: &DeployInstanceInput) -> Result<VultrInstanceDetail, VultrError> {
    if input.label.trim().is_empty()
        || input.region.trim().is_empty()
        || input.plan.trim().is_empty()
    {
        return Err(VultrError::InvalidInput(
            "Deploy requires label, region, and plan".into(),
        ));
    }
    let body = json!({
        "label": input.label.trim(),
        "region": input.region.trim(),
        "plan": input.plan.trim(),
        "os_id": input.os_id,
        "ssh_key_ids": input.ssh_key_ids,
    });
    let value = vultr_request("POST", "/instances", api_key, Some(body))?;
    let detail: InstanceDetailEnvelope = serde_json::from_value(value)
        .map_err(|e| VultrError::Api(format!("Unexpected deploy payload: {e}")))?;
    Ok(VultrInstanceDetail {
        id: detail.instance.id,
        label: detail.instance.label,
        region: detail.instance.region,
        plan: detail.instance.plan,
        os: detail.instance.os,
        os_id: detail.instance.os_id,
        main_ip: detail.instance.main_ip,
        internal_ip: detail.instance.internal_ip,
        status: detail.instance.status,
        power_status: detail.instance.power_status,
        server_status: detail.instance.server_status,
        vcpu_count: detail.instance.vcpu_count,
        ram_mb: detail.instance.ram,
        disk_gb: detail.instance.disk,
        date_created: detail.instance.date_created,
        ssh_state: "ssh_missing".into(),
        access_profile: None,
    })
}

fn power_action_api(api_key: &str, instance_id: &str, action: &str) -> Result<(), VultrError> {
    let normalized = action.trim().to_lowercase();
    let endpoint = match normalized.as_str() {
        "start" => "start",
        "stop" => "halt",
        "reboot" => "reboot",
        other => {
            return Err(VultrError::InvalidInput(format!(
                "Unsupported power action '{other}'. Use start, stop, or reboot."
            )))
        }
    };
    let _ = vultr_request(
        "POST",
        &format!("/instances/{instance_id}/{endpoint}"),
        api_key,
        Some(json!({})),
    )?;
    Ok(())
}

fn load_profiles_map(user_id: i64) -> Result<HashMap<String, InstanceAccessProfile>, VultrError> {
    let profiles = db::auth_store()
        .list_instance_access_profiles(user_id)
        .map_err(|e| VultrError::Api(e.to_string()))?;
    Ok(profiles
        .into_iter()
        .map(|profile| (profile.instance_id.clone(), profile))
        .collect())
}

fn load_profile(user_id: i64, instance_id: &str) -> Result<InstanceAccessProfile, VultrError> {
    db::auth_store()
        .get_instance_access_profile(user_id, instance_id)
        .map_err(|e| VultrError::Api(e.to_string()))?
        .ok_or_else(|| {
            VultrError::SshProfileMissing(format!(
                "No SSH access profile is saved for instance '{instance_id}'. Open the Vultr panel, attach access, and test the connection first."
            ))
        })
}

fn sshpass_available() -> bool {
    Command::new("sshpass")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

fn write_temp_private_key(private_key: &str) -> Result<PathBuf, VultrError> {
    let mut suffix = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut suffix);
    let path = std::env::temp_dir().join(format!(
        "mixer-vultr-key-{}-{}.tmp",
        unix_secs(),
        B64.encode(suffix)
            .replace('/', "_")
            .replace('+', "-")
            .replace('=', "")
    ));
    std::fs::write(&path, private_key)
        .map_err(|e| VultrError::Api(format!("Could not write temp SSH key: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

fn run_remote_command_internal(
    profile: &InstanceAccessProfile,
    command: &str,
    timeout_secs: u64,
) -> Result<RemoteCommandResult, VultrError> {
    let host = profile.host.trim();
    if host.is_empty() {
        return Err(VultrError::InvalidInput(
            "SSH profile is missing a host/IP address".into(),
        ));
    }
    let timeout_secs = timeout_secs.clamp(1, 120);
    let started = Instant::now();
    let mut temp_key_path: Option<PathBuf> = None;
    let mut cmd = if profile.auth_mode == "password" {
        if !sshpass_available() {
            return Err(VultrError::Unsupported(
                "Password SSH needs sshpass on the server host, but sshpass is not installed. Save an SSH key or install sshpass on the Mixer host.".into(),
            ));
        }
        let secret = decrypt_secret(&profile.encrypted_secret)?;
        let mut cmd = Command::new("sshpass");
        cmd.arg("-e");
        cmd.env("SSHPASS", secret);
        cmd.arg("ssh");
        cmd
    } else if profile.auth_mode == "ssh_key" {
        let secret = decrypt_secret(&profile.encrypted_secret)?;
        let key_path = write_temp_private_key(&secret)?;
        temp_key_path = Some(key_path.clone());
        let mut cmd = Command::new("ssh");
        cmd.arg("-i").arg(key_path);
        cmd
    } else {
        return Err(VultrError::InvalidInput(format!(
            "Unsupported SSH auth mode '{}'",
            profile.auth_mode
        )));
    };

    cmd.arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-o")
        .arg("UserKnownHostsFile=/dev/null")
        .arg("-o")
        .arg(format!("ConnectTimeout={}", timeout_secs.min(10)))
        .arg("-p")
        .arg(profile.port.to_string())
        .arg(format!("{}@{}", profile.username.trim(), host))
        .arg(command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            if let Some(path) = temp_key_path.as_ref() {
                let _ = std::fs::remove_file(path);
            }
            return Err(VultrError::SshAuthFailed(format!(
                "Could not start SSH command: {e}"
            )));
        }
    };

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| VultrError::Api("Missing SSH stdout".into()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| VultrError::Api("Missing SSH stderr".into()))?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx_out = tx.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 1024];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = tx_out.send(String::from_utf8_lossy(&buf[..n]).into_owned());
                }
                Err(_) => break,
            }
        }
    });
    thread::spawn(move || {
        let mut buf = [0u8; 1024];
        loop {
            match stderr.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
                }
                Err(_) => break,
            }
        }
    });

    let mut combined = String::new();
    let mut exit_code = -1;
    let mut timed_out = false;
    loop {
        while let Ok(chunk) = rx.try_recv() {
            combined.push_str(&chunk);
            if combined.len() > REMOTE_OUTPUT_LIMIT {
                combined.truncate(REMOTE_OUTPUT_LIMIT);
                combined.push_str("\n[output truncated]");
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = status.code().unwrap_or(-1);
                break;
            }
            Ok(None) => {
                if started.elapsed() >= Duration::from_secs(timeout_secs) {
                    let _ = child.kill();
                    let _ = child.wait();
                    exit_code = 124;
                    timed_out = true;
                    if !combined.ends_with('\n') {
                        combined.push('\n');
                    }
                    combined.push_str(&format!("Remote command timed out after {timeout_secs}s"));
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                return Err(VultrError::SshAuthFailed(format!(
                    "SSH command failed while waiting: {error}"
                )));
            }
        }
    }
    while let Ok(chunk) = rx.try_recv() {
        combined.push_str(&chunk);
    }
    let output = tools::redact_sensitive(&combined);
    if let Some(path) = temp_key_path.as_ref() {
        let _ = std::fs::remove_file(path);
    }
    let result = RemoteCommandResult {
        instance_id: profile.instance_id.clone(),
        host: profile.host.clone(),
        port: profile.port,
        username: profile.username.clone(),
        exit_code,
        duration_ms: started.elapsed().as_millis() as u64,
        timed_out,
        output: output.clone(),
    };
    if exit_code == 255 {
        return Err(VultrError::SshAuthFailed(format!(
            "SSH authentication failed or the host is unreachable for {}@{}:{}",
            profile.username, profile.host, profile.port
        )));
    }
    Ok(result)
}

fn save_verified_profile(user_id: i64, mut profile: InstanceAccessProfile) -> Result<(), VultrError> {
    profile.updated_at = unix_secs();
    db::auth_store()
        .upsert_instance_access_profile(user_id, &profile)
        .map_err(|e| VultrError::Api(e.to_string()))
}

pub async fn get_integration(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(user) => user,
        Err(response) => return response,
    };
    match db::auth_store().get_vultr_account(user.id) {
        Ok(account) => Response::json(json!({ "ok": true, "connection": account_status(account.as_ref()) }).to_string()),
        Err(error) => server_error(error),
    }
}

pub async fn put_integration(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(user) => user,
        Err(response) => return response,
    };
    let input: PutIntegrationInput = match serde_json::from_slice(&req.body) {
        Ok(value) => value,
        Err(_) => return json_error(400, "invalid json body", Some("invalid_json")),
    };
    let now = unix_secs();
    let api_key = input.api_key.trim();
    let existing = match db::auth_store().get_vultr_account(user.id) {
        Ok(account) => account,
        Err(error) => return server_error(error),
    };
    let account = if api_key.is_empty() {
        let Some(mut existing_account) = existing else {
            return json_error(400, "api_key is required", Some("invalid_input"));
        };
        existing_account.label = input.label.trim().to_string();
        existing_account.updated_at = now;
        existing_account
    } else {
        let mut new_account = VultrAccount {
            encrypted_api_key: match encrypt_secret(api_key) {
                Ok(value) => value,
                Err(error) => {
                    return json_error(error.status_code(), error.to_string(), Some(error.code()))
                }
            },
            api_key_last4: api_key_last4(api_key),
            label: input.label.trim().to_string(),
            verified_at: None,
            last_error: None,
            created_at: existing.as_ref().map(|account| account.created_at).unwrap_or(now),
            updated_at: now,
        };
        match verify_api_key(api_key) {
            Ok(()) => new_account.verified_at = Some(now),
            Err(error) => new_account.last_error = Some(error.to_string()),
        }
        new_account
    };
    if let Err(error) = db::auth_store().upsert_vultr_account(user.id, &account) {
        return server_error(error);
    }
    Response::json(
        json!({
            "ok": true,
            "connection": account_status(Some(&account)),
        })
        .to_string(),
    )
}

pub async fn verify_integration(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(user) => user,
        Err(response) => return response,
    };
    let input: VerifyIntegrationInput = serde_json::from_slice(&req.body).unwrap_or(VerifyIntegrationInput {
        api_key: String::new(),
    });
    let api_key = if input.api_key.trim().is_empty() {
        match ensure_connected_account(user.id) {
            Ok((_account, api_key)) => api_key,
            Err(error) => return json_error(error.status_code(), error.to_string(), Some(error.code())),
        }
    } else {
        input.api_key.trim().to_string()
    };
    let now = unix_secs();
    match verify_api_key(&api_key) {
        Ok(()) => {
            if let Ok(Some(mut account)) = db::auth_store().get_vultr_account(user.id) {
                account.verified_at = Some(now);
                account.last_error = None;
                account.updated_at = now;
                let _ = db::auth_store().upsert_vultr_account(user.id, &account);
            }
            Response::json(json!({ "ok": true, "status": "connected" }).to_string())
        }
        Err(error) => {
            if let Ok(Some(mut account)) = db::auth_store().get_vultr_account(user.id) {
                account.last_error = Some(error.to_string());
                account.updated_at = now;
                let _ = db::auth_store().upsert_vultr_account(user.id, &account);
            }
            json_error(error.status_code(), error.to_string(), Some(error.code()))
        }
    }
}

pub async fn delete_integration(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(user) => user,
        Err(response) => return response,
    };
    match db::auth_store().delete_vultr_account(user.id) {
        Ok(()) => {
            if let Err(error) = db::auth_store().delete_all_instance_access_profiles(user.id) {
                return server_error(error);
            }
            Response::json(json!({ "ok": true }).to_string())
        }
        Err(error) => server_error(error),
    }
}

pub async fn list_instances(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(user) => user,
        Err(response) => return response,
    };
    let api_key = match ensure_connected_account(user.id) {
        Ok((_account, api_key)) => api_key,
        Err(error) => return json_error(error.status_code(), error.to_string(), Some(error.code())),
    };
    let profiles = match load_profiles_map(user.id) {
        Ok(profiles) => profiles,
        Err(error) => return json_error(error.status_code(), error.to_string(), Some(error.code())),
    };
    match list_instances_api(&api_key, &profiles) {
        Ok(instances) => Response::json(json!({ "ok": true, "instances": instances }).to_string()),
        Err(error) => json_error(error.status_code(), error.to_string(), Some(error.code())),
    }
}

pub async fn get_instance(req: &Request, instance_id: &str) -> Response {
    let user = match current_user(req) {
        Ok(user) => user,
        Err(response) => return response,
    };
    let api_key = match ensure_connected_account(user.id) {
        Ok((_account, api_key)) => api_key,
        Err(error) => return json_error(error.status_code(), error.to_string(), Some(error.code())),
    };
    let profile = match db::auth_store().get_instance_access_profile(user.id, instance_id) {
        Ok(profile) => profile,
        Err(error) => return server_error(error),
    };
    match get_instance_api(&api_key, instance_id, profile) {
        Ok(instance) => Response::json(json!({ "ok": true, "instance": instance }).to_string()),
        Err(error) => json_error(error.status_code(), error.to_string(), Some(error.code())),
    }
}

pub async fn create_instance(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(user) => user,
        Err(response) => return response,
    };
    let input: DeployInstanceInput = match serde_json::from_slice(&req.body) {
        Ok(value) => value,
        Err(_) => return json_error(400, "invalid json body", Some("invalid_json")),
    };
    let api_key = match ensure_connected_account(user.id) {
        Ok((_account, api_key)) => api_key,
        Err(error) => return json_error(error.status_code(), error.to_string(), Some(error.code())),
    };
    match deploy_instance_api(&api_key, &input) {
        Ok(instance) => Response::json(json!({ "ok": true, "instance": instance }).to_string()),
        Err(error) => json_error(error.status_code(), error.to_string(), Some(error.code())),
    }
}

pub async fn power_action(req: &Request, instance_id: &str) -> Response {
    let user = match current_user(req) {
        Ok(user) => user,
        Err(response) => return response,
    };
    let input: PowerActionInput = match serde_json::from_slice(&req.body) {
        Ok(value) => value,
        Err(_) => return json_error(400, "invalid json body", Some("invalid_json")),
    };
    let api_key = match ensure_connected_account(user.id) {
        Ok((_account, api_key)) => api_key,
        Err(error) => return json_error(error.status_code(), error.to_string(), Some(error.code())),
    };
    match power_action_api(&api_key, instance_id, &input.action) {
        Ok(()) => Response::json(json!({ "ok": true }).to_string()),
        Err(error) => json_error(error.status_code(), error.to_string(), Some(error.code())),
    }
}

pub async fn list_regions(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(user) => user,
        Err(response) => return response,
    };
    let api_key = match ensure_connected_account(user.id) {
        Ok((_account, api_key)) => api_key,
        Err(error) => return json_error(error.status_code(), error.to_string(), Some(error.code())),
    };
    match list_regions_api(&api_key) {
        Ok(entries) => Response::json(json!({ "ok": true, "regions": entries }).to_string()),
        Err(error) => json_error(error.status_code(), error.to_string(), Some(error.code())),
    }
}

pub async fn list_plans(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(user) => user,
        Err(response) => return response,
    };
    let api_key = match ensure_connected_account(user.id) {
        Ok((_account, api_key)) => api_key,
        Err(error) => return json_error(error.status_code(), error.to_string(), Some(error.code())),
    };
    match list_plans_api(&api_key) {
        Ok(entries) => Response::json(json!({ "ok": true, "plans": entries }).to_string()),
        Err(error) => json_error(error.status_code(), error.to_string(), Some(error.code())),
    }
}

pub async fn list_os(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(user) => user,
        Err(response) => return response,
    };
    let api_key = match ensure_connected_account(user.id) {
        Ok((_account, api_key)) => api_key,
        Err(error) => return json_error(error.status_code(), error.to_string(), Some(error.code())),
    };
    match list_os_api(&api_key) {
        Ok(entries) => Response::json(json!({ "ok": true, "os": entries }).to_string()),
        Err(error) => json_error(error.status_code(), error.to_string(), Some(error.code())),
    }
}

pub async fn list_ssh_keys(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(user) => user,
        Err(response) => return response,
    };
    let api_key = match ensure_connected_account(user.id) {
        Ok((_account, api_key)) => api_key,
        Err(error) => return json_error(error.status_code(), error.to_string(), Some(error.code())),
    };
    match list_ssh_keys_api(&api_key) {
        Ok(entries) => Response::json(json!({ "ok": true, "ssh_keys": entries }).to_string()),
        Err(error) => json_error(error.status_code(), error.to_string(), Some(error.code())),
    }
}

pub async fn import_ssh_key(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(user) => user,
        Err(response) => return response,
    };
    let input: ImportSshKeyInput = match serde_json::from_slice(&req.body) {
        Ok(value) => value,
        Err(_) => return json_error(400, "invalid json body", Some("invalid_json")),
    };
    let api_key = match ensure_connected_account(user.id) {
        Ok((_account, api_key)) => api_key,
        Err(error) => return json_error(error.status_code(), error.to_string(), Some(error.code())),
    };
    match create_ssh_key_api(&api_key, input.name.trim(), input.public_key.trim()) {
        Ok(key) => Response::json(json!({ "ok": true, "ssh_key": key }).to_string()),
        Err(error) => json_error(error.status_code(), error.to_string(), Some(error.code())),
    }
}

pub async fn upsert_access_profile(req: &Request, instance_id: &str) -> Response {
    let user = match current_user(req) {
        Ok(user) => user,
        Err(response) => return response,
    };
    let input: SaveAccessProfileInput = match serde_json::from_slice(&req.body) {
        Ok(value) => value,
        Err(_) => return json_error(400, "invalid json body", Some("invalid_json")),
    };
    if input.host.trim().is_empty() {
        return json_error(400, "host is required", Some("invalid_input"));
    }
    if input.auth_mode != "password" && input.auth_mode != "ssh_key" {
        return json_error(400, "auth_mode must be password or ssh_key", Some("invalid_input"));
    }
    let now = unix_secs();
    let existing = match db::auth_store().get_instance_access_profile(user.id, instance_id) {
        Ok(profile) => profile,
        Err(error) => return server_error(error),
    };
    let requested_auth_mode = input.auth_mode.as_str();
    let secret_plain = if input.auth_mode == "password" {
        if !input.password.trim().is_empty() {
            input.password.trim().to_string()
        } else if let Some(profile) = &existing {
            if profile.auth_mode != requested_auth_mode {
                return json_error(400, "password is required when switching to password auth", Some("invalid_input"));
            }
            match decrypt_secret(&profile.encrypted_secret) {
                Ok(value) => value,
                Err(error) => return json_error(error.status_code(), error.to_string(), Some(error.code())),
            }
        } else {
            return json_error(400, "password is required for password auth", Some("invalid_input"));
        }
    } else if !input.private_key.trim().is_empty() {
        input.private_key.to_string()
    } else if let Some(profile) = &existing {
        if profile.auth_mode != requested_auth_mode {
            return json_error(400, "private_key is required when switching to ssh_key auth", Some("invalid_input"));
        }
        match decrypt_secret(&profile.encrypted_secret) {
            Ok(value) => value,
            Err(error) => return json_error(error.status_code(), error.to_string(), Some(error.code())),
        }
    } else {
        return json_error(400, "private_key is required for ssh_key auth", Some("invalid_input"));
    };

    let profile = InstanceAccessProfile {
        instance_id: instance_id.to_string(),
        instance_label: input.instance_label.trim().to_string(),
        host: input.host.trim().to_string(),
        port: input.port.max(1),
        username: if input.username.trim().is_empty() {
            default_ssh_username()
        } else {
            input.username.trim().to_string()
        },
        auth_mode: input.auth_mode.clone(),
        encrypted_secret: match encrypt_secret(&secret_plain) {
            Ok(value) => value,
            Err(error) => return json_error(error.status_code(), error.to_string(), Some(error.code())),
        },
        public_key: input.public_key,
        last_verified_at: existing.as_ref().and_then(|profile| profile.last_verified_at),
        last_error: existing.as_ref().and_then(|profile| profile.last_error.clone()),
        created_at: existing.as_ref().map(|profile| profile.created_at).unwrap_or(now),
        updated_at: now,
    };
    match db::auth_store().upsert_instance_access_profile(user.id, &profile) {
        Ok(()) => Response::json(json!({ "ok": true, "profile": profile_view(&profile) }).to_string()),
        Err(error) => server_error(error),
    }
}

pub async fn delete_access_profile(req: &Request, instance_id: &str) -> Response {
    let user = match current_user(req) {
        Ok(user) => user,
        Err(response) => return response,
    };
    match db::auth_store().delete_instance_access_profile(user.id, instance_id) {
        Ok(()) => Response::json(json!({ "ok": true }).to_string()),
        Err(error) => server_error(error),
    }
}

pub async fn test_access_profile(req: &Request, instance_id: &str) -> Response {
    let user = match current_user(req) {
        Ok(user) => user,
        Err(response) => return response,
    };
    let input: TestAccessProfileInput = serde_json::from_slice(&req.body).unwrap_or(TestAccessProfileInput {
        command: String::new(),
    });
    let profile = match load_profile(user.id, instance_id) {
        Ok(profile) => profile,
        Err(error) => return json_error(error.status_code(), error.to_string(), Some(error.code())),
    };
    let test_command = if input.command.trim().is_empty() {
        "echo mixer-ssh-ok".to_string()
    } else {
        input.command.trim().to_string()
    };
    match run_remote_command_internal(&profile, &test_command, 20) {
        Ok(result) => {
            let mut updated = profile.clone();
            updated.last_verified_at = Some(unix_secs());
            updated.last_error = None;
            let _ = save_verified_profile(user.id, updated.clone());
            Response::json(
                json!({
                    "ok": true,
                    "profile": profile_view(&updated),
                    "result": result,
                })
                .to_string(),
            )
        }
        Err(error) => {
            let mut updated = profile.clone();
            updated.last_error = Some(error.to_string());
            updated.updated_at = unix_secs();
            let _ = db::auth_store().upsert_instance_access_profile(user.id, &updated);
            json_error(error.status_code(), error.to_string(), Some(error.code()))
        }
    }
}

pub(crate) fn tool_list_instances(user_id: i64) -> Result<Vec<VultrInstanceSummary>, VultrError> {
    let (_account, api_key) = ensure_connected_account(user_id)?;
    let profiles = load_profiles_map(user_id)?;
    list_instances_api(&api_key, &profiles)
}

pub(crate) fn tool_get_instance(user_id: i64, instance_id: &str) -> Result<VultrInstanceDetail, VultrError> {
    let (_account, api_key) = ensure_connected_account(user_id)?;
    let profile = db::auth_store()
        .get_instance_access_profile(user_id, instance_id)
        .map_err(|e| VultrError::Api(e.to_string()))?;
    get_instance_api(&api_key, instance_id, profile)
}

pub(crate) fn tool_deploy_instance(user_id: i64, input: DeployInstanceInput) -> Result<VultrInstanceDetail, VultrError> {
    let (_account, api_key) = ensure_connected_account(user_id)?;
    deploy_instance_api(&api_key, &input)
}

pub(crate) fn tool_power_action(user_id: i64, instance_id: &str, action: &str) -> Result<(), VultrError> {
    let (_account, api_key) = ensure_connected_account(user_id)?;
    power_action_api(&api_key, instance_id, action)
}

pub(crate) fn tool_list_regions(user_id: i64) -> Result<Vec<VultrCatalogEntry>, VultrError> {
    let (_account, api_key) = ensure_connected_account(user_id)?;
    list_regions_api(&api_key)
}

pub(crate) fn tool_list_plans(user_id: i64) -> Result<Vec<VultrCatalogEntry>, VultrError> {
    let (_account, api_key) = ensure_connected_account(user_id)?;
    list_plans_api(&api_key)
}

pub(crate) fn tool_list_os(user_id: i64) -> Result<Vec<VultrCatalogEntry>, VultrError> {
    let (_account, api_key) = ensure_connected_account(user_id)?;
    list_os_api(&api_key)
}

pub(crate) fn tool_list_ssh_keys(user_id: i64) -> Result<Vec<VultrCatalogEntry>, VultrError> {
    let (_account, api_key) = ensure_connected_account(user_id)?;
    list_ssh_keys_api(&api_key)
}

pub(crate) fn tool_import_ssh_key(user_id: i64, name: &str, public_key: &str) -> Result<VultrCatalogEntry, VultrError> {
    let (_account, api_key) = ensure_connected_account(user_id)?;
    create_ssh_key_api(&api_key, name, public_key)
}

pub(crate) fn tool_check_access(
    user_id: i64,
    instance_id: &str,
    verify: bool,
) -> Result<InstanceAccessProfileView, VultrError> {
    let profile = load_profile(user_id, instance_id)?;
    if !verify {
        return Ok(profile_view(&profile));
    }
    let result = run_remote_command_internal(&profile, "echo mixer-ssh-ok", 20)?;
    let mut updated = profile.clone();
    updated.last_verified_at = Some(unix_secs());
    updated.last_error = if result.exit_code == 0 {
        None
    } else {
        Some(format!("SSH check exited with {}", result.exit_code))
    };
    save_verified_profile(user_id, updated.clone())?;
    Ok(profile_view(&updated))
}

pub(crate) fn tool_remote_command(
    user_id: i64,
    instance_id: &str,
    command: &str,
    timeout_secs: u64,
) -> Result<RemoteCommandResult, VultrError> {
    let profile = load_profile(user_id, instance_id)?;
    let result = run_remote_command_internal(&profile, command, timeout_secs)?;
    let mut updated = profile.clone();
    updated.last_verified_at = Some(unix_secs());
    updated.last_error = if result.exit_code == 0 {
        None
    } else {
        Some(format!("Remote command exited with {}", result.exit_code))
    };
    let _ = save_verified_profile(user_id, updated);
    Ok(result)
}

pub(crate) fn format_tool_error(error: VultrError) -> String {
    format!("Failed: {}", error)
}
