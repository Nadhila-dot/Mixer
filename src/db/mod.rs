mod sqlite;

use std::fmt;

pub use sqlite::SqliteAuthStore;

#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub email: String,
    pub tier: String,
}

#[derive(Debug, Clone)]
pub struct UserWithPassword {
    pub user: User,
    pub password_hash: String,
}

pub trait AuthStore {
    fn find_user_by_email(&self, email: &str) -> Result<Option<UserWithPassword>, DbError>;
    fn create_user(&self, name: &str, email: &str, password_hash: &str) -> Result<User, DbError>;
    fn create_session(&self, token: &str, user_id: i64, expires_at: i64) -> Result<(), DbError>;
    fn user_by_session(&self, token: &str, now: i64) -> Result<Option<User>, DbError>;
    fn delete_session(&self, token: &str) -> Result<(), DbError>;
    fn usage_for_day(&self, user_id: i64, day: &str) -> Result<Usage, DbError>;
    fn increment_usage(
        &self,
        user_id: i64,
        day: &str,
        messages: i64,
        tokens: i64,
        updated_at: i64,
    ) -> Result<Usage, DbError>;
    fn recent_chats(&self, user_id: i64, limit: usize) -> Result<Vec<ChatSummary>, DbError>;
    fn chat_by_uuid(&self, user_id: i64, uuid: &str) -> Result<Option<ChatSummary>, DbError>;
    fn chat_messages(&self, user_id: i64, uuid: &str) -> Result<Vec<ChatMessage>, DbError>;
    fn create_chat(
        &self,
        user_id: i64,
        uuid: &str,
        title: &str,
        preview: &str,
        created_at: i64,
    ) -> Result<ChatSummary, DbError>;
    fn append_message(
        &self,
        user_id: i64,
        chat_uuid: &str,
        role: &str,
        content: &str,
        created_at: i64,
        model: Option<&str>,
    ) -> Result<ChatMessage, DbError>;
    fn delete_chat(&self, user_id: i64, uuid: &str) -> Result<bool, DbError>;

    fn create_workspace(
        &self,
        user_id: i64,
        uuid: &str,
        name: &str,
        created_at: i64,
    ) -> Result<Workspace, DbError>;
    fn get_workspace(&self, user_id: i64, uuid: &str) -> Result<Option<Workspace>, DbError>;
    fn get_workspace_any(&self, uuid: &str) -> Result<Option<Workspace>, DbError>;
    fn list_workspaces(&self, user_id: i64) -> Result<Vec<Workspace>, DbError>;
    fn update_message_content(&self, msg_uuid: &str, content: &str) -> Result<(), DbError>;

    // Agent runs
    fn create_run(&self, run: &AgentRun) -> Result<(), DbError>;
    fn update_run_status(
        &self,
        run_uuid: &str,
        status: &str,
        final_text: Option<&str>,
        error_summary: Option<&str>,
        assistant_message_uuid: Option<&str>,
        ended_at: Option<i64>,
    ) -> Result<(), DbError>;
    fn get_run(&self, user_id: i64, run_uuid: &str) -> Result<Option<AgentRun>, DbError>;
    fn runs_for_chat(&self, user_id: i64, chat_uuid: &str) -> Result<Vec<AgentRun>, DbError>;

    // Tool calls
    fn insert_tool_call(&self, call: &ToolCall) -> Result<(), DbError>;
    fn update_tool_call_status(
        &self,
        call_uuid: &str,
        status: &str,
        output: Option<&str>,
        error: Option<&str>,
        started_at: Option<i64>,
        ended_at: Option<i64>,
    ) -> Result<(), DbError>;
    fn tool_calls_for_run(&self, run_uuid: &str) -> Result<Vec<ToolCall>, DbError>;

    // Events
    fn insert_event(
        &self,
        run_uuid: &str,
        seq: i64,
        event_type: &str,
        payload_json: &str,
        created_at: i64,
    ) -> Result<(), DbError>;
    fn events_for_run_since(&self, run_uuid: &str, since: i64) -> Result<Vec<AgentEvent>, DbError>;
    fn max_event_seq(&self, run_uuid: &str) -> Result<i64, DbError>;

