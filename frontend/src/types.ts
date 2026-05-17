// ── SSR envelope ─────────────────────────────────────────────────────────────

export interface SsrEnvelope<T = Record<string, unknown>> {
  /** Matches the route name: "home" | "auth-screen" | "not-found" */
  page: string;
  /** Unix milliseconds — when the server rendered this response */
  ts: number;
  /** Page-specific payload */
  data: T;
}

// ── Page data shapes ─────────────────────────────────────────────────────────

export interface HomeData {
  user: {
    tier?: "free" | "pro" | "enterprise";
    name: string;
    email: string;
    usage: UsageData;
  };
  limits: LimitsData;
  recent_chats: ChatSummary[];
}

export interface AuthScreenData {
  next: string;
  loginEndpoint: string;
  registerEndpoint: string;
  sessionEndpoint: string;
}

export interface UsageData {
  messages: number;
  tokens: number;
  last_updated: string;
}

export interface LimitsData {
  messages: number;
  tokens: number;
  resets_at: string;
}

export interface ChatSummary {
  id: string;
  title: string;
  preview: string;
  updated_at: string;
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  created_at: string;
  model?: string;
}

export interface AiModel {
  id: string;
  provider: string;
  label: string;
}

export interface ModelsPayload {
  provider: string;
  default_model: string;
  models: AiModel[];
}
