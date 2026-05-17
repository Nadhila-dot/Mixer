import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useNavigate } from "react-router-dom";
import {
  Paperclip,
  SendHorizontal,
  Sidebar as SidebarIcon,
} from "lucide-react";
import {
  FONT,
  KEYFRAMES,
  outerGlass,
  innerGlass,
  pillGlass,
  smallPillGlass,
  IconBtn,
} from "../lib/glass";
import { getSsrEnvelope } from "../lib/ssr";
import AppBackground from "../components/AppBackground";
import Sidebar from "../components/Sidebar";
import ModelSelector from "../components/ModelSelector";
import type {
  AiModel,
  HomeData,
  LimitsData,
  ModelsPayload,
  UsageData,
} from "../types";

export default function Home() {
  const navigate = useNavigate();
  const ssr = getSsrEnvelope<HomeData>();
  const [appData, setAppData] = useState<HomeData | null>(
    ssr?.page === "home" ? ssr.data : null,
  );
  const [text, setText] = useState("");
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [models, setModels] = useState<AiModel[]>([]);
  const [selectedModel, setSelectedModel] = useState(
    () => localStorage.getItem("mixer:model") ?? "",
  );
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    fetch("/api/models", { credentials: "same-origin" })
      .then(async (res) => {
        const body = await res.json().catch(() => ({}));
        if (!res.ok || !isModelsPayload(body)) throw new Error(body.error || "Could not load models");
        return body;
      })
      .then((body) => {
        setModels(body.models);
        setSelectedModel((current) => {
          const saved = current || localStorage.getItem("mixer:model") || "";
          const exists = body.models.some((model) => model.id === saved);
          const next = exists ? saved : body.default_model || body.models[0]?.id || "";
          if (next) localStorage.setItem("mixer:model", next);
          return next;
        });
      })
      .catch((err: Error) => setError(err.message));
  }, []);

  const remainingMessages = useMemo(() => {
    if (!appData) return 0;
    return Math.max(0, appData.limits.messages - appData.user.usage.messages);
  }, [appData]);

  const tokenPercent = useMemo(() => {
    if (!appData || appData.limits.tokens === 0) return 0;
    return Math.min(100, Math.round((appData.user.usage.tokens / appData.limits.tokens) * 100));
  }, [appData]);

  function sendMessage() {
    const message = text.trim();
    if (!message) return;
    setText("");
    navigate(`/chat/${crypto.randomUUID()}`, {
      state: {
        startAssistant: true,
        message,
        model: selectedModel || undefined,
      },
    });
  }

  async function deleteChat(id: string) {
    const res = await fetch(`/api/chats/${id}`, {
      method: "DELETE",
      credentials: "same-origin",
    });

    if (!res.ok) {
      const body = await res.json().catch(() => ({}));
      setError(body.error || "Could not delete chat");
      return;
    }

    setAppData((current) =>
      current
        ? {
            ...current,
            recent_chats: current.recent_chats.filter((chat) => chat.id !== id),
          }
        : current,
    );

    if (window.location.pathname === `/chat/${id}`) {
      window.history.replaceState({}, "", "/");
    }
  }

  const handleKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    }
  };

  if (!appData) {
    return null;
  }

  return (
    <div
      style={{
        position: "relative",
        width: "100vw",
        height: "100vh",
        overflow: "hidden",
        fontFamily: FONT,
      }}
    >
      <style>{KEYFRAMES}</style>

      <AppBackground />

      <div
        aria-hidden
        style={{
          position: "absolute",
          inset: 0,
          zIndex: 1,
          background:
            "radial-gradient(ellipse 100% 100% at 50% 50%, transparent 42%, rgba(0,0,0,0.30) 100%)",
          pointerEvents: "none",
        }}
      />

      <div
        style={{
          position: "relative",
          zIndex: 2,
          width: "100%",
          height: "100%",
          display: "flex",
          flexDirection: "column",
        }}
      >
        <header
          style={{
            padding: "14px 18px",
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
          }}
        >
          <button
            type="button"
            onClick={() => setSidebarOpen(true)}
            style={{
              ...pillGlass,
              display: "inline-flex",
              alignItems: "center",
              gap: 8,
              padding: "8px 14px",
              borderRadius: 999,
              fontSize: 13,
              color: "rgba(255,255,255,0.82)",
              cursor: "pointer",
            }}
          >
            <SidebarIcon size={14} strokeWidth={1.8} />
            Chats
          </button>

          <a
            href="/usr/logout"
            style={{
              ...smallPillGlass,
              padding: "7px 12px",
              borderRadius: 999,
              color: "rgba(255,255,255,0.78)",
              fontSize: 12,
              textDecoration: "none",
            }}
          >
            {appData.user.name} - {appData.user.tier ?? "free"}
          </a>
        </header>

        <main
          style={{
            flex: 1,
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            justifyContent: "center",
            padding: "0 20px 60px",
          }}
        >
          <svg
            viewBox="0 0 1000 200"
            width="min(820px, 92vw)"
            preserveAspectRatio="xMidYMid meet"
            aria-label="Mixer chat"
            style={{
              display: "block",
              margin: "0 auto -1.1rem",
              position: "relative",
              zIndex: 1,
              pointerEvents: "none",
              filter: "drop-shadow(0 1px 30px rgba(0,0,0,0.30))",
            }}
          >
            <text
              x="500"
              y="160"
              textAnchor="middle"
              textLength="950"
              lengthAdjust="spacingAndGlyphs"
              fontFamily={FONT}
              fontWeight={400}
              fontSize="190"
              letterSpacing="-4"
              fill="#FFFAEE"
            >
              Mixer chat
            </text>
          </svg>

          <section
            style={{
              ...outerGlass,
              width: "min(820px, 92vw)",
            }}
          >
            <header
              style={{
                display: "flex",
                justifyContent: "space-between",
                alignItems: "center",
                gap: 14,
                padding: "4px 8px 8px",
                flexWrap: "wrap",
              }}
            >
              <UsageLine usage={appData.user.usage} limits={appData.limits} />
              <div
                style={{
                  display: "inline-flex",
                  alignItems: "center",
                  gap: 8,
                }}
              >
                <ModelSelector
                  models={models}
                  selected={selectedModel}
                  onChange={(model) => {
                    setSelectedModel(model);
                    localStorage.setItem("mixer:model", model);
                  }}
                />
                <span style={{ fontSize: 12, color: "rgba(255,255,255,0.48)" }}>
                  Resets {formatReset(appData.limits.resets_at)}
                </span>
              </div>
            </header>

            <div
              style={{
                ...innerGlass,
              }}
            >
              <textarea
                ref={textareaRef}
                value={text}
                onChange={(e) => setText(e.target.value)}
                onKeyDown={handleKey}
                placeholder="Ask anything, or start a new thread..."
                rows={2}
                disabled={remainingMessages === 0}
                style={{
                  display: "block",
                  width: "100%",
                  background: "transparent",
                  border: "none",
                  outline: "none",
                  resize: "none",
                  padding: "12px 14px 6px",
                  fontFamily: FONT,
                  fontSize: 14.5,
                  lineHeight: 1.6,
                  color: "rgba(255,255,255,0.88)",
                  caretColor: "rgba(255,255,255,0.7)",
                }}
              />

              <footer
                style={{
                  display: "flex",
                  alignItems: "center",
                  padding: "4px 8px 8px",
                  gap: 6,
                }}
              >
                <IconBtn title="Attach file">
                  <Paperclip size={15} strokeWidth={1.7} />
                </IconBtn>
                <span style={{ flex: 1 }} />
                <span style={{ fontSize: 11.5, color: "rgba(255,255,255,0.36)" }}>
                  {tokenPercent}% tokens used
                </span>
                <IconBtn title="Send" onClick={sendMessage}>
                  <SendHorizontal size={15} strokeWidth={1.8} />
                </IconBtn>
              </footer>
            </div>

            {error && (
              <p style={{ margin: "8px 8px 0", color: "rgba(255,150,150,0.95)", fontSize: 12.5 }}>
                {error}
              </p>
            )}
          </section>
        </main>
      </div>

      <Sidebar
        open={sidebarOpen}
        onClose={() => setSidebarOpen(false)}
        user={appData.user}
        chats={appData.recent_chats}
        onDeleteChat={(id) => void deleteChat(id)}
      />
    </div>
  );
}