    // User settings + profile mutations
    fn get_user_settings(&self, user_id: i64) -> Result<UserSettings, DbError>;
    fn upsert_user_settings(&self, user_id: i64, settings: &UserSettings) -> Result<(), DbError>;
    fn update_user_name(&self, user_id: i64, name: &str) -> Result<(), DbError>;
    fn get_vultr_account(&self, user_id: i64) -> Result<Option<VultrAccount>, DbError>;
    fn upsert_vultr_account(&self, user_id: i64, account: &VultrAccount) -> Result<(), DbError>;
    fn delete_vultr_account(&self, user_id: i64) -> Result<(), DbError>;
    fn get_instance_access_profile(
        &self,
        user_id: i64,
        instance_id: &str,
    ) -> Result<Option<InstanceAccessProfile>, DbError>;
    fn list_instance_access_profiles(
        &self,
        user_id: i64,
    ) -> Result<Vec<InstanceAccessProfile>, DbError>;
    fn upsert_instance_access_profile(
        &self,
        user_id: i64,
        profile: &InstanceAccessProfile,
    ) -> Result<(), DbError>;
    fn delete_instance_access_profile(&self, user_id: i64, instance_id: &str) -> Result<(), DbError>;
    fn delete_all_instance_access_profiles(&self, user_id: i64) -> Result<(), DbError>;

    // Bulk destructive operations (used by the settings page)
    fn delete_all_chats_for_user(&self, user_id: i64) -> Result<usize, DbError>;
    fn delete_all_workspaces_for_user(&self, user_id: i64) -> Result<usize, DbError>;
}

#[derive(Debug, Clone)]
pub struct AgentRun {
    pub uuid: String,
    pub chat_uuid: String,
    pub user_id: i64,
    pub user_message_uuid: String,
    pub assistant_message_uuid: Option<String>,
    pub model: String,
    pub status: String,
    pub final_text: Option<String>,
    pub error_summary: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub uuid: String,
    pub run_uuid: String,
    pub parent_call_uuid: Option<String>,
    pub seq: i64,
    pub name: String,
    pub args_json: String,
    pub body: String,
    pub status: String,
    pub output: Option<String>,
    pub error: Option<String>,
    pub attempt: i64,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct AgentEvent {
    pub seq: i64,
    pub event_type: String,
    pub payload_json: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct Usage {
    pub messages: i64,
    pub tokens: i64,
    pub last_updated: i64,
}

#[derive(Debug, Clone)]
pub struct ChatSummary {
    pub uuid: String,
    pub title: String,
    pub preview: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub uuid: String,
    pub role: String,
    pub content: String,
    pub created_at: i64,
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub uuid: String,
    pub name: String,
    pub created_at: i64,
}

/// Per-user personalization stored in the user_settings table. Loaded for every
/// agent turn and folded into the system prompt so the model adapts its style.
#[derive(Debug, Clone)]
pub struct UserSettings {
    /// One of: default, concise, detailed, friendly, professional, witty, direct.
    pub base_style: String,
    /// JSON-encoded map of characteristic → preference, e.g.
    /// `{"warmth":"more","enthusiasm":"less","headers_lists":"default","emoji":"less"}`.
    /// We keep the wire format opaque-JSON so we can grow the set without DB migrations.
    pub characteristics_json: String,
    /// Free-text additional behavior preferences.
    pub custom_instructions: String,
    pub updated_at: i64,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            base_style: "default".into(),
            characteristics_json: "{}".into(),
            custom_instructions: String::new(),
            updated_at: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VultrAccount {
    pub encrypted_api_key: String,
    pub api_key_last4: String,
    pub label: String,
    pub verified_at: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct InstanceAccessProfile {
    pub instance_id: String,
    pub instance_label: String,
    pub host: String,
    pub port: i64,
    pub username: String,
    pub auth_mode: String,
    pub encrypted_secret: String,
    pub public_key: String,
    pub last_verified_at: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub enum DatabaseProvider {
    Sqlite,
    Postgres,
    Mysql,
    Mongo,
    Unsupported(String),
}

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub provider: DatabaseProvider,
    pub url: String,
}

impl DatabaseConfig {
    pub fn from_env() -> Self {
        let url = std::env::var("DATABASE_URL")
            .or_else(|_| std::env::var("AUTH_DB_URL"))
            .or_else(|_| std::env::var("AUTH_DB_PATH").map(|path| format!("sqlite://{path}")))
            .unwrap_or_else(|_| "sqlite://forge_auth.sqlite3".into());

        let provider = match url.split_once(':').map(|(scheme, _)| scheme) {
            Some("sqlite") | Some("file") => DatabaseProvider::Sqlite,
            Some("postgres") | Some("postgresql") => DatabaseProvider::Postgres,
            Some("mysql") | Some("mariadb") => DatabaseProvider::Mysql,
            Some("mongodb") | Some("mongo") => DatabaseProvider::Mongo,
            Some(other) => DatabaseProvider::Unsupported(other.to_string()),
            None => DatabaseProvider::Sqlite,
        };

        Self { provider, url }
    }
}

pub fn auth_store() -> Box<dyn AuthStore> {
    let config = DatabaseConfig::from_env();
    match config.provider {
        DatabaseProvider::Sqlite => Box::new(SqliteAuthStore::new(config.url)),
        DatabaseProvider::Postgres => Box::new(UnsupportedAuthStore::new(
            "postgresql",
            "PostgreSQL provider selected, but the postgres driver is not linked yet",
        )),
        DatabaseProvider::Mysql => Box::new(UnsupportedAuthStore::new(
            "mysql",
            "MySQL provider selected, but the mysql driver is not linked yet",
        )),
        DatabaseProvider::Mongo => Box::new(UnsupportedAuthStore::new(
            "mongo",
            "Mongo provider selected, but the mongodb driver is not linked yet",
        )),
        DatabaseProvider::Unsupported(provider) => Box::new(UnsupportedAuthStore::new(
            provider,
            "Unsupported database provider in DATABASE_URL",
        )),
    }
}

#[derive(Debug, Clone)]
pub enum DbError {
    DuplicateEmail,
    InvalidConfig(String),
    DriverUnavailable { provider: String, detail: String },
    Query(String),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::DuplicateEmail => write!(f, "email already registered"),
            DbError::InvalidConfig(message) => write!(f, "invalid database config: {message}"),
            DbError::DriverUnavailable { provider, detail } => {
                write!(f, "{provider} database unavailable: {detail}")
            }
            DbError::Query(message) => write!(f, "database query failed: {message}"),
        }
    }
}

impl std::error::Error for DbError {}

struct UnsupportedAuthStore {
    provider: String,
    detail: String,
}

impl UnsupportedAuthStore {
    fn new(provider: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            detail: detail.into(),
        }
    }

    fn error(&self) -> DbError {
        DbError::DriverUnavailable {
            provider: self.provider.clone(),
            detail: self.detail.clone(),
        }
    }
}

impl AuthStore for UnsupportedAuthStore {
    fn find_user_by_email(&self, _email: &str) -> Result<Option<UserWithPassword>, DbError> {
        Err(self.error())
    }

