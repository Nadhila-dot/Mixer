use super::{
    AgentEvent, AgentRun, AuthStore, ChatMessage, ChatSummary, DbError, ToolCall, Usage, User,
    UserWithPassword,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;

pub struct SqliteAuthStore {
    url: String,
}

impl SqliteAuthStore {
    pub fn new(url: String) -> Self {
        Self { url }
    }

    fn connect(&self) -> Result<Connection, DbError> {
        let conn = Connection::open(self.path()).map_err(|e| DbError::Query(e.to_string()))?;
        conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;
            PRAGMA busy_timeout = 5000;
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            ",
        )
        .map_err(|e| DbError::Query(e.to_string()))?;
        migrate(&conn)?;
        Ok(conn)
    }

    fn path(&self) -> PathBuf {
        self.url
            .strip_prefix("sqlite://")
            .or_else(|| self.url.strip_prefix("file://"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(&self.url))
    }
}

impl AuthStore for SqliteAuthStore {
    fn find_user_by_email(&self, email: &str) -> Result<Option<UserWithPassword>, DbError> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT id, name, email, password_hash, tier FROM users WHERE email = ?1",
            params![email],
            |row| {
                Ok(UserWithPassword {
                    user: User {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        email: row.get(2)?,
                        tier: row.get(4)?,
                    },
                    password_hash: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|e| DbError::Query(e.to_string()))
    }

    fn create_user(&self, name: &str, email: &str, password_hash: &str) -> Result<User, DbError> {
        let conn = self.connect()?;
        let inserted = conn.execute(
            "INSERT INTO users (name, email, password_hash, created_at) VALUES (?1, ?2, ?3, strftime('%s', 'now'))",
            params![name, email, password_hash],
        );

        match inserted {
            Ok(_) => Ok(User {
                id: conn.last_insert_rowid(),
                name: name.to_string(),
                email: email.to_string(),
                tier: "free".into(),
            }),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
            {
                Err(DbError::DuplicateEmail)
            }
            Err(e) => Err(DbError::Query(e.to_string())),
        }
    }

    fn create_session(&self, token: &str, user_id: i64, expires_at: i64) -> Result<(), DbError> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO sessions (token, user_id, expires_at, created_at) VALUES (?1, ?2, ?3, strftime('%s', 'now'))",
            params![token, user_id, expires_at],
        )
        .map(|_| ())
        .map_err(|e| DbError::Query(e.to_string()))
    }

    fn user_by_session(&self, token: &str, now: i64) -> Result<Option<User>, DbError> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT u.id, u.name, u.email, u.tier
             FROM sessions s
             JOIN users u ON u.id = s.user_id
             WHERE s.token = ?1 AND s.expires_at > ?2",
            params![token, now],
            |row| {
                Ok(User {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    email: row.get(2)?,
                    tier: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|e| DbError::Query(e.to_string()))
    }

    fn delete_session(&self, token: &str) -> Result<(), DbError> {
        let conn = self.connect()?;
        conn.execute("DELETE FROM sessions WHERE token = ?1", params![token])
            .map(|_| ())
            .map_err(|e| DbError::Query(e.to_string()))
    }

    fn usage_for_day(&self, user_id: i64, day: &str) -> Result<Usage, DbError> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT messages, tokens, last_updated FROM usage_days WHERE user_id = ?1 AND day = ?2",
            params![user_id, day],
            |row| {
                Ok(Usage {
                    messages: row.get(0)?,
                    tokens: row.get(1)?,
                    last_updated: row.get(2)?,
                })
            },
        )
        .optional()
        .map(|usage| {
            usage.unwrap_or(Usage {
                messages: 0,
                tokens: 0,
                last_updated: 0,
            })
        })
        .map_err(|e| DbError::Query(e.to_string()))
    }

    fn increment_usage(
        &self,
        user_id: i64,
        day: &str,
        messages: i64,
        tokens: i64,
        updated_at: i64,
    ) -> Result<Usage, DbError> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO usage_days (user_id, day, messages, tokens, last_updated)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(user_id, day) DO UPDATE SET
                messages = messages + excluded.messages,
                tokens = tokens + excluded.tokens,
                last_updated = excluded.last_updated",
            params![user_id, day, messages, tokens, updated_at],
        )
        .map_err(|e| DbError::Query(e.to_string()))?;

        self.usage_for_day(user_id, day)
    }

    fn recent_chats(&self, user_id: i64, limit: usize) -> Result<Vec<ChatSummary>, DbError> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT uuid, title, preview, updated_at
                 FROM chats
                 WHERE user_id = ?1
                 ORDER BY updated_at DESC
                 LIMIT ?2",
            )
            .map_err(|e| DbError::Query(e.to_string()))?;

        let rows = stmt
            .query_map(params![user_id, limit as i64], |row| {
                Ok(ChatSummary {
                    uuid: row.get(0)?,
                    title: row.get(1)?,
                    preview: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            })
            .map_err(|e| DbError::Query(e.to_string()))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| DbError::Query(e.to_string()))
    }

    fn chat_by_uuid(&self, user_id: i64, uuid: &str) -> Result<Option<ChatSummary>, DbError> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT uuid, title, preview, updated_at
             FROM chats
             WHERE user_id = ?1 AND uuid = ?2",
            params![user_id, uuid],
            |row| {
                Ok(ChatSummary {
                    uuid: row.get(0)?,
                    title: row.get(1)?,
                    preview: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|e| DbError::Query(e.to_string()))
    }

    fn chat_messages(&self, user_id: i64, uuid: &str) -> Result<Vec<ChatMessage>, DbError> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT m.uuid, m.role, m.content, m.created_at, m.model
                 FROM chat_messages m
                 JOIN chats c ON c.id = m.chat_id
                 WHERE c.user_id = ?1 AND c.uuid = ?2
                 ORDER BY m.created_at ASC, m.id ASC",
            )
            .map_err(|e| DbError::Query(e.to_string()))?;

        let rows = stmt
            .query_map(params![user_id, uuid], |row| {
                Ok(ChatMessage {
                    uuid: row.get(0)?,
                    role: row.get(1)?,
                    content: row.get(2)?,
                    created_at: row.get(3)?,
                    model: row.get(4)?,
                })
            })
            .map_err(|e| DbError::Query(e.to_string()))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| DbError::Query(e.to_string()))
    }

    fn create_chat(
        &self,
        user_id: i64,
        uuid: &str,
        title: &str,
        preview: &str,
        created_at: i64,
    ) -> Result<ChatSummary, DbError> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO chats (uuid, user_id, title, preview, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![uuid, user_id, title, preview, created_at],
        )
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(ChatSummary {
            uuid: uuid.to_string(),
            title: title.to_string(),
            preview: preview.to_string(),
            updated_at: created_at,
        })
    }

    fn delete_chat(&self, user_id: i64, uuid: &str) -> Result<bool, DbError> {
        let conn = self.connect()?;
        conn.execute(
            "DELETE FROM chats WHERE user_id = ?1 AND uuid = ?2",
            params![user_id, uuid],
        )
        .map(|count| count > 0)
        .map_err(|e| DbError::Query(e.to_string()))
    }

    fn append_message(
        &self,
        user_id: i64,
        chat_uuid: &str,
        role: &str,
        content: &str,
        created_at: i64,
        model: Option<&str>,
    ) -> Result<ChatMessage, DbError> {
        let conn = self.connect()?;
        let chat_id = conn
            .query_row(
                "SELECT id FROM chats WHERE user_id = ?1 AND uuid = ?2",
                params![user_id, chat_uuid],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|e| DbError::Query(e.to_string()))?
            .ok_or_else(|| DbError::Query("chat not found".into()))?;

        let message_uuid = uuid_v4_sql(&conn)?;
        conn.execute(
            "INSERT INTO chat_messages (uuid, chat_id, role, content, created_at, model)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![message_uuid, chat_id, role, content, created_at, model],
        )
        .map_err(|e| DbError::Query(e.to_string()))?;

        conn.execute(
            "UPDATE chats
             SET preview = CASE WHEN ?1 = 'user' THEN ?2 ELSE preview END,
                 updated_at = ?3
             WHERE id = ?4",
            params![role, content, created_at, chat_id],
        )
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(ChatMessage {
            uuid: message_uuid,
            role: role.to_string(),
            content: content.to_string(),
            created_at,
            model: model.map(String::from),
        })
    }

    fn create_workspace(
        &self,
        user_id: i64,
        uuid: &str,
        name: &str,
        created_at: i64,
    ) -> Result<super::Workspace, super::DbError> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO workspaces (uuid, user_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![uuid, user_id, name, created_at],
        )
        .map_err(|e| super::DbError::Query(e.to_string()))?;
        Ok(super::Workspace {
            uuid: uuid.to_string(),
            name: name.to_string(),
            created_at,
        })
    }

    fn get_workspace(
        &self,
        user_id: i64,
        uuid: &str,
    ) -> Result<Option<super::Workspace>, super::DbError> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT uuid, name, created_at FROM workspaces WHERE user_id = ?1 AND uuid = ?2",
            params![user_id, uuid],
            |row| {
                Ok(super::Workspace {
                    uuid: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(|e| super::DbError::Query(e.to_string()))
    }

    fn get_workspace_any(&self, uuid: &str) -> Result<Option<super::Workspace>, super::DbError> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT uuid, name, created_at FROM workspaces WHERE uuid = ?1",
            params![uuid],
            |row| {
                Ok(super::Workspace {
                    uuid: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(|e| super::DbError::Query(e.to_string()))
    }

    fn list_workspaces(&self, user_id: i64) -> Result<Vec<super::Workspace>, super::DbError> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare("SELECT uuid, name, created_at FROM workspaces WHERE user_id = ?1 ORDER BY created_at DESC")
            .map_err(|e| super::DbError::Query(e.to_string()))?;
        let rows = stmt
            .query_map(params![user_id], |row| {
                Ok(super::Workspace {
                    uuid: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                })
            })
            .map_err(|e| super::DbError::Query(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| super::DbError::Query(e.to_string()))
    }

    fn update_message_content(&self, msg_uuid: &str, content: &str) -> Result<(), super::DbError> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE chat_messages SET content = ?1 WHERE uuid = ?2",
            params![content, msg_uuid],
        )
        .map(|_| ())
        .map_err(|e| super::DbError::Query(e.to_string()))
    }

    fn create_run(&self, run: &AgentRun) -> Result<(), DbError> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO agent_runs (uuid, chat_uuid, user_id, user_message_uuid, assistant_message_uuid, model, status, final_text, error_summary, started_at, ended_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                run.uuid, run.chat_uuid, run.user_id, run.user_message_uuid,
                run.assistant_message_uuid, run.model, run.status,
                run.final_text, run.error_summary, run.started_at, run.ended_at,
            ],
        )
        .map(|_| ())
        .map_err(|e| DbError::Query(e.to_string()))
    }

    fn update_run_status(
        &self,
        run_uuid: &str,
        status: &str,
        final_text: Option<&str>,
        error_summary: Option<&str>,
        assistant_message_uuid: Option<&str>,
        ended_at: Option<i64>,
    ) -> Result<(), DbError> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE agent_runs
             SET status = ?1,
                 final_text = COALESCE(?2, final_text),
                 error_summary = COALESCE(?3, error_summary),
                 assistant_message_uuid = COALESCE(?4, assistant_message_uuid),
                 ended_at = COALESCE(?5, ended_at)
             WHERE uuid = ?6",
            params![
                status,
                final_text,
                error_summary,
                assistant_message_uuid,
                ended_at,
                run_uuid
            ],
        )
        .map(|_| ())
        .map_err(|e| DbError::Query(e.to_string()))
    }

    fn get_run(&self, user_id: i64, run_uuid: &str) -> Result<Option<AgentRun>, DbError> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT uuid, chat_uuid, user_id, user_message_uuid, assistant_message_uuid, model, status, final_text, error_summary, started_at, ended_at
             FROM agent_runs WHERE uuid = ?1 AND user_id = ?2",
            params![run_uuid, user_id],
            |row| Ok(AgentRun {
                uuid: row.get(0)?,
                chat_uuid: row.get(1)?,
                user_id: row.get(2)?,
                user_message_uuid: row.get(3)?,
                assistant_message_uuid: row.get(4)?,
                model: row.get(5)?,
                status: row.get(6)?,
                final_text: row.get(7)?,
                error_summary: row.get(8)?,
                started_at: row.get(9)?,
                ended_at: row.get(10)?,
            }),
        )
        .optional()
        .map_err(|e| DbError::Query(e.to_string()))
    }

    fn runs_for_chat(&self, user_id: i64, chat_uuid: &str) -> Result<Vec<AgentRun>, DbError> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT uuid, chat_uuid, user_id, user_message_uuid, assistant_message_uuid, model, status, final_text, error_summary, started_at, ended_at
             FROM agent_runs WHERE chat_uuid = ?1 AND user_id = ?2 ORDER BY started_at ASC",
        ).map_err(|e| DbError::Query(e.to_string()))?;
        let rows = stmt
            .query_map(params![chat_uuid, user_id], |row| {
                Ok(AgentRun {
                    uuid: row.get(0)?,
                    chat_uuid: row.get(1)?,
                    user_id: row.get(2)?,
                    user_message_uuid: row.get(3)?,
                    assistant_message_uuid: row.get(4)?,
                    model: row.get(5)?,
                    status: row.get(6)?,
                    final_text: row.get(7)?,
                    error_summary: row.get(8)?,
                    started_at: row.get(9)?,
                    ended_at: row.get(10)?,
                })
            })
            .map_err(|e| DbError::Query(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| DbError::Query(e.to_string()))
    }

    fn insert_tool_call(&self, call: &ToolCall) -> Result<(), DbError> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO tool_calls (uuid, run_uuid, parent_call_uuid, seq, name, args_json, body, status, output, error, attempt, started_at, ended_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                call.uuid, call.run_uuid, call.parent_call_uuid, call.seq,
                call.name, call.args_json, call.body, call.status,
                call.output, call.error, call.attempt, call.started_at, call.ended_at,
            ],
        )
        .map(|_| ())
        .map_err(|e| DbError::Query(e.to_string()))
    }

    fn update_tool_call_status(
        &self,
        call_uuid: &str,
        status: &str,
        output: Option<&str>,
        error: Option<&str>,
        started_at: Option<i64>,
        ended_at: Option<i64>,
    ) -> Result<(), DbError> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE tool_calls SET
                status = ?1,
                output = COALESCE(?2, output),
                error = COALESCE(?3, error),
                started_at = COALESCE(?4, started_at),
                ended_at = COALESCE(?5, ended_at)
             WHERE uuid = ?6",
            params![status, output, error, started_at, ended_at, call_uuid],
        )
        .map(|_| ())
        .map_err(|e| DbError::Query(e.to_string()))
    }

    fn tool_calls_for_run(&self, run_uuid: &str) -> Result<Vec<ToolCall>, DbError> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT uuid, run_uuid, parent_call_uuid, seq, name, args_json, body, status, output, error, attempt, started_at, ended_at
             FROM tool_calls WHERE run_uuid = ?1 ORDER BY seq ASC, attempt ASC",
        ).map_err(|e| DbError::Query(e.to_string()))?;
        let rows = stmt
            .query_map(params![run_uuid], |row| {
                Ok(ToolCall {
                    uuid: row.get(0)?,
                    run_uuid: row.get(1)?,
                    parent_call_uuid: row.get(2)?,
                    seq: row.get(3)?,
                    name: row.get(4)?,
                    args_json: row.get(5)?,
                    body: row.get(6)?,
                    status: row.get(7)?,
                    output: row.get(8)?,
                    error: row.get(9)?,
                    attempt: row.get(10)?,
                    started_at: row.get(11)?,
                    ended_at: row.get(12)?,
                })
            })
            .map_err(|e| DbError::Query(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| DbError::Query(e.to_string()))
    }

    fn insert_event(
        &self,
        run_uuid: &str,
        seq: i64,
        event_type: &str,
        payload_json: &str,
        created_at: i64,
    ) -> Result<(), DbError> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO agent_events (run_uuid, seq, event_type, payload_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![run_uuid, seq, event_type, payload_json, created_at],
        )
        .map(|_| ())
        .map_err(|e| DbError::Query(e.to_string()))
    }

    fn events_for_run_since(&self, run_uuid: &str, since: i64) -> Result<Vec<AgentEvent>, DbError> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT seq, event_type, payload_json, created_at FROM agent_events WHERE run_uuid = ?1 AND seq > ?2 ORDER BY seq ASC",
        ).map_err(|e| DbError::Query(e.to_string()))?;
        let rows = stmt
            .query_map(params![run_uuid, since], |row| {
                Ok(AgentEvent {
                    seq: row.get(0)?,
                    event_type: row.get(1)?,
                    payload_json: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .map_err(|e| DbError::Query(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| DbError::Query(e.to_string()))
    }

    fn max_event_seq(&self, run_uuid: &str) -> Result<i64, DbError> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM agent_events WHERE run_uuid = ?1",
            params![run_uuid],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| DbError::Query(e.to_string()))
    }

    fn get_user_settings(&self, user_id: i64) -> Result<super::UserSettings, DbError> {
        let conn = self.connect()?;
        let result = conn
            .query_row(
                "SELECT base_style, characteristics_json, custom_instructions, updated_at \
                 FROM user_settings WHERE user_id = ?1",
                params![user_id],
                |row| {
                    Ok(super::UserSettings {
                        base_style: row.get(0)?,
                        characteristics_json: row.get(1)?,
                        custom_instructions: row.get(2)?,
                        updated_at: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(result.unwrap_or_default())
    }

    fn upsert_user_settings(
        &self,
        user_id: i64,
        settings: &super::UserSettings,
    ) -> Result<(), DbError> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO user_settings (user_id, base_style, characteristics_json, custom_instructions, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(user_id) DO UPDATE SET \
                base_style = excluded.base_style, \
                characteristics_json = excluded.characteristics_json, \
                custom_instructions = excluded.custom_instructions, \
                updated_at = excluded.updated_at",
            params![
                user_id,
                &settings.base_style,
                &settings.characteristics_json,
                &settings.custom_instructions,
                settings.updated_at,
            ],
        )
        .map(|_| ())
        .map_err(|e| DbError::Query(e.to_string()))
    }

    fn update_user_name(&self, user_id: i64, name: &str) -> Result<(), DbError> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE users SET name = ?1 WHERE id = ?2",
            params![name, user_id],
        )
        .map(|_| ())
        .map_err(|e| DbError::Query(e.to_string()))
    }

    fn delete_all_chats_for_user(&self, user_id: i64) -> Result<usize, DbError> {
        let conn = self.connect()?;
        // chat_messages, agent_runs, tool_calls, agent_events all cascade via FK.
        // (chats.user_id has ON DELETE CASCADE; agent_runs' chat_uuid references
        // chats.uuid by convention but isn't a hard FK — chats are deleted by user_id
        // here and the agent_runs rows are deleted separately below.)
        let chats_deleted = conn
            .execute("DELETE FROM chats WHERE user_id = ?1", params![user_id])
            .map_err(|e| DbError::Query(e.to_string()))?;
        // agent_runs is indexed by user_id directly — clean those too so streamed
        // run history doesn't survive a "delete all chats" action.
        conn.execute(
            "DELETE FROM agent_runs WHERE user_id = ?1",
            params![user_id],
        )
        .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(chats_deleted)
    }

    fn delete_all_workspaces_for_user(&self, user_id: i64) -> Result<usize, DbError> {
        let conn = self.connect()?;
        let deleted = conn
            .execute(
                "DELETE FROM workspaces WHERE user_id = ?1",
                params![user_id],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(deleted)
    }
}

fn migrate(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            email TEXT NOT NULL UNIQUE,
            tier TEXT NOT NULL DEFAULT 'free',
            password_hash TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_users_email ON users(email);

        CREATE TABLE IF NOT EXISTS sessions (
            token TEXT PRIMARY KEY,
            user_id INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_user_id ON sessions(user_id);
        CREATE INDEX IF NOT EXISTS idx_sessions_expires_at ON sessions(expires_at);

        CREATE TABLE IF NOT EXISTS usage_days (
            user_id INTEGER NOT NULL,
            day TEXT NOT NULL,
            messages INTEGER NOT NULL DEFAULT 0,
            tokens INTEGER NOT NULL DEFAULT 0,
            last_updated INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(user_id, day),
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS chats (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            uuid TEXT NOT NULL DEFAULT '',
            user_id INTEGER NOT NULL,
            title TEXT NOT NULL,
            preview TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_chats_user_updated ON chats(user_id, updated_at DESC);

        CREATE TABLE IF NOT EXISTS chat_messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            uuid TEXT NOT NULL UNIQUE,
            chat_id INTEGER NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(chat_id) REFERENCES chats(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_chat_messages_chat_created ON chat_messages(chat_id, created_at ASC);

        CREATE TABLE IF NOT EXISTS workspaces (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            uuid TEXT NOT NULL UNIQUE,
            user_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_workspaces_user ON workspaces(user_id, created_at DESC);

        CREATE TABLE IF NOT EXISTS agent_runs (
            uuid TEXT PRIMARY KEY,
            chat_uuid TEXT NOT NULL,
            user_id INTEGER NOT NULL,
            user_message_uuid TEXT NOT NULL,
            assistant_message_uuid TEXT,
            model TEXT NOT NULL,
            status TEXT NOT NULL,
            final_text TEXT,
            error_summary TEXT,
            started_at INTEGER NOT NULL,
            ended_at INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_runs_chat ON agent_runs(chat_uuid, started_at);
        CREATE INDEX IF NOT EXISTS idx_runs_user ON agent_runs(user_id);

        CREATE TABLE IF NOT EXISTS tool_calls (
            uuid TEXT PRIMARY KEY,
            run_uuid TEXT NOT NULL,
            parent_call_uuid TEXT,
            seq INTEGER NOT NULL,
            name TEXT NOT NULL,
            args_json TEXT NOT NULL,
            body TEXT NOT NULL,
            status TEXT NOT NULL,
            output TEXT,
            error TEXT,
            attempt INTEGER NOT NULL DEFAULT 1,
            started_at INTEGER,
            ended_at INTEGER,
            FOREIGN KEY(run_uuid) REFERENCES agent_runs(uuid) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_tool_calls_run ON tool_calls(run_uuid, seq);

        CREATE TABLE IF NOT EXISTS agent_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            run_uuid TEXT NOT NULL,
            seq INTEGER NOT NULL,
            event_type TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            UNIQUE(run_uuid, seq),
            FOREIGN KEY(run_uuid) REFERENCES agent_runs(uuid) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_agent_events_run ON agent_events(run_uuid, seq);

        CREATE TABLE IF NOT EXISTS user_settings (
            user_id INTEGER PRIMARY KEY,
            base_style TEXT NOT NULL DEFAULT 'default',
            characteristics_json TEXT NOT NULL DEFAULT '{}',
            custom_instructions TEXT NOT NULL DEFAULT '',
            updated_at INTEGER NOT NULL,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );

        DELETE FROM sessions WHERE expires_at <= strftime('%s', 'now');
        ",
    )
    .and_then(|_| ignore_duplicate_column(conn, "ALTER TABLE users ADD COLUMN tier TEXT NOT NULL DEFAULT 'free';"))
    .and_then(|_| ignore_duplicate_column(conn, "ALTER TABLE chat_messages ADD COLUMN model TEXT;"))
    .and_then(|_| ignore_duplicate_column(conn, "ALTER TABLE chats ADD COLUMN uuid TEXT NOT NULL DEFAULT '';"))
    .and_then(|_| {
        conn.execute_batch(
            "
            UPDATE chats
            SET uuid = lower(
                substr(hex(randomblob(16)), 1, 8) || '-' ||
                substr(hex(randomblob(16)), 1, 4) || '-' ||
                '4' || substr(hex(randomblob(16)), 1, 3) || '-' ||
                substr('89ab', abs(random()) % 4 + 1, 1) || substr(hex(randomblob(16)), 1, 3) || '-' ||
                substr(hex(randomblob(16)), 1, 12)
            )
            WHERE uuid = '';
            CREATE UNIQUE INDEX IF NOT EXISTS idx_chats_uuid ON chats(uuid);
            ",
        )
    })
    .map_err(|e| DbError::Query(e.to_string()))
}

fn uuid_v4_sql(conn: &Connection) -> Result<String, DbError> {
    conn.query_row(
        "
        SELECT lower(
            substr(hex(randomblob(16)), 1, 8) || '-' ||
            substr(hex(randomblob(16)), 1, 4) || '-' ||
            '4' || substr(hex(randomblob(16)), 1, 3) || '-' ||
            substr('89ab', abs(random()) % 4 + 1, 1) || substr(hex(randomblob(16)), 1, 3) || '-' ||
            substr(hex(randomblob(16)), 1, 12)
        )
        ",
        [],
        |row| row.get(0),
    )
    .map_err(|e| DbError::Query(e.to_string()))
}

fn ignore_duplicate_column(conn: &Connection, sql: &str) -> rusqlite::Result<()> {
    conn.execute_batch(sql).or_else(|e| match e {
        rusqlite::Error::SqliteFailure(_, Some(message))
            if message.contains("duplicate column name") =>
        {
            Ok(())
        }
        other => Err(other),
    })
}