function UsageLine({ usage, limits }: { usage: UsageData; limits: LimitsData }) {
  const remaining = Math.max(0, limits.messages - usage.messages);
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 8,
      }}
    >
      <span style={{ fontSize: 12.5, color: "rgba(255,255,255,0.62)" }}>
        {remaining} of {limits.messages} messages left today
      </span>
      <span
        style={{
          ...smallPillGlass,
          padding: "2px 10px",
          borderRadius: 99,
          fontSize: 11.5,
          color: "rgba(255,255,255,0.82)",
        }}
      >
        {usage.tokens.toLocaleString()} / {limits.tokens.toLocaleString()} tokens
      </span>
    </div>
  );
}

function formatReset(value: string) {
  return new Intl.DateTimeFormat(undefined, {
    hour: "numeric",
    minute: "2-digit",
    weekday: "short",
  }).format(new Date(value));
}

function isModelsPayload(value: unknown): value is ModelsPayload {
  if (!value || typeof value !== "object") return false;
  const payload = value as Partial<ModelsPayload>;
  return (
    typeof payload.provider === "string" &&
    typeof payload.default_model === "string" &&
    Array.isArray(payload.models) &&
    payload.models.every((model) => {
      const item = model as Partial<AiModel>;
      return (
        typeof item.id === "string" &&
        typeof item.provider === "string" &&
        typeof item.label === "string"
      );
    })
  );
}