    fn create_user(
        &self,
        _name: &str,
        _email: &str,
        _password_hash: &str,
    ) -> Result<User, DbError> {
        Err(self.error())
    }

    fn create_session(&self, _token: &str, _user_id: i64, _expires_at: i64) -> Result<(), DbError> {
        Err(self.error())
    }

    fn user_by_session(&self, _token: &str, _now: i64) -> Result<Option<User>, DbError> {
        Err(self.error())
    }

    fn delete_session(&self, _token: &str) -> Result<(), DbError> {
        Err(self.error())
    }

    fn usage_for_day(&self, _user_id: i64, _day: &str) -> Result<Usage, DbError> {
        Err(self.error())
    }

    fn increment_usage(
        &self,
        _user_id: i64,
        _day: &str,
        _messages: i64,
        _tokens: i64,
        _updated_at: i64,
    ) -> Result<Usage, DbError> {
        Err(self.error())
    }

    fn recent_chats(&self, _user_id: i64, _limit: usize) -> Result<Vec<ChatSummary>, DbError> {
        Err(self.error())
    }

    fn chat_by_uuid(&self, _user_id: i64, _uuid: &str) -> Result<Option<ChatSummary>, DbError> {
        Err(self.error())
    }

    fn chat_messages(&self, _user_id: i64, _uuid: &str) -> Result<Vec<ChatMessage>, DbError> {
        Err(self.error())
    }

    fn create_chat(
        &self,
        _user_id: i64,
        _uuid: &str,
        _title: &str,
        _preview: &str,
        _created_at: i64,
    ) -> Result<ChatSummary, DbError> {
        Err(self.error())
    }

    fn append_message(
        &self,
        _user_id: i64,
        _chat_uuid: &str,
        _role: &str,
        _content: &str,
        _created_at: i64,
        _model: Option<&str>,
    ) -> Result<ChatMessage, DbError> {
        Err(self.error())
    }

    fn delete_chat(&self, _user_id: i64, _uuid: &str) -> Result<bool, DbError> {
        Err(self.error())
    }

    fn create_workspace(
        &self,
        _user_id: i64,
        _uuid: &str,
        _name: &str,
        _created_at: i64,
    ) -> Result<Workspace, DbError> {
        Err(self.error())
    }

    fn get_workspace(&self, _user_id: i64, _uuid: &str) -> Result<Option<Workspace>, DbError> {
        Err(self.error())
    }

    fn get_workspace_any(&self, _uuid: &str) -> Result<Option<Workspace>, DbError> {
        Err(self.error())
    }

    fn list_workspaces(&self, _user_id: i64) -> Result<Vec<Workspace>, DbError> {
        Err(self.error())
    }

    fn update_message_content(&self, _msg_uuid: &str, _content: &str) -> Result<(), DbError> {
        Err(self.error())
    }

    fn create_run(&self, _run: &AgentRun) -> Result<(), DbError> {
        Err(self.error())
    }
    fn update_run_status(
        &self,
        _run_uuid: &str,
        _status: &str,
        _final_text: Option<&str>,
        _error_summary: Option<&str>,
        _assistant_message_uuid: Option<&str>,
        _ended_at: Option<i64>,
    ) -> Result<(), DbError> {
        Err(self.error())
    }
    fn get_run(&self, _user_id: i64, _run_uuid: &str) -> Result<Option<AgentRun>, DbError> {
        Err(self.error())
    }
    fn runs_for_chat(&self, _user_id: i64, _chat_uuid: &str) -> Result<Vec<AgentRun>, DbError> {
        Err(self.error())
    }

    fn insert_tool_call(&self, _call: &ToolCall) -> Result<(), DbError> {
        Err(self.error())
    }
    fn update_tool_call_status(
        &self,
        _call_uuid: &str,
        _status: &str,
        _output: Option<&str>,
        _error: Option<&str>,
        _started_at: Option<i64>,
        _ended_at: Option<i64>,
    ) -> Result<(), DbError> {
        Err(self.error())
    }
    fn tool_calls_for_run(&self, _run_uuid: &str) -> Result<Vec<ToolCall>, DbError> {
        Err(self.error())
    }

    fn insert_event(
        &self,
        _run_uuid: &str,
        _seq: i64,
        _event_type: &str,
        _payload_json: &str,
        _created_at: i64,
    ) -> Result<(), DbError> {
        Err(self.error())
    }
    fn events_for_run_since(
        &self,
        _run_uuid: &str,
        _since: i64,
    ) -> Result<Vec<AgentEvent>, DbError> {
        Err(self.error())
    }
    fn max_event_seq(&self, _run_uuid: &str) -> Result<i64, DbError> {
        Err(self.error())
    }

    fn get_user_settings(&self, _user_id: i64) -> Result<UserSettings, DbError> {
        Err(self.error())
    }
    fn upsert_user_settings(&self, _user_id: i64, _settings: &UserSettings) -> Result<(), DbError> {
        Err(self.error())
    }
    fn update_user_name(&self, _user_id: i64, _name: &str) -> Result<(), DbError> {
        Err(self.error())
    }
    fn get_vultr_account(&self, _user_id: i64) -> Result<Option<VultrAccount>, DbError> {
        Err(self.error())
    }
    fn upsert_vultr_account(&self, _user_id: i64, _account: &VultrAccount) -> Result<(), DbError> {
        Err(self.error())
    }
    fn delete_vultr_account(&self, _user_id: i64) -> Result<(), DbError> {
        Err(self.error())
    }
    fn get_instance_access_profile(
        &self,
        _user_id: i64,
        _instance_id: &str,
    ) -> Result<Option<InstanceAccessProfile>, DbError> {
        Err(self.error())
    }
    fn list_instance_access_profiles(
        &self,
        _user_id: i64,
    ) -> Result<Vec<InstanceAccessProfile>, DbError> {
        Err(self.error())
    }
    fn upsert_instance_access_profile(
        &self,
        _user_id: i64,
        _profile: &InstanceAccessProfile,
    ) -> Result<(), DbError> {
        Err(self.error())
    }
    fn delete_instance_access_profile(
        &self,
        _user_id: i64,
        _instance_id: &str,
    ) -> Result<(), DbError> {
        Err(self.error())
    }
    fn delete_all_instance_access_profiles(&self, _user_id: i64) -> Result<(), DbError> {
        Err(self.error())
    }
    fn delete_all_chats_for_user(&self, _user_id: i64) -> Result<usize, DbError> {
        Err(self.error())
    }
    fn delete_all_workspaces_for_user(&self, _user_id: i64) -> Result<usize, DbError> {
        Err(self.error())
    }
}
