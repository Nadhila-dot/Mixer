import React, {
  type CSSProperties,
  type ClipboardEvent,
  type KeyboardEvent,
  type ReactNode,
  Suspense,
  lazy,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useNavigate, useParams } from "react-router-dom";
import {
  ChevronDown,
  Code2,
  FileText,
  X,
  Paperclip,
  SendHorizontal,
  Sidebar as SidebarIcon,
  Square,
} from "lucide-react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  FONT,
  IconBtn,
  KEYFRAMES,
  innerGlass,
  outerGlass,
  pillGlass,
  smallPillGlass,
} from "../lib/glass";
import { getSsrEnvelope } from "../lib/ssr";
import { safeUuid } from "../lib/uuid";
import AppBackground from "../components/AppBackground";
import Sidebar from "../components/Sidebar";
import ModelSelector from "../components/ModelSelector";
import WorkspacePanel, { type WorkspaceState, parseWorkspaceToolResult, applyWorkspaceUpdate } from "../components/WorkspacePanel";
import VultrPanel from "../components/VultrPanel";
import type {
  AiModel,
  ChatMessage,
  ChatSummary,
  HomeData,
  LimitsData,
  ModelsPayload,
  UsageData,
} from "../types";

interface ChatWsRequest {
  action: "create" | "send" | "assistant" | "subscribe_run";
  chat_id: string;
  message?: string;
  model?: string;
  attachments?: PendingAttachment[];
  run_id?: string;
  after_seq?: number;
}

interface PendingAttachment {
  id: string;
  name: string;
  mime_type: string;
  size: number;
  encoding: "text" | "data_url";
  content: string;
}

export interface AgentToolCall {
  uuid: string;
  parent_call_uuid?: string | null;
  seq: number;
  name: string;
  args: Record<string, unknown>;
  body?: string | null;
  status: "pending" | "running" | "succeeded" | "failed" | "approval_required";
  output?: string | null;
  resultXml?: string | null;
  error?: string | null;
  attempt: number;
}

type UiMessage = ChatMessage & {
  activeTools?: string[];
  activeThought?: {
    status: "thinking" | "done";
    durationMs?: number;
  };
  isStreaming?: boolean;
  runUuid?: string;
  toolCalls?: AgentToolCall[];
  runStatus?: "running" | "completed" | "failed" | "cancelled";
  runError?: string;
  thinkingTotalMs?: number;
};

type StreamBuffer = {
  target: string;
  visible: string;
  frame: number | null;
  finalMessage?: ChatMessage;
};

interface AgentEventEnvelope {
  seq: number;
  type: string;
  payload: Record<string, unknown>;
  created_at: number;
}

interface RunSnapshot {
  uuid: string;
  chat_uuid: string;
  user_message_uuid: string;
  assistant_message_uuid?: string | null;
  model: string;
  status: "running" | "completed" | "failed" | "cancelled";
  final_text?: string | null;
  error_summary?: string | null;
  started_at: number;
  ended_at?: number | null;
  tool_calls: AgentToolCall[];
  events: AgentEventEnvelope[];
}

const LazyHighlightedCodeBlock = lazy(() => import("../components/HighlightedCodeBlock"));
const LazyMermaidBlock = lazy(() => import("../components/MermaidBlock"));

function isMermaidLanguage(lang: string | undefined): boolean {
  if (!lang) return false;
  const normalized = lang.toLowerCase().trim();
  return normalized === "mermaid" || normalized === "mmd";
}

// Module-level — cleared on page refresh, persists across React Router navigations.
// Used to hand off a pending new-chat stream from the home UI to the newly-mounted
// chat route without location state (which survives hard refresh and causes duplicate creates).
let _pendingChat: { chatId: string; message: string; model: string; attachments: PendingAttachment[] } | null = null;

const WORKSPACE_WIDTH_KEY = "mixer:workspace-width";
const ATTACHMENT_START = "--- MIXER_ATTACHMENTS_JSON ---";
const ATTACHMENT_END = "--- END_MIXER_ATTACHMENTS_JSON ---";
const MAX_ATTACHMENTS = 8;
const MAX_ATTACHMENT_BYTES = 2_000_000;
const WS_DEFAULT_RATIO = 0.60; // 40% chat / 60% workspace
const WS_MIN_WIDTH = 380;
const WS_MAX_RATIO = 0.72;

function defaultWorkspaceWidth(): number {
  if (typeof window === "undefined") return 720;
  return Math.round(window.innerWidth * WS_DEFAULT_RATIO);
}

function readStoredWorkspaceWidth(): number {
  if (typeof window === "undefined") return defaultWorkspaceWidth();
  const raw = localStorage.getItem(WORKSPACE_WIDTH_KEY);
  const parsed = raw ? Number.parseInt(raw, 10) : NaN;
  return Number.isFinite(parsed) && parsed > 0 ? parsed : defaultWorkspaceWidth();
}

function clampWorkspaceWidth(w: number): number {
  if (typeof window === "undefined") return Math.max(WS_MIN_WIDTH, w);
  const max = Math.max(WS_MIN_WIDTH, Math.round(window.innerWidth * WS_MAX_RATIO));
  return Math.min(max, Math.max(WS_MIN_WIDTH, Math.round(w)));
}

export default function ChatPage() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const ssr = getSsrEnvelope<HomeData>();
  const [appData, setAppData] = useState<HomeData | null>(
    ssr?.page === "home" ? ssr.data : null,
  );
  const [text, setText] = useState("");
  const [attachments, setAttachments] = useState<PendingAttachment[]>([]);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [messages, setMessages] = useState<UiMessage[]>([]);
  const [isSending, setIsSending] = useState(false);
  const [isLoadingChat, setIsLoadingChat] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [workspaceState, setWorkspaceState] = useState<WorkspaceState | null>(null);
  const [workspacePanelOpen, setWorkspacePanelOpen] = useState(false);
  const [vultrPanelOpen, setVultrPanelOpen] = useState(false);
  const [workspaceWidth, setWorkspaceWidth] = useState<number>(() => clampWorkspaceWidth(readStoredWorkspaceWidth()));
  const [isDraggingDivider, setIsDraggingDivider] = useState(false);
  const [models, setModels] = useState<AiModel[]>([]);
  const [selectedModel, setSelectedModel] = useState(
    () => localStorage.getItem("mixer:model") ?? "",
  );

  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const streamBuffersRef = useRef<Record<string, StreamBuffer>>({});
  const stopStreamRef = useRef<(() => void) | null>(null);

  const chatTitle = useMemo<string>(() => {
    if (appData && id) {
      const match = appData.recent_chats.find((c) => c.id === id);
      if (match) return match.title;
    }
    return "Conversation";
  }, [appData, id]);

  const hasRunningTool = useMemo(
    () => messages.some((m) => (m.toolCalls ?? []).some((t) => t.status === "running" || t.status === "pending")),
    [messages],
  );

  // Agent is "busy" as long as any assistant message is still streaming or its
  // run is in-progress. isSending only covers the initial send; this also covers
  // resumed run subscriptions (page refresh -> reconnect) and ongoing tool runs.
  const isAgentBusy = useMemo(
    () =>
      isSending ||
      messages.some(
        (m) =>
          m.role === "assistant" &&
          (m.isStreaming === true || m.runStatus === "running"),
      ),
    [isSending, messages],
  );

  async function loadChatSnapshot(chatId: string) {
    const res = await fetch(`/api/chats/${chatId}`, { credentials: "same-origin" });
    const body = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(body.error || "Could not load chat");
    if (!isChatPayload(body)) throw new Error("Server returned an invalid chat payload");
    return body;
  }

  async function loadChatRuns(chatId: string) {
    const res = await fetch(`/api/chats/${chatId}/runs`, { credentials: "same-origin" });
    const body = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(body.error || "Could not load chat runs");
    if (!isRunListPayload(body)) throw new Error("Server returned an invalid run payload");
    return body.runs;
  }

  function applyChatContext(
    body: { chat: ChatSummary; messages: ChatMessage[] },
    runs: RunSnapshot[],
  ) {
    const hydrated = hydrateMessagesWithRuns(body.messages, runs);
    setMessages(hydrated);
    setAppData((current) =>
      current
        ? {
            ...current,
            recent_chats: [
              body.chat,
              ...current.recent_chats.filter((chat) => chat.id !== body.chat.id),
            ],
          }
        : current,
    );
  }

  useEffect(() => {
    if (appData) return;
    fetch("/api/app-state", { credentials: "same-origin" })
      .then(async (res) => {
        const body = await res.json().catch(() => ({}));
        if (!res.ok || !isHomeData(body)) throw new Error(body.error || "Could not load app state");
        return body;
      })
      .then(setAppData)
      .catch((err: Error) => setError(err.message));
  }, [appData]);

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

  useEffect(() => {
    if (!id) return;

    // If this chat was just created from the home UI, _pendingChat holds the message.
    // Claim it immediately (before any async work) so StrictMode double-invoke is harmless.
    const pending = _pendingChat;
    if (pending && pending.chatId === id) {
      _pendingChat = null;
      void streamCreateChat(id, pending.message, pending.model, pending.attachments);
      return;
    }

    setMessages([]);
    setError(null);
    setWorkspaceState(null);
    setWorkspacePanelOpen(false);
    setIsLoadingChat(true);
    stopStreamRef.current?.();
    stopStreamRef.current = null;

    Promise.all([loadChatSnapshot(id), loadChatRuns(id)])
      .then(([body, runs]) => {
        applyChatContext(body, runs);
        const activeRun = [...runs].reverse().find((run) => run.status === "running");
        if (activeRun) {
          const lastEvent = activeRun.events.length > 0 ? activeRun.events[activeRun.events.length - 1] : undefined;
          void subscribeToRun(activeRun.uuid, lastEvent?.seq ?? 0, activeRun.assistant_message_uuid ?? undefined);
        }
      })
      .catch((err: Error) => setError(err.message))
      .finally(() => setIsLoadingChat(false));
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  }, [messages]);

  useEffect(() => {
    return () => {
      for (const buffer of Object.values(streamBuffersRef.current)) {
        if (buffer.frame !== null) cancelAnimationFrame(buffer.frame);
      }
    };
  }, []);

  useEffect(() => {
    if (!id) {
      setWorkspaceState(null);
      setWorkspacePanelOpen(false);
      return;
    }

    let ws: WorkspaceState | null = null;
    for (const msg of messages) {
      if (msg.role !== "assistant") continue;
      for (const call of msg.toolCalls ?? []) {
        const recoveredWorkspaceId = extractWorkspaceIdFromToolCall(call);
        if (recoveredWorkspaceId) {
          ws = applyWorkspaceUpdate(
            ws,
            recoveredWorkspaceId,
            recoveredWorkspaceId,
            [],
            null,
            "WorkspaceStatus",
            "",
            null,
            null,
          );
        }
      }
      const sources = [
        ...parseToolParts(msg.content)
          .filter((part): part is Exclude<ToolPart, { kind: "text" }> => part.kind !== "text")
          .filter((part) => part.name === "tool_result" && part.attrs.tool === "workspace")
          .map((part) => ({ attrs: part.attrs, content: decodeXml(part.content) })),
        ...(msg.toolCalls ?? [])
          .flatMap((call) => [call.resultXml, call.output])
          .filter((output): output is string => typeof output === "string" && output.includes("<tool_result tool=\"workspace\""))
          .flatMap((output) =>
            parseToolParts(output)
              .filter((part): part is Exclude<ToolPart, { kind: "text" }> => part.kind !== "text")
              .filter((part) => part.name === "tool_result" && part.attrs.tool === "workspace")
              .map((part) => ({ attrs: part.attrs, content: decodeXml(part.content) })),
          ),
      ];

      for (const source of sources) {
        const parsed = parseWorkspaceToolResult(source.attrs, source.content);
        if (parsed) {
          ws = applyWorkspaceUpdate(
            ws,
            parsed.workspaceId,
            parsed.workspaceId,
            parsed.fileTree,
            parsed.commandInfo,
            parsed.action,
            source.content,
            parsed.previewPath,
            parsed.previewUrl,
          );
        }
      }
    }
    if (ws !== null) {
      setWorkspaceState(ws);
      if (id) {
        try {
          localStorage.setItem(workspaceRecoveryKey(id), JSON.stringify({ id: ws.id, name: ws.name }));
        } catch {}
      }
      return;
    }
    if (id) {
      const recovered = readRecoveredWorkspace(id);
      if (recovered) {
        setWorkspaceState({
          id: recovered.id,
          name: recovered.name,
          fileTree: [],
          commands: [],
          activeFile: null,
          activeFileContent: null,
          previewPath: null,
          previewUrl: null,
        });
        return;
      }
    }
    setWorkspaceState(null);
    setWorkspacePanelOpen(false);
  }, [messages, id]);

  useEffect(() => {
    if (workspaceState) setWorkspacePanelOpen(true);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspaceState?.id]);

  // Persist workspace width to localStorage (only when not actively dragging)
  useEffect(() => {
    if (isDraggingDivider) return;
    try {
      localStorage.setItem(WORKSPACE_WIDTH_KEY, String(workspaceWidth));
    } catch {}
  }, [workspaceWidth, isDraggingDivider]);

  // Re-clamp width on window resize so the panel can't get stuck wider than viewport
  useEffect(() => {
    function onResize() {
      setWorkspaceWidth((w) => clampWorkspaceWidth(w));
    }
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  function onDividerPointerDown(event: React.PointerEvent<HTMLDivElement>) {
    event.preventDefault();
    setIsDraggingDivider(true);
    const originalCursor = document.body.style.cursor;
    const originalUserSelect = document.body.style.userSelect;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";

    const handleMove = (e: PointerEvent) => {
      setWorkspaceWidth(clampWorkspaceWidth(window.innerWidth - e.clientX));
    };
    const handleUp = () => {
      setIsDraggingDivider(false);
      document.body.style.cursor = originalCursor;
      document.body.style.userSelect = originalUserSelect;
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
    };

    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
  }

  function appendAssistantDelta(messageId: string, text: string) {
    const buffer = streamBuffersRef.current[messageId] ?? {
      target: "",
      visible: "",
      frame: null,
    };
    buffer.target += text;
    debugFrontendStream("delta-buffer", {
      messageId,
      text,
      visible_chars: buffer.visible.length,
      target_chars: buffer.target.length,
      queued_chars: buffer.target.length - buffer.visible.length,
    });
    streamBuffersRef.current[messageId] = buffer;
    scheduleAssistantDrain(messageId);
  }

  function scheduleAssistantDrain(messageId: string) {
    const buffer = streamBuffersRef.current[messageId];
    if (!buffer) return;
    if (buffer.frame !== null) return;
    buffer.frame = requestAnimationFrame(() => drainAssistantBuffer(messageId));
  }

  function drainAssistantBuffer(messageId: string) {
    const buffer = streamBuffersRef.current[messageId];
    if (!buffer) return;

    buffer.frame = null;
    const pending = buffer.target.length - buffer.visible.length;
    if (pending > 0) {
      const take = Math.min(pending, Math.max(2, Math.min(10, Math.ceil(pending / 10))));
      buffer.visible += buffer.target.slice(buffer.visible.length, buffer.visible.length + take);
      debugFrontendStream("delta-flush", {
        messageId,
        visible_chars: buffer.visible.length,
        target_chars: buffer.target.length,
        remaining_chars: buffer.target.length - buffer.visible.length,
      });
      setMessages((prev) =>
        prev.map((message) =>
          message.id === messageId
            ? { ...message, content: buffer.visible, isStreaming: true }
            : message,
        ),
      );
      scheduleAssistantDrain(messageId);
      return;
    }

    if (buffer.finalMessage) {
      const finalMessage = buffer.finalMessage;
      delete streamBuffersRef.current[messageId];
      setMessages((prev) =>
        prev.map((item) => (item.id === messageId ? { ...finalMessage, isStreaming: false } : item)),
      );
    }
  }

  function flushAllBuffers() {
    for (const [messageId, buffer] of Object.entries(streamBuffersRef.current)) {
      if (buffer.frame !== null) {
        cancelAnimationFrame(buffer.frame);
        buffer.frame = null;
      }
      const content = buffer.target;
      delete streamBuffersRef.current[messageId];
      setMessages((prev) =>
        prev.map((msg) =>
          msg.id === messageId ? { ...msg, content, isStreaming: false } : msg,
        ),
      );
    }
  }

  async function cancelRun(runUuid: string) {
    const res = await fetch(`/api/runs/${encodeURIComponent(runUuid)}/cancel`, {
      method: "POST",
      credentials: "same-origin",
    });
    if (!res.ok) {
      const body = await res.json().catch(() => ({}));
      throw new Error(body.error || "Could not stop run");
    }
  }

  function markActiveRunsCancelled(runIds: Set<string>) {
    setMessages((prev) =>
      prev.map((message) => {
        if (message.role !== "assistant") return message;
        if (!message.runUuid || !runIds.has(message.runUuid)) return message;
        return {
          ...message,
          isStreaming: false,
          runStatus: "cancelled" as const,
          activeThought: undefined,
          runError: "Stopped by user",
          toolCalls: (message.toolCalls ?? []).map((call) =>
            call.status === "pending" || call.status === "running"
              ? { ...call, status: "failed" as const, error: "Stopped by user" }
              : call,
          ),
        };
      }),
    );
  }

  function stopStreaming() {
    const activeRunIds = new Set(
      messages
        .filter((message) => message.role === "assistant" && message.runStatus === "running" && message.runUuid)
        .map((message) => message.runUuid as string),
    );
    stopStreamRef.current?.();
    stopStreamRef.current = null;
    if (activeRunIds.size > 0) {
      markActiveRunsCancelled(activeRunIds);
      for (const runUuid of activeRunIds) {
        void cancelRun(runUuid).catch((err: Error) => {
          setError(err.message);
        });
      }
    }
    flushAllBuffers();
  }

  function finalizeAssistantMessage(messageId: string, message: ChatMessage) {
    const buffer = streamBuffersRef.current[messageId];
    if (!buffer) {
      setMessages((prev) =>
        prev.map((item) => (item.id === messageId ? { ...message, isStreaming: false } : item)),
      );
      return;
    }

    buffer.finalMessage = message;
    if (buffer.visible.length >= buffer.target.length) {
      drainAssistantBuffer(messageId);
    } else {
      scheduleAssistantDrain(messageId);
    }
  }

  function markAssistantTools(messageId: string, tools: string[]) {
    setMessages((prev) =>
      prev.map((message) =>
        message.id === messageId ? { ...message, activeTools: tools } : message,
      ),
    );
  }

  function applyAgentEvent(messageId: string, eventType: string, payload: Record<string, unknown>) {
    setMessages((prev) =>
      prev.map((m) => {
        if (m.id !== messageId) return m;
        const tools = [...(m.toolCalls ?? [])];
        const upsert = (incoming: Partial<AgentToolCall> & { uuid: string }) => {
          const i = tools.findIndex((t) => t.uuid === incoming.uuid);
          if (i >= 0) tools[i] = { ...tools[i], ...incoming };
          else
            tools.push({
              seq: tools.length + 1,
              name: "tool",
              args: {},
              status: "pending",
              attempt: 1,
              ...incoming,
            } as AgentToolCall);
        };

        switch (eventType) {
          case "text_delta": {
            const t = typeof payload.text === "string" ? payload.text : "";
            if (!t) return m;
            return { ...m, content: (m.content ?? "") + t, toolCalls: tools };
          }
          case "tool_pending":
            upsert({
              uuid: String(payload.uuid),
              seq: Number(payload.seq ?? 0),
              name: String(payload.name ?? "tool"),
              args: (payload.args as Record<string, unknown>) ?? {},
              status: "pending",
              attempt: Number(payload.attempt ?? 1),
              parent_call_uuid: (payload.parent_uuid as string | null | undefined) ?? null,
            });
            break;
          case "tool_running":
            upsert({ uuid: String(payload.uuid), status: "running" });
            break;
          case "tool_output":
            upsert({
              uuid: String(payload.uuid),
              output: `${tools.find((t) => t.uuid === String(payload.uuid))?.output ?? ""}${typeof payload.chunk === "string" ? payload.chunk : ""}`,
            });
            break;
          case "tool_succeeded":
            upsert({
              uuid: String(payload.uuid),
              status: "succeeded",
              output: typeof payload.output === "string" ? payload.output : null,
              resultXml: typeof payload.result_xml === "string" ? payload.result_xml : null,
            });
            break;
          case "tool_failed":
            upsert({
              uuid: String(payload.uuid),
              status: "failed",
              error: typeof payload.error === "string" ? payload.error : "tool failed",
            });
            break;
          case "tool_approval_required":
            upsert({
              uuid: String(payload.uuid),
              status: "approval_required",
              output: typeof payload.output === "string" ? payload.output : null,
              error: typeof payload.reason === "string" ? payload.reason : "approval required",
              resultXml: typeof payload.result_xml === "string" ? payload.result_xml : null,
            });
            break;
          case "thinking_started":
            return {
              ...m,
              toolCalls: tools,
              activeThought: { status: "thinking" as const },
            };
          case "thinking_done": {
            const dur = Number(payload.duration_ms ?? 0);
            return {
              ...m,
              toolCalls: tools,
              activeThought: {
                status: "done" as const,
                durationMs: dur,
              },
              thinkingTotalMs: (m.thinkingTotalMs ?? 0) + dur,
            };
          }
          case "run_completed":
            {
              const finalText = typeof payload.final_text === "string" ? payload.final_text : "";
              const visibleFinalText = finalText && !looksLikeControlPlaneLeak(finalText) ? finalText : m.content;
              return {
                ...m,
                toolCalls: tools,
                runStatus: "completed" as const,
                isStreaming: false,
                activeThought: undefined,
                content: visibleFinalText,
              };
            }
          case "run_failed":
            return {
              ...m,
              toolCalls: tools,
              runStatus: "failed" as const,
              isStreaming: false,
              activeThought: undefined,
              runError: typeof payload.error === "string" ? payload.error : "run failed",
            };
          case "run_cancelled":
            return {
              ...m,
              toolCalls: tools.map((call) =>
                call.status === "pending" || call.status === "running"
                  ? { ...call, status: "failed" as const, error: "Stopped by user" }
                  : call,
              ),
              runStatus: "cancelled" as const,
              isStreaming: false,
              activeThought: undefined,
              runError: typeof payload.reason === "string" ? payload.reason : "Stopped by user",
            };
        }
        return { ...m, toolCalls: tools };
      }),
    );
  }

  function ensureAssistantMessageForRun(
    runUuid: string,
    assistantMessageUuid?: string | null,
  ) {
    const targetId = assistantMessageUuid || `run-${runUuid}`;
    setMessages((prev) => {
      if (prev.some((message) => message.id === targetId || message.runUuid === runUuid)) {
        return prev.map((message) =>
          message.runUuid === runUuid
            ? { ...message, id: message.id, runUuid }
            : message,
        );
      }

      const lastUserIndex = [...prev]
        .map((message, index) => ({ message, index }))
        .reverse()
        .find(({ message }) => message.role === "user")?.index;

      const placeholder: UiMessage = {
        id: targetId,
        role: "assistant",
        content: "",
        created_at: new Date().toISOString(),
        isStreaming: true,
        runUuid,
        toolCalls: [],
        runStatus: "running",
      };

      if (lastUserIndex === undefined) {
        return [...prev, placeholder];
      }

      const next = [...prev];
      next.splice(lastUserIndex + 1, 0, placeholder);
      return next;
    });
    return targetId;
  }

  function setMessageRunUuid(messageId: string, runUuid: string) {
    setMessages((prev) =>
      prev.map((m) => (m.id === messageId ? { ...m, runUuid, runStatus: "running" } : m)),
    );
  }

  async function subscribeToRun(
    runUuid: string,
    afterSeq = 0,
    assistantMessageUuid?: string,
  ) {
    const messageId = ensureAssistantMessageForRun(runUuid, assistantMessageUuid);
    await readChatWebSocket(
      {
        action: "subscribe_run",
        chat_id: id || "",
        run_id: runUuid,
        after_seq: afterSeq,
      },
      (event, data) => {
        debugFrontendStream(event, data);
        if (event === "agent_event" && data && typeof data === "object") {
          const env = data as { type?: string; payload?: Record<string, unknown> };
          if (env.type && env.payload) {
            applyAgentEvent(messageId, env.type, env.payload);
          }
        }

        if (event === "done" && isStreamDonePayload(data) && data.assistant_message) {
          finalizeAssistantMessage(messageId, data.assistant_message);
        }

        if (event === "error" && isErrorPayload(data)) {
          setError(data.error);
        }
      },
      (stop) => {
        stopStreamRef.current = stop;
      },
    );
  }

  function replaceAssistantContent(messageId: string, text: string) {
    const buffer = streamBuffersRef.current[messageId];
    if (buffer) {
      if (buffer.frame !== null) cancelAnimationFrame(buffer.frame);
      buffer.target = text;
      buffer.visible = text;
      buffer.frame = null;
    }
    setMessages((prev) =>
      prev.map((m) =>
        m.id === messageId ? { ...m, content: text, isStreaming: true } : m,
      ),
    );
  }

  async function addAttachmentFiles(files: File[]) {
    if (files.length === 0) return;
    setError(null);
    try {
      const existing = attachments.length;
      const slots = Math.max(0, MAX_ATTACHMENTS - existing);
      if (slots === 0) {
        setError(`You can attach up to ${MAX_ATTACHMENTS} files per message.`);
        return;
      }
      const selected = files.slice(0, slots);
      const next = await Promise.all(selected.map(readAttachmentFile));
      setAttachments((current) => [...current, ...next].slice(0, MAX_ATTACHMENTS));
      if (files.length > slots) {
        setError(`Attached ${slots} file${slots === 1 ? "" : "s"}. You can attach up to ${MAX_ATTACHMENTS} files per message.`);
      }
      textareaRef.current?.focus();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not attach file");
    }
  }

  async function handleAttachmentInput(files: FileList | null) {
    if (!files || files.length === 0) return;
    try {
      await addAttachmentFiles(Array.from(files));
    } finally {
      if (fileInputRef.current) fileInputRef.current.value = "";
    }
  }

  function handlePaste(event: ClipboardEvent<HTMLTextAreaElement>) {
    const pastedFiles = filesFromClipboard(event.clipboardData);
    if (pastedFiles.length === 0) return;
    event.preventDefault();
    void addAttachmentFiles(pastedFiles);
  }

  function removeAttachment(id: string) {
    setAttachments((current) => current.filter((attachment) => attachment.id !== id));
  }

  async function sendMessage() {
    const trimmed = text.trim();
    const outgoingAttachments = attachments;
    if (!trimmed && outgoingAttachments.length === 0) return;

    // Home state: create a brand-new chat
    if (!id) {
      const chatId = safeUuid();
      _pendingChat = {
        chatId,
        message: trimmed,
        model: selectedModel || localStorage.getItem("mixer:model") || "",
        attachments: outgoingAttachments,
      };
      setText("");
      setAttachments([]);
      navigate(`/chat/${chatId}`);
      return;
    }

    stopStreaming();

    setIsSending(true);
    setError(null);
    setText("");
    setAttachments([]);
    const assistantId = `stream-${Date.now()}`;

    try {
      let hasAssistant = false;
      await readChatWebSocket(
        {
          action: "send",
          chat_id: id,
          message: trimmed,
          model: selectedModel || undefined,
          attachments: outgoingAttachments,
        },
        (event, data) => {
        debugFrontendStream(event, data);
        if (event === "meta") {
          if (!isStreamMetaPayload(data)) {
            setError("Server returned an invalid stream payload");
            return;
          }

          setMessages((prev) => [
            ...prev,
            data.user_message,
            {
              id: assistantId,
              role: "assistant",
              content: "",
              model: data.model,
              created_at: new Date().toISOString(),
              isStreaming: true,
            },
          ]);
          setAppData((current) =>
            current
              ? {
                  ...current,
                  user: { ...current.user, usage: data.usage },
                  limits: data.limits,
                  recent_chats: [
                    data.chat,
                    ...current.recent_chats.filter((item) => item.id !== data.chat.id),
                  ],
                }
              : current,
          );
        }

        if (event === "delta" && isDeltaPayload(data)) {
          hasAssistant = true;
          appendAssistantDelta(assistantId, data.text);
        }

        if (event === "tool_start" && isToolLifecyclePayload(data)) {
          hasAssistant = true;
          markAssistantTools(assistantId, data.tools);
        }

        if (event === "tool_done" && isToolLifecyclePayload(data)) {
          markAssistantTools(assistantId, []);
        }

        if (event === "replace" && isReplacePayload(data)) {
          hasAssistant = true;
          replaceAssistantContent(assistantId, data.text);
        }

        if (event === "run_started" && data && typeof data === "object" && "run_uuid" in data) {
          hasAssistant = true;
          setMessageRunUuid(assistantId, String((data as Record<string, unknown>).run_uuid));
        }

        if (event === "agent_event" && data && typeof data === "object") {
          hasAssistant = true;
          const env = data as { type?: string; payload?: Record<string, unknown> };
          if (env.type && env.payload) {
            applyAgentEvent(assistantId, env.type, env.payload);
          }
        }

        if (event === "done" && isStreamDonePayload(data) && data.assistant_message) {
          finalizeAssistantMessage(assistantId, data.assistant_message);
        }

        if (event === "error" && isErrorPayload(data)) {
          setError(data.error);
        }
        },
        (stop) => { stopStreamRef.current = stop; },
      );

      if (!hasAssistant) {
        setMessages((prev) => prev.filter((message) => message.id !== assistantId));
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "Message failed");
    } finally {
      stopStreamRef.current = null;
      setIsSending(false);
    }
  }

  async function streamCreateChat(
    chatId: string,
    message: string,
    model: string,
    outgoingAttachments: PendingAttachment[],
  ) {
    const optimisticUserId = `optimistic-${Date.now()}`;
    const assistantId = `stream-${Date.now()}`;

    setMessages([
      {
        id: optimisticUserId,
        role: "user",
        content: formatMessageWithAttachments(message, outgoingAttachments),
        created_at: new Date().toISOString(),
      },
      {
        id: assistantId,
        role: "assistant",
        content: "",
        model: model || undefined,
        created_at: new Date().toISOString(),
        isStreaming: true,
      },
    ]);
    setIsSending(true);
    setError(null);

    try {
      let hasAssistant = false;
      await readChatWebSocket(
        {
          action: "create",
          chat_id: chatId,
          message,
          model: model || undefined,
          attachments: outgoingAttachments,
        },
        (event, data) => {
          debugFrontendStream(event, data);

          if (event === "meta" && isStreamMetaPayload(data)) {
            setMessages((prev) =>
              prev.map((m) => (m.id === optimisticUserId ? data.user_message : m)),
            );
            setAppData((current) =>
              current
                ? {
                    ...current,
                    user: { ...current.user, usage: data.usage },
                    limits: data.limits,
                    recent_chats: [
                      data.chat,
                      ...current.recent_chats.filter((item) => item.id !== data.chat.id),
                    ],
                  }
                : current,
            );
          }

          if (event === "delta" && isDeltaPayload(data)) {
            hasAssistant = true;
            appendAssistantDelta(assistantId, data.text);
          }

          if (event === "tool_start" && isToolLifecyclePayload(data)) {
            hasAssistant = true;
            markAssistantTools(assistantId, data.tools);
          }

          if (event === "tool_done" && isToolLifecyclePayload(data)) {
            markAssistantTools(assistantId, []);
          }

          if (event === "replace" && isReplacePayload(data)) {
            hasAssistant = true;
            replaceAssistantContent(assistantId, data.text);
          }

          if (event === "run_started" && data && typeof data === "object" && "run_uuid" in data) {
            hasAssistant = true;
            setMessageRunUuid(assistantId, String((data as Record<string, unknown>).run_uuid));
          }

          if (event === "agent_event" && data && typeof data === "object") {
            hasAssistant = true;
            const env = data as { type?: string; payload?: Record<string, unknown> };
            if (env.type && env.payload) {
              applyAgentEvent(assistantId, env.type, env.payload);
            }
          }

          if (event === "done" && isStreamDonePayload(data) && data.assistant_message) {
            finalizeAssistantMessage(assistantId, data.assistant_message);
          }

          if (event === "error" && isErrorPayload(data)) {
            setError(data.error);
          }
        },
        (stop) => { stopStreamRef.current = stop; },
      );

      if (!hasAssistant) {
        setMessages((prev) => prev.filter((m) => m.id !== assistantId));
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to start chat");
    } finally {
      stopStreamRef.current = null;
      setIsSending(false);
    }
  }

  async function deleteChat(chatId: string) {
    const res = await fetch(`/api/chats/${chatId}`, {
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
            recent_chats: current.recent_chats.filter((chat) => chat.id !== chatId),
          }
        : current,
    );

    if (id === chatId) {
      navigate("/");
    }
  }

  function handleKey(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void sendMessage();
    }
  }

  if (!appData) return null;

  // Home UI — shown when no chat is active (route is "/")
  if (!id) {
    const remainingMessages = Math.max(0, appData.limits.messages - appData.user.usage.messages);
    const tokenPercent = appData.limits.tokens === 0
      ? 0
      : Math.min(100, Math.round((appData.user.usage.tokens / appData.limits.tokens) * 100));

    return (
      <div style={{ position: "relative", width: "100vw", height: "100vh", overflow: "hidden", fontFamily: FONT }}>
        <style>{KEYFRAMES}</style>
        <AppBackground />
        <div aria-hidden style={{ position: "absolute", inset: 0, zIndex: 1, background: "radial-gradient(ellipse 100% 100% at 50% 50%, transparent 42%, rgba(0,0,0,0.30) 100%)", pointerEvents: "none" }} />
        <div style={{ position: "relative", zIndex: 2, width: "100%", height: "100%", display: "flex", flexDirection: "column" }}>
          <header style={{ padding: "14px 18px", display: "flex", alignItems: "center", justifyContent: "space-between" }}>
            <div style={{ display: "inline-flex", alignItems: "center", gap: 8 }}>
              <button type="button" onClick={() => setSidebarOpen(true)} style={{ ...pillGlass, display: "inline-flex", alignItems: "center", gap: 8, padding: "8px 14px", borderRadius: 999, fontSize: 13, color: "rgba(255,255,255,0.82)", cursor: "pointer" }}>
                <SidebarIcon size={14} strokeWidth={1.8} />
                Chats
              </button>
              <button type="button" onClick={() => setVultrPanelOpen(true)} style={{ ...pillGlass, display: "inline-flex", alignItems: "center", gap: 8, padding: "8px 14px", borderRadius: 999, fontSize: 13, color: "rgba(255,255,255,0.82)", cursor: "pointer" }}>
                Vultr
              </button>
            </div>
            <a href="/usr/logout" style={{ ...smallPillGlass, padding: "7px 12px", borderRadius: 999, color: "rgba(255,255,255,0.78)", fontSize: 12, textDecoration: "none" }}>
              {appData.user.name} - {appData.user.tier ?? "free"}
            </a>
          </header>

          <main style={{ flex: 1, display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", padding: "0 20px 60px" }}>
            <svg viewBox="0 0 1000 200" width="min(820px, 92vw)" preserveAspectRatio="xMidYMid meet" aria-label="Mixer chat" style={{ display: "block", margin: "0 auto -1.1rem", position: "relative", zIndex: 1, pointerEvents: "none", filter: "drop-shadow(0 1px 30px rgba(0,0,0,0.30))" }}>
              <text x="500" y="160" textAnchor="middle" textLength="950" lengthAdjust="spacingAndGlyphs" fontFamily={FONT} fontWeight={400} fontSize="190" letterSpacing="-4" fill="#FFFAEE">Mixer chat</text>
            </svg>

            <section style={{ ...outerGlass, width: "min(820px, 92vw)" }}>
              <header style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 14, padding: "4px 8px 8px", flexWrap: "wrap" }}>
                <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                  <span style={{ fontSize: 12.5, color: "rgba(255,255,255,0.62)" }}>
                    {remainingMessages} of {appData.limits.messages} messages left today
                  </span>
                  <span style={{ ...smallPillGlass, padding: "2px 10px", borderRadius: 99, fontSize: 11.5, color: "rgba(255,255,255,0.82)" }}>
                    {appData.user.usage.tokens.toLocaleString()} / {appData.limits.tokens.toLocaleString()} tokens
                  </span>
                </div>
                <div style={{ display: "inline-flex", alignItems: "center", gap: 8 }}>
                  <ModelSelector models={models} selected={selectedModel} onChange={(model) => { setSelectedModel(model); localStorage.setItem("mixer:model", model); }} />
                  <span style={{ fontSize: 12, color: "rgba(255,255,255,0.48)" }}>
                    Resets {new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", weekday: "short" }).format(new Date(appData.limits.resets_at))}
                  </span>
                </div>
              </header>

              <div style={innerGlass}>
                <textarea
                  ref={textareaRef}
                  value={text}
                  onChange={(e) => setText(e.target.value)}
                  onKeyDown={handleKey}
                  onPaste={handlePaste}
                  placeholder="Ask anything, or start a new thread..."
                  rows={2}
                  disabled={remainingMessages === 0}
                  style={{ display: "block", width: "100%", background: "transparent", border: "none", outline: "none", resize: "none", padding: "12px 14px 6px", fontFamily: FONT, fontSize: 14.5, lineHeight: 1.6, color: "rgba(255,255,255,0.88)", caretColor: "rgba(255,255,255,0.7)" }}
                />
                <AttachmentTray attachments={attachments} onRemove={removeAttachment} />
                <footer style={{ display: "flex", alignItems: "center", padding: "4px 8px 8px", gap: 6 }}>
                  <input
                    ref={fileInputRef}
                    type="file"
                    multiple
                    onChange={(event) => void handleAttachmentInput(event.currentTarget.files)}
                    style={{ display: "none" }}
                  />
                  <IconBtn title="Attach file" onClick={() => fileInputRef.current?.click()}><Paperclip size={15} strokeWidth={1.7} /></IconBtn>
                  <span style={{ flex: 1 }} />
                  <span style={{ fontSize: 11.5, color: "rgba(255,255,255,0.36)" }}>{tokenPercent}% tokens used</span>
                  <IconBtn title="Send" onClick={() => void sendMessage()}><SendHorizontal size={15} strokeWidth={1.8} /></IconBtn>
                </footer>
              </div>

              {error && (
                <p style={{ margin: "8px 8px 0", color: "rgba(255,150,150,0.95)", fontSize: 12.5 }}>{error}</p>
              )}
            </section>
          </main>
        </div>

        <Sidebar open={sidebarOpen} onClose={() => setSidebarOpen(false)} user={appData.user} chats={appData.recent_chats} onDeleteChat={(chatId) => void deleteChat(chatId)} />
        <VultrPanel open={vultrPanelOpen} onClose={() => setVultrPanelOpen(false)} />
      </div>
    );
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
            "radial-gradient(ellipse 100% 100% at 50% 50%, transparent 42%, rgba(0,0,0,0.34) 100%)",
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
            position: "relative",
            display: "flex",
            alignItems: "center",
            padding: "14px 18px",
            gap: 12,
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
              fontFamily: FONT,
              fontSize: 13,
              color: "rgba(255,255,255,0.82)",
              cursor: "pointer",
              zIndex: 1,
            }}
          >
            <SidebarIcon size={14} strokeWidth={1.8} />
            Chats
          </button>
          <button
            type="button"
            onClick={() => setVultrPanelOpen(true)}
            style={{
              ...pillGlass,
              display: "inline-flex",
              alignItems: "center",
              gap: 8,
              padding: "8px 14px",
              borderRadius: 999,
              fontFamily: FONT,
              fontSize: 13,
              color: "rgba(255,255,255,0.82)",
              cursor: "pointer",
              zIndex: 1,
            }}
          >
            Vultr
          </button>

          <div
            style={{
              position: "absolute",
              left: "50%",
              top: "50%",
              transform: "translate(-50%, -50%)",
              maxWidth: "min(60%, 520px)",
              fontFamily: FONT,
              fontSize: 13,
              fontWeight: 500,
              color: "rgba(255,255,255,0.74)",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
              padding: "0 12px",
              zIndex: 0,
              textAlign: "center",
            }}
            title={chatTitle}
          >
            {chatTitle}
          </div>
        </header>

        {/* Main content row: chat column + embedded workspace panel */}
        <div style={{ flex: 1, display: "flex", minHeight: 0 }}>
          {/* Chat column */}
          <div style={{ flex: 1, display: "flex", flexDirection: "column", minWidth: 0 }}>
            <div
              ref={scrollRef}
              style={{
                flex: 1,
                overflowY: "auto",
                display: "flex",
                justifyContent: "center",
              }}
            >
              <div
                style={{
                  width: "min(820px, 92vw)",
                  padding: "20px 0 40px",
                  display: "flex",
                  flexDirection: "column",
                  gap: 12,
                }}
              >
                {isLoadingChat ? (
                  <div style={{ display: "flex", justifyContent: "center", paddingTop: 60 }}>
                    <div style={spinnerStyle} />
                  </div>
                ) : (
                  <>
                    {messages.map((msg, i) => {
                      const prev = messages[i - 1];
                      const showDivider =
                        msg.role === "assistant" &&
                        msg.model &&
                        prev?.role === "assistant" &&
                        prev.model &&
                        prev.model !== msg.model;
                      const modelLabel = (id: string) =>
                        models.find((m) => m.id === id)?.label ?? id;
                      return (
                        <React.Fragment key={msg.id}>
                          {showDivider && (
                            <ModelDivider from={modelLabel(prev.model!)} to={modelLabel(msg.model!)} />
                          )}
                          <MessageBubble message={msg} index={i} />
                        </React.Fragment>
                      );
                    })}
                    {error && (
                      <p style={{ color: "rgba(255,150,150,0.95)", fontSize: 13 }}>{error}</p>
                    )}
                  </>
                )}
              </div>
            </div>

            <div
              style={{
                padding: "0 20px 24px",
                display: "flex",
                justifyContent: "center",
              }}
            >
              <section style={{ ...outerGlass, width: "min(820px, 92vw)", padding: 8 }}>
                <header
                  style={{
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "flex-end",
                    padding: "2px 6px 8px",
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
                </header>
                <div style={innerGlass}>
                  <textarea
                    ref={textareaRef}
                    value={text}
                    onChange={(e) => setText(e.target.value)}
                    onKeyDown={handleKey}
                    onPaste={handlePaste}
                    placeholder="Reply to the conversation..."
                    rows={2}
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
                  <AttachmentTray attachments={attachments} onRemove={removeAttachment} />
                  <footer
                    style={{
                      display: "flex",
                      alignItems: "center",
                      padding: "4px 8px 8px",
                      gap: 6,
                    }}
                  >
                    <input
                      ref={fileInputRef}
                      type="file"
                      multiple
                      onChange={(event) => void handleAttachmentInput(event.currentTarget.files)}
                      style={{ display: "none" }}
                    />
                    <IconBtn title="Attach file" onClick={() => fileInputRef.current?.click()}>
                      <Paperclip size={15} strokeWidth={1.7} />
                    </IconBtn>
                    {workspaceState && (
                      <span style={{ position: "relative", display: "inline-flex" }}>
                        <IconBtn
                          title={workspacePanelOpen ? "Close workspace" : "Open workspace"}
                          onClick={() => setWorkspacePanelOpen((v) => !v)}
                        >
                          <Code2 size={15} strokeWidth={1.8} />
                        </IconBtn>
                        {!workspacePanelOpen && (
                          <span
                            aria-hidden
                            style={{
                              position: "absolute",
                              top: 4,
                              right: 4,
                              width: 6,
                              height: 6,
                              borderRadius: 999,
                              background: hasRunningTool
                                ? "rgba(100, 180, 255, 0.95)"
                                : "rgba(120, 220, 140, 0.95)",
                              boxShadow:
                                "inset 0 1px 0 rgba(255,255,255,0.50), 0 0 6px rgba(0,0,0,0.40), 0 0 0 1.5px rgba(0,0,0,0.45)",
                              animation: hasRunningTool
                                ? "pulse-dot 1.2s ease-in-out infinite"
                                : "none",
                              pointerEvents: "none",
                            }}
                          />
                        )}
                      </span>
                    )}
                    <span style={{ flex: 1 }} />
                    {isAgentBusy ? (
                      <IconBtn title="Stop generating" onClick={stopStreaming}>
                        <Square size={15} strokeWidth={1.8} />
                      </IconBtn>
                    ) : (
                      <IconBtn title="Send" onClick={() => void sendMessage()}>
                        <SendHorizontal size={15} strokeWidth={1.8} />
                      </IconBtn>
                    )}
                  </footer>
                </div>
              </section>
            </div>
          </div>

          {/* Drag-resize handle */}
          {workspaceState && workspacePanelOpen && (
            <div
              onPointerDown={onDividerPointerDown}
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize workspace"
              style={{
                width: 8,
                cursor: "col-resize",
                position: "relative",
                flexShrink: 0,
                zIndex: 3,
                touchAction: "none",
              }}
              onMouseEnter={(e) => {
                const line = e.currentTarget.querySelector<HTMLDivElement>("[data-line]");
                if (line) line.style.background = "rgba(255,255,255,0.18)";
              }}
              onMouseLeave={(e) => {
                if (isDraggingDivider) return;
                const line = e.currentTarget.querySelector<HTMLDivElement>("[data-line]");
                if (line) line.style.background = "rgba(255,255,255,0.06)";
              }}
            >
              <div
                data-line
                style={{
                  position: "absolute",
                  left: "50%",
                  top: 10,
                  bottom: 14,
                  width: 1,
                  marginLeft: -0.5,
                  borderRadius: 1,
                  background: isDraggingDivider ? "rgba(255,255,255,0.32)" : "rgba(255,255,255,0.06)",
                  transition: isDraggingDivider ? "none" : "background 0.16s ease",
                  pointerEvents: "none",
                }}
              />
            </div>
          )}

          {/* Embedded workspace panel */}
          {workspaceState && workspacePanelOpen && (
            <WorkspacePanel
              workspace={workspaceState}
              onClose={() => setWorkspacePanelOpen(false)}
              embedded
              width={workspaceWidth}
              resizing={isDraggingDivider}
            />
          )}
        </div>
      </div>

      <Sidebar
        open={sidebarOpen}
        onClose={() => setSidebarOpen(false)}
        user={appData.user}
        chats={appData.recent_chats}
        onDeleteChat={(chatId) => void deleteChat(chatId)}
      />
      <VultrPanel open={vultrPanelOpen} onClose={() => setVultrPanelOpen(false)} />
    </div>
  );
}

function debugFrontendStream(event: string, data: unknown) {
  console.log(
    `[frontend<-chat ${new Date().toISOString()} +${performance.now().toFixed(1)}ms]\n` +
      `event: ${event}\n` +
      `data: ${safeDebugString(data)}\n`,
  );
}

function readChatWebSocket(
  payload: ChatWsRequest,
  onEvent: (event: string, data: unknown) => void,
  registerStop?: (stop: () => void) => void,
) {
  return new Promise<void>((resolve, reject) => {
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(`${protocol}//${window.location.host}/api/chat/ws`);
    let completed = false;
    let aborted = false;

    registerStop?.(() => {
      aborted = true;
      socket.close();
    });

    socket.addEventListener("open", () => {
      socket.send(JSON.stringify(payload));
    });

    socket.addEventListener("message", (message) => {
      let parsed: unknown;
      try {
        parsed = JSON.parse(String(message.data));
      } catch {
        reject(new Error("Invalid websocket message"));
        socket.close();
        return;
      }

      if (!isChatWsEvent(parsed)) {
        reject(new Error("Invalid websocket event"));
        socket.close();
        return;
      }

      onEvent(parsed.event, parsed.data);
      if (parsed.event === "done") {
        completed = true;
        socket.close();
        resolve();
      }
    });

    socket.addEventListener("error", () => {
      if (!completed && !aborted) reject(new Error("Chat websocket failed"));
    });

    socket.addEventListener("close", () => {
      if (!completed) {
        if (aborted) resolve();
        else reject(new Error("Chat websocket closed before completion"));
      }
    });
  });
}

async function readAttachmentFile(file: File): Promise<PendingAttachment> {
  if (file.size > MAX_ATTACHMENT_BYTES) {
    throw new Error(`${file.name} is too large. Maximum attachment size is ${formatBytes(MAX_ATTACHMENT_BYTES)}.`);
  }

  const textLike = isTextLikeFile(file);
  const content = textLike ? await file.text() : await readFileAsDataUrl(file);
  return {
    id: safeUuid(),
    name: file.name,
    mime_type: file.type || "application/octet-stream",
    size: file.size,
    encoding: textLike ? "text" : "data_url",
    content,
  };
}

function readFileAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error(`Could not read ${file.name}`));
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.readAsDataURL(file);
  });
}

function filesFromClipboard(data: DataTransfer): File[] {
  const byKey = new Map<string, File>();
  const add = (file: File, index: number) => {
    const normalized = normalizePastedFile(file, index);
    const key = `${normalized.name}:${normalized.type}:${normalized.size}:${normalized.lastModified}`;
    byKey.set(key, normalized);
  };

  Array.from(data.files ?? []).forEach(add);
  Array.from(data.items ?? []).forEach((item, index) => {
    if (item.kind !== "file") return;
    const file = item.getAsFile();
    if (file) add(file, index);
  });

  return Array.from(byKey.values()).filter(isSupportedAttachmentFile);
}

function normalizePastedFile(file: File, index: number): File {
  if (file.name && file.name.trim() && file.name !== "image.png") return file;
  const ext = extensionForMime(file.type) ?? "bin";
  const prefix = file.type.startsWith("image/") ? "pasted-image" : "pasted-file";
  return new File([file], `${prefix}-${Date.now()}-${index + 1}.${ext}`, {
    type: file.type || "application/octet-stream",
    lastModified: file.lastModified || Date.now(),
  });
}

function extensionForMime(mime: string): string | null {
  const normalized = mime.toLowerCase();
  if (normalized === "image/png") return "png";
  if (normalized === "image/jpeg") return "jpg";
  if (normalized === "image/webp") return "webp";
  if (normalized === "image/gif") return "gif";
  if (normalized === "image/svg+xml") return "svg";
  if (normalized === "application/pdf") return "pdf";
  if (normalized === "text/plain") return "txt";
  if (normalized === "text/html") return "html";
  if (normalized === "text/markdown") return "md";
  if (normalized === "application/json") return "json";
  return null;
}

function isSupportedAttachmentFile(file: File): boolean {
  if (file.size <= 0) return false;
  return true;
}

function isTextLikeFile(file: File) {
  if (file.type.startsWith("text/")) return true;
  const lower = file.name.toLowerCase();
  return [
    ".txt", ".md", ".mdx", ".json", ".jsonl", ".csv", ".tsv", ".xml", ".html", ".css",
    ".js", ".jsx", ".ts", ".tsx", ".py", ".rs", ".go", ".java", ".c", ".cpp", ".h",
    ".hpp", ".rb", ".php", ".sh", ".zsh", ".bash", ".yaml", ".yml", ".toml", ".ini",
    ".env", ".sql", ".log", ".svg",
  ].some((ext) => lower.endsWith(ext));
}

function formatMessageWithAttachments(message: string, attachments: PendingAttachment[]) {
  const trimmed = message.trim();
  if (attachments.length === 0) return trimmed;
  const payload = attachments.map(({ id: _id, ...attachment }) => attachment);
  return `${trimmed ? `${trimmed}\n\n` : ""}${ATTACHMENT_START}\n${JSON.stringify(payload, null, 2)}\n${ATTACHMENT_END}`;
}

function splitMessageAttachments(content: string): {
  text: string;
  attachments: Array<Pick<PendingAttachment, "name" | "mime_type" | "size" | "encoding">>;
} {
  const start = content.indexOf(ATTACHMENT_START);
  const end = content.indexOf(ATTACHMENT_END);
  if (start === -1 || end === -1 || end <= start) {
    return { text: content, attachments: [] };
  }
  const text = content.slice(0, start).trimEnd();
  const raw = content.slice(start + ATTACHMENT_START.length, end).trim();
  try {
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return { text, attachments: [] };
    return {
      text,
      attachments: parsed
        .filter((item): item is Record<string, unknown> => item && typeof item === "object")
        .map((item) => ({
          name: typeof item.name === "string" ? item.name : "attachment",
          mime_type: typeof item.mime_type === "string" ? item.mime_type : "",
          size: typeof item.size === "number" ? item.size : 0,
          encoding: item.encoding === "data_url" ? "data_url" : "text",
        })),
    };
  } catch {
    return { text, attachments: [] };
  }
}

function formatBytes(bytes: number) {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const power = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
  return `${(bytes / 1024 ** power).toFixed(power === 0 ? 0 : 1)} ${units[power]}`;
}

function extractWorkspaceIdFromToolCall(call: AgentToolCall): string | null {
  const candidates = [
    call.resultXml,
    call.output,
    call.error,
    call.body,
  ].filter((value): value is string => typeof value === "string" && value.length > 0);
  for (const candidate of candidates) {
    const decoded = decodeXml(candidate);
    const quoted = decoded.match(/workspace_id=["']([^"']+)["']/i);
    if (quoted?.[1]) return quoted[1].trim();
    const labeled = decoded.match(/workspace_id\s*:\s*([a-z0-9][a-z0-9_-]*)/i);
    if (labeled?.[1]) return labeled[1].trim();
  }
  return null;
}

function workspaceRecoveryKey(chatId: string) {
  return `mixer:chat:${chatId}:workspace`;
}

function readRecoveredWorkspace(chatId: string): { id: string; name: string } | null {
  try {
    const raw = localStorage.getItem(workspaceRecoveryKey(chatId));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as { id?: unknown; name?: unknown };
    if (typeof parsed.id !== "string" || parsed.id.trim().length === 0) return null;
    return {
      id: parsed.id,
      name: typeof parsed.name === "string" && parsed.name.trim() ? parsed.name : parsed.id,
    };
  } catch {
    return null;
  }
}

function isChatWsEvent(value: unknown): value is { event: string; data: unknown } {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as { event?: unknown }).event === "string" &&
    "data" in value
  );
}

function safeDebugString(data: unknown) {
  try {
    const value = typeof data === "string" ? data : JSON.stringify(data);
    return value.length > 2000 ? `${value.slice(0, 2000)}...[truncated]` : value;
  } catch {
    return String(data);
  }
}

function looksLikeControlPlaneLeak(content: string) {
  const trimmed = content.trim();
  if (!trimmed) return false;
  if (trimmed.startsWith("<think>")) return true;
  if (trimmed.startsWith("{") && trimmed.includes('"type"')) return true;
  if (trimmed.includes('"type":"tool_call"')) return true;
  if (trimmed.includes('"type": "tool_call"')) return true;
  if (trimmed.includes('"arguments"') && trimmed.includes('"path"') && trimmed.includes('"id"')) return true;
  if (trimmed.includes('"assistant_message_uuid"') || trimmed.includes('"final_text"')) return true;
  return false;
}

function ModelDivider({ from, to }: { from: string; to: string }) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 10,
        margin: "4px 0",
        userSelect: "none",
      }}
    >
      <div style={{ flex: 1, height: 1, background: "rgba(255,255,255,0.08)" }} />
      <div
        style={{
          display: "inline-flex",
          alignItems: "center",
          gap: 7,
          padding: "4px 12px",
          borderRadius: 999,
          background: "rgba(255,255,255,0.05)",
          border: "1px solid rgba(255,255,255,0.09)",
          fontSize: 11,
          letterSpacing: "0.02em",
          color: "rgba(255,255,255,0.40)",
          fontFamily: FONT,
        }}
      >
        <span style={{ color: "rgba(255,255,255,0.55)", fontWeight: 500 }}>{from}</span>
        <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden style={{ opacity: 0.45 }}>
          <path d="M2 6h8M7 3l3 3-3 3" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
        <span style={{ color: "rgba(255,255,255,0.55)", fontWeight: 500 }}>{to}</span>
      </div>
      <div style={{ flex: 1, height: 1, background: "rgba(255,255,255,0.08)" }} />
    </div>
  );
}

function AttachmentTray({
  attachments,
  onRemove,
}: {
  attachments: PendingAttachment[];
  onRemove: (id: string) => void;
}) {
  if (attachments.length === 0) return null;
  return (
    <div
      style={{
        display: "flex",
        flexWrap: "wrap",
        gap: 6,
        padding: "2px 10px 7px",
      }}
    >
      {attachments.map((attachment) => (
        <span
          key={attachment.id}
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: 6,
            maxWidth: 240,
            padding: "5px 7px",
            borderRadius: 8,
            background: "rgba(255,255,255,0.07)",
            border: "1px solid rgba(255,255,255,0.12)",
            color: "rgba(255,255,255,0.78)",
            fontSize: 11.5,
          }}
        >
          <FileText size={13} strokeWidth={1.7} style={{ flexShrink: 0, color: "rgba(180,210,255,0.85)" }} />
          <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{attachment.name}</span>
          <span style={{ color: "rgba(255,255,255,0.38)", flexShrink: 0 }}>{formatBytes(attachment.size)}</span>
          <button
            type="button"
            aria-label={`Remove ${attachment.name}`}
            onClick={() => onRemove(attachment.id)}
            style={{
              display: "inline-flex",
              alignItems: "center",
              justifyContent: "center",
              width: 18,
              height: 18,
              border: "none",
              borderRadius: 999,
              background: "rgba(255,255,255,0.08)",
              color: "rgba(255,255,255,0.62)",
              cursor: "pointer",
              padding: 0,
              flexShrink: 0,
            }}
          >
            <X size={12} strokeWidth={1.8} />
          </button>
        </span>
      ))}
    </div>
  );
}

function MessageBubble({ message, index }: { message: UiMessage; index: number }) {
  const isUser = message.role === "user";
  void index;
  const userContent = isUser ? splitMessageAttachments(message.content) : null;

  return (
    <div
      style={{
        display: "flex",
        justifyContent: isUser ? "flex-end" : "flex-start",
      }}
    >
      <div
        style={{
          maxWidth: "78%",
          minWidth: 0,
          overflow: "hidden",
          overflowWrap: "anywhere",
          fontFamily: FONT,
          fontSize: 14,
          lineHeight: 1.55,
          color: "rgba(255,255,255,0.92)",
          ...(isUser ? userBubbleStyle : assistantBubbleStyle),
        }}
      >
        {!isUser && (
          <div
            style={{
              fontFamily: FONT,
              fontSize: 10.5,
              fontWeight: 500,
              letterSpacing: "0.08em",
              textTransform: "uppercase",
              color: "rgba(255,255,255,0.40)",
              marginBottom: 4,
            }}
          >
            Mixer
          </div>
        )}
        {isUser ? (
          <UserMessageContent text={userContent?.text ?? message.content} attachments={userContent?.attachments ?? []} />
        ) : (
          <AssistantContent
            content={message.content}
            activeTools={message.activeTools ?? []}
            isStreaming={message.isStreaming}
            toolCalls={message.toolCalls ?? []}
            activeThought={message.activeThought}
            runStatus={message.runStatus}
            runError={message.runError}
            thinkingTotalMs={message.thinkingTotalMs}
          />
        )}
      </div>
    </div>
  );
}

function UserMessageContent({
  text,
  attachments,
}: {
  text: string;
  attachments: Array<Pick<PendingAttachment, "name" | "mime_type" | "size" | "encoding">>;
}) {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: attachments.length > 0 ? 8 : 0 }}>
      {text.trim() ? <span style={{ whiteSpace: "pre-wrap" }}>{text}</span> : null}
      {attachments.length > 0 && (
        <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
          {attachments.map((attachment, index) => (
            <span
              key={`${attachment.name}-${index}`}
              title={`${attachment.mime_type || "file"} - ${formatBytes(attachment.size)} - ${attachment.encoding}`}
              style={{
                display: "inline-flex",
                alignItems: "center",
                gap: 6,
                maxWidth: 260,
                padding: "5px 8px",
                borderRadius: 8,
                background: "rgba(0,0,0,0.18)",
                border: "1px solid rgba(255,255,255,0.13)",
                color: "rgba(255,255,255,0.82)",
                fontSize: 11.5,
              }}
            >
              <FileText size={13} strokeWidth={1.7} style={{ flexShrink: 0, color: "rgba(180,210,255,0.90)" }} />
              <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{attachment.name}</span>
              <span style={{ flexShrink: 0, color: "rgba(255,255,255,0.42)" }}>{formatBytes(attachment.size)}</span>
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

function AssistantContent({
  content,
  activeTools,
  isStreaming,
  toolCalls,
  activeThought,
  runStatus,
  runError,
  thinkingTotalMs,
}: {
  content: string;
  activeTools: string[];
  isStreaming?: boolean;
  toolCalls?: AgentToolCall[];
  activeThought?: {
    status: "thinking" | "done";
    durationMs?: number;
  };
  runStatus?: "running" | "completed" | "failed" | "cancelled";
  runError?: string;
  thinkingTotalMs?: number;
}) {
  const runningTool = toolCalls?.find((call) => call.status === "running" || call.status === "pending");
  const showRunActivity = runStatus === "running";
  const leakedControlPlane = looksLikeControlPlaneLeak(content);
  const isLive = !!isStreaming || runStatus === "running";

  // Structured tool-call cards from agent_event events.
  //
  // While the run is live, render each tool call as a separate card so the user
  // sees the real-time progress (pending → running → succeeded/failed).
  //
  // Once the run is complete, consolidate the entire activity into a single
  // collapsible timeline at the top of the message — compact summary header
  // ("Thought for 4.2s · 6 steps") plus per-step expanders.
  let toolCallTimeline: ReactNode = null;
  if (toolCalls && toolCalls.length > 0) {
    if (isLive) {
      toolCallTimeline = (
        <div style={{ display: "flex", flexDirection: "column", gap: 6, marginBottom: 10 }}>
          {toolCalls
            .slice()
            .sort((a, b) => a.seq - b.seq || a.attempt - b.attempt)
            .map((tc) => (
              <ToolCallCard key={tc.uuid} call={tc} />
            ))}
          {showRunActivity && !runningTool ? (
            <ToolLoading tool="planning next step" />
          ) : null}
          {(runStatus === "failed" || runStatus === "cancelled") && runError ? (
            <div
              style={{
                padding: "8px 12px",
                border: runStatus === "cancelled" ? "1px solid rgba(255,255,255,0.14)" : "1px solid rgba(220, 80, 80, 0.30)",
                borderRadius: 8,
                background: runStatus === "cancelled" ? "rgba(255,255,255,0.07)" : "rgba(220, 80, 80, 0.10)",
                color: runStatus === "cancelled" ? "rgba(255,255,255,0.70)" : "rgba(255, 200, 200, 0.85)",
                fontSize: 12,
              }}
            >
              {runStatus === "cancelled" ? runError : `Run failed: ${runError}`}
            </div>
          ) : null}
        </div>
      );
    } else {
      toolCallTimeline = (
        <ActivityTimeline
          toolCalls={toolCalls}
          thinkingMs={thinkingTotalMs ?? activeThought?.durationMs ?? null}
          runStatus={runStatus}
          runError={runError}
        />
      );
    }
  }
  if (isStreaming) {
    if (leakedControlPlane) {
      return (
        <>
          <style>{TOOL_KEYFRAMES}</style>
          {toolCallTimeline}
          <ToolLoading tool={runningTool?.name ?? "planning next step"} />
        </>
      );
    }

    const parts = parseToolParts(content);

    // When we have structured tool_calls, prefer them and suppress text-parsed tool tags
    // (they're noise — the structured cards have the real status). Keep only text and
    // the `response`/`code` complete-tool blocks that the user actually wants to see.
    const hasStructuredTools = (toolCalls?.length ?? 0) > 0;
    const visibleParts = hasStructuredTools
      ? parts.filter((p) =>
          p.kind === "text"
          || (p.kind === "tool" && (p.name === "response" || p.name === "code")),
        )
      : parts;

    return (
      <>
        <style>{TOOL_KEYFRAMES}</style>
        {toolCallTimeline}
        {activeThought ? <ThoughtCard thought={activeThought} /> : null}
        {visibleParts.map((part, index) => {
          if (part.kind === "text") {
            if (part.content.length === 0) return null;
            return (
              <span key={index} style={{ whiteSpace: "pre-wrap" }}>
                {part.content}
              </span>
            );
          }

          if (part.kind === "pending") {
            return <ToolLoading key={index} tool={streamingToolLabel(part.name)} />;
          }

          if (part.name === "response") {
            return (
              <span key={index} style={{ whiteSpace: "pre-wrap" }}>
                {part.content}
              </span>
            );
          }

          if (part.name === "code") {
            if (isMermaidLanguage(part.attrs.language)) {
              return (
                <Suspense key={index} fallback={<PlainCodeBlock code={part.content.trim()} />}>
                  <LazyMermaidBlock code={part.content.trim()} />
                </Suspense>
              );
            }
            return <PlainCodeBlock key={index} code={part.content.trim()} />;
          }

          if (part.name === "html") {
            return <ToolLoading key={index} tool={streamingToolLabel(part.name)} />;
          }

          // Completed tool_result blocks (from replace events or processed content) —
          // show as collapsed results, not spinners
          if (part.name === "tool_result") {
            const toolName = part.attrs.tool || "tool";
            const collapsible = ["search", "query", "python", "completeTodo", "viewTodo",
              "listSkills", "readSkill", "workspace"].includes(toolName);
            return (
              <ToolResult
                key={index}
                tool={toolName}
                attrs={part.attrs}
                content={decodeXml(part.content)}
                collapsible={collapsible}
                defaultCollapsed={true}
              />
            );
          }

          return <ToolLoading key={index} tool={streamingToolLabel(part.name)} />;
        })}
        {isStreaming && <span style={streamCursorStyle} aria-hidden />}
      </>
    );
  }

  const parts = parseToolParts(content);
  const inlineToolNames = new Set(
    parts.flatMap((part) =>
      part.kind !== "text" && isExecutingTool(part.name) ? [part.name] : [],
    ),
  );

  return (
    <>
      <style>{TOOL_KEYFRAMES}</style>
      {toolCallTimeline}
      {activeThought && runStatus === "running" ? <ThoughtCard thought={activeThought} /> : null}
      {!isStreaming && leakedControlPlane && runStatus === "running" && !runningTool ? (
        <ToolLoading tool="planning next step" />
      ) : null}
      {!isStreaming && leakedControlPlane && runStatus !== "running" ? (
        <ToolResult
          tool="agent"
          content={
            runStatus === "failed"
              ? runError || "The agent produced an invalid control message."
              : runStatus === "cancelled"
                ? runError || "Stopped by user"
              : "The agent produced an internal action payload instead of a user-facing answer."
          }
        />
      ) : null}
      {!leakedControlPlane ? parts.map((part, index) => {
        if (part.kind === "text") {
          return <Markdown key={index} content={part.content} />;
        }

        if (part.name === "response") {
          return <Markdown key={index} content={part.content} />;
        }

        if (part.kind === "pending") {
          if (part.name === "response") {
            return <Markdown key={index} content={part.content} />;
          }
          // tool_result is backend-generated; if it's pending it's a stream artifact, suppress it
          if (part.name === "tool_result") {
            return null;
          }
          return isStreaming ? (
            <ToolLoading key={index} tool={part.name} />
          ) : (
            <ToolResult
              key={index}
              tool={part.name}
              content={`The ${part.name} tool call was incomplete and did not finish.`}
            />
          );
        }

        if (isExecutingTool(part.name)) {
          return isStreaming ? (
            <ToolLoading key={index} tool={part.name} />
          ) : (
            <ToolResult
              key={index}
              tool={part.name}
              content={`The ${part.name} tool call was not executed in this session.`}
            />
          );
        }

        if (part.name === "tool_result") {
          const toolName = part.attrs.tool || "tool";
          const collapsible = ["search", "query", "python", "completeTodo", "viewTodo", "listSkills", "readSkill", "workspace"].includes(toolName);
          const defaultCollapsed = collapsible && toolName !== "createTodo";
          return (
            <ToolResult
              key={index}
              tool={toolName}
              attrs={part.attrs}
              content={decodeXml(part.content)}
              collapsible={collapsible}
              defaultCollapsed={defaultCollapsed}
            />
          );
        }

        if (part.name === "code") {
          if (isMermaidLanguage(part.attrs.language)) {
            return (
              <Suspense key={index} fallback={<PlainCodeBlock code={part.content.trim()} />}>
                <LazyMermaidBlock code={part.content.trim()} />
              </Suspense>
            );
          }
          return (
            <HighlightedCodeBlock
              key={index}
              code={part.content.trim()}
              language={part.attrs.language}
            />
          );
        }

        if (part.name === "html") {
          return (
            <iframe
              key={index}
              sandbox=""
              srcDoc={part.content}
              title="Rendered HTML tool output"
              style={{
                width: "100%",
                minHeight: 180,
                marginTop: 8,
                border: "1px solid rgba(255,255,255,0.12)",
                borderRadius: 10,
                background: "rgba(255,255,255,0.94)",
              }}
            />
          );
        }

        return (
          <ToolResult
            key={index}
            tool={part.name}
            content={part.content.trim() || "Tool call parsed."}
          />
        );
      }) : null}
      {activeTools
        .filter((tool) => !inlineToolNames.has(tool))
        .map((tool) => (
          <ToolLoading key={`active-${tool}`} tool={tool} />
        ))}
      {isStreaming && content.length > 0 && activeTools.length === 0 && (
        <span style={streamCursorStyle} aria-hidden />
      )}
    </>
  );
}

function ActivityTimeline({
  toolCalls,
  thinkingMs,
  runStatus,
  runError,
}: {
  toolCalls: AgentToolCall[];
  thinkingMs: number | null;
  runStatus?: "running" | "completed" | "failed" | "cancelled";
  runError?: string;
}) {
  const [open, setOpen] = useState(false);
  const [expandedSteps, setExpandedSteps] = useState<Set<string>>(() => new Set());

  const sorted = useMemo(
    () => toolCalls.slice().sort((a, b) => a.seq - b.seq || a.attempt - b.attempt),
    [toolCalls],
  );

  const stepCount = sorted.length;
  const failed = sorted.filter((c) => c.status === "failed").length;
  const secondsLabel =
    thinkingMs && thinkingMs >= 100
      ? `${Math.max(0.1, thinkingMs / 1000).toFixed(1)}s`
      : null;

  function toggleStep(uuid: string) {
    setExpandedSteps((prev) => {
      const next = new Set(prev);
      if (next.has(uuid)) next.delete(uuid);
      else next.add(uuid);
      return next;
    });
  }

  return (
    <div style={timelineCardStyle}>
      <button type="button" onClick={() => setOpen((v) => !v)} style={timelineHeaderStyle}>
        <span style={timelineHeaderGlyphStyle} aria-hidden>✦</span>
        <span style={timelineHeaderTextStyle}>
          {secondsLabel ? (
            <>
              Thought for <span style={{ color: "rgba(255,255,255,0.85)" }}>{secondsLabel}</span>
            </>
          ) : (
            "Activity"
          )}
          {stepCount > 0 && (
            <>
              <span style={timelineSepStyle}>·</span>
              <span>{stepCount} step{stepCount === 1 ? "" : "s"}</span>
            </>
          )}
          {failed > 0 && (
            <>
              <span style={timelineSepStyle}>·</span>
              <span style={{ color: "rgba(255, 130, 130, 0.92)" }}>{failed} failed</span>
            </>
          )}
        </span>
        <ChevronDown
          size={13}
          style={{
            color: "rgba(255,255,255,0.50)",
            transform: open ? "rotate(180deg)" : "rotate(0deg)",
            transition: "transform 0.18s ease",
            flexShrink: 0,
          }}
        />
      </button>

      {open && (
        <div style={timelineBodyStyle}>
          {sorted.length > 1 && <span style={timelineRailStyle} aria-hidden />}
          {sorted.map((call) => (
            <TimelineStep
              key={call.uuid}
              call={call}
              expanded={expandedSteps.has(call.uuid)}
              onToggle={() => toggleStep(call.uuid)}
            />
          ))}
          {runStatus === "failed" && runError && (
            <div style={timelineRunErrorStyle}>Run failed: {runError}</div>
          )}
          {runStatus === "cancelled" && (
            <div style={timelineRunErrorStyle}>{runError || "Stopped by user"}</div>
          )}
        </div>
      )}
    </div>
  );
}

function TimelineStep({
  call,
  expanded,
  onToggle,
}: {
  call: AgentToolCall;
  expanded: boolean;
  onToggle: () => void;
}) {
  const statusColor = {
    pending: "rgba(255,255,255,0.45)",
    running: "rgba(100, 180, 255, 0.95)",
    succeeded: "rgba(120, 220, 140, 0.95)",
    failed: "rgba(255, 110, 110, 0.95)",
    approval_required: "rgba(255, 195, 95, 0.95)",
  }[call.status];

  const statusLabel = {
    pending: "queued",
    running: "running",
    succeeded: "done",
    failed: "failed",
    approval_required: "approval",
  }[call.status];

  const argSummary = Object.entries(call.args)
    .map(([k, v]) => `${k}=${truncateForLine(String(v ?? ""), 22)}`)
    .join(" ");
  const hasDetail = (call.output && call.output.length > 0) || (call.error && call.error.length > 0);

  return (
    <div style={timelineStepStyle}>
      <span
        style={{
          ...timelineDotStyle,
          background: statusColor,
        }}
        aria-hidden
      />
      <button
        type="button"
        onClick={() => hasDetail && onToggle()}
        disabled={!hasDetail}
        style={{
          ...timelineStepHeaderStyle,
          cursor: hasDetail ? "pointer" : "default",
        }}
      >
        <span style={timelineStepNameStyle}>{call.name}</span>
        {argSummary && <span style={timelineStepArgsStyle}>{argSummary}</span>}
        <span
          style={{
            ...timelineStepStatusStyle,
            color: statusColor,
          }}
        >
          {statusLabel}
          {call.attempt > 1 && ` · ${call.attempt}`}
        </span>
        {hasDetail && (
          <ChevronDown
            size={11}
            style={{
              color: "rgba(255,255,255,0.40)",
              transform: expanded ? "rotate(180deg)" : "rotate(0deg)",
              transition: "transform 0.18s ease",
              flexShrink: 0,
            }}
          />
        )}
      </button>
      {expanded && hasDetail && (
        <pre
          style={{
            ...timelineStepDetailStyle,
            color: call.status === "failed" ? "rgba(255, 180, 180, 0.88)" : "rgba(255,255,255,0.72)",
          }}
        >
          {call.error ? call.error : truncateForLine(call.output ?? "", 4000)}
        </pre>
      )}
    </div>
  );
}

function ToolCallCard({ call }: { call: AgentToolCall }) {
  const [expanded, setExpanded] = useState(false);
  const statusColor = {
    pending: "rgba(255,255,255,0.40)",
    running: "rgba(100, 180, 255, 0.95)",
    succeeded: "rgba(120, 220, 140, 0.95)",
    failed: "rgba(255, 110, 110, 0.95)",
    approval_required: "rgba(255, 195, 95, 0.95)",
  }[call.status];
  const statusLabel = {
    pending: "queued",
    running: "running",
    succeeded: "done",
    failed: "failed",
    approval_required: "approval",
  }[call.status];
  const argSummary = Object.entries(call.args)
    .map(([k, v]) => `${k}=${truncateForLine(String(v ?? ""), 24)}`)
    .join(" ");
  const hasDetail = (call.output && call.output.length > 0) || (call.error && call.error.length > 0);
  return (
    <div
      style={{
        ...pillGlass,
        borderRadius: 10,
        border: call.status === "failed" || call.status === "approval_required"
          ? "1px solid rgba(220, 80, 80, 0.40)"
          : "1px solid rgba(255,255,255,0.08)",
        borderTop: call.status === "failed" || call.status === "approval_required"
          ? "1px solid rgba(255, 150, 150, 0.30)"
          : "1px solid rgba(255,255,255,0.18)",
        overflow: "hidden",
        minWidth: 0,
        animation: "tool-card-in 200ms cubic-bezier(0.22,1,0.36,1) both",
      }}
    >
      <button
        type="button"
        onClick={() => hasDetail && setExpanded((e) => !e)}
        style={{
          width: "100%",
          background: "transparent",
          border: "none",
          padding: "8px 12px",
          display: "flex",
          alignItems: "center",
          gap: 10,
          minWidth: 0,
          cursor: hasDetail ? "pointer" : "default",
          fontFamily: "inherit",
          color: "inherit",
          textAlign: "left",
        }}
      >
        <span
          style={{
            width: 7,
            height: 7,
            borderRadius: 999,
            background: statusColor,
            flexShrink: 0,
            boxShadow: "inset 0 1px 0 rgba(255,255,255,0.45), 0 0 8px rgba(0,0,0,0.30)",
            animation: call.status === "running" ? "pulse-dot 1.4s ease-in-out infinite" : "none",
          }}
        />
        <span
          style={{
            fontFamily: "'DM Mono', ui-monospace, monospace",
            fontSize: 12,
            color: "rgba(255,255,255,0.88)",
            fontWeight: 500,
            flexShrink: 0,
          }}
        >
          {call.name}
        </span>
        {argSummary && (
          <span
            style={{
              fontFamily: "'DM Mono', ui-monospace, monospace",
              fontSize: 11,
              color: "rgba(255,255,255,0.45)",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
              flex: 1,
              minWidth: 0,
            }}
          >
            {argSummary}
          </span>
        )}
        <span
          style={{
            ...smallPillGlass,
            padding: "3px 8px",
            borderRadius: 99,
            fontSize: 9.5,
            letterSpacing: "0.10em",
            textTransform: "uppercase",
            color: statusColor,
            flexShrink: 0,
            fontWeight: 500,
          }}
        >
          {statusLabel}
          {call.attempt > 1 && ` · ${call.attempt}`}
        </span>
        {hasDetail && (
          <ChevronDown
            size={13}
            style={{
              color: "rgba(255,255,255,0.40)",
              transform: expanded ? "rotate(180deg)" : "rotate(0deg)",
              transition: "transform 0.18s ease",
              flexShrink: 0,
            }}
          />
        )}
      </button>
      {expanded && hasDetail && (
        <div
          style={{
            padding: "8px 12px 10px 12px",
            borderTop: "1px solid rgba(255,255,255,0.06)",
            fontFamily: "'DM Mono', ui-monospace, monospace",
            fontSize: 11.5,
            color: call.status === "failed" || call.status === "approval_required" ? "rgba(255, 210, 160, 0.88)" : "rgba(255,255,255,0.72)",
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
            overflowWrap: "anywhere",
            maxHeight: 240,
            overflowY: "auto",
            background: "rgba(0,0,0,0.18)",
          }}
        >
          {call.error ? call.error : truncateForLine(call.output ?? "", 4000)}
        </div>
      )}
    </div>
  );
}

const THINKING_WORDS = [
  "Pondering", "Musing", "Ruminating", "Mulling", "Wandering",
  "Brewing", "Voyaging", "Tinkering", "Marinating", "Percolating",
  "Conjuring", "Untangling", "Charting", "Weaving", "Distilling",
  "Composing", "Synthesizing", "Reflecting", "Deliberating", "Sketching",
  "Wrangling", "Sifting", "Decanting", "Wayfinding", "Cogitating",
  "Reckoning", "Discerning", "Steeping", "Threading", "Plotting",
  "Hatching", "Drafting", "Crafting", "Surveying", "Reasoning",
];

// Cycling glyphs that read like a starburst spinning between forms.
// Order chosen so adjacent frames feel like a rotation, not a slideshow.
const THINKING_GLYPHS = ["✢", "✣", "✤", "✥", "✦", "✧", "✩", "✪", "✫", "✬", "✭", "✮", "✯"];

function pickThinkingWord(prev?: string): string {
  let next = prev;
  while (!next || next === prev) {
    next = THINKING_WORDS[Math.floor(Math.random() * THINKING_WORDS.length)];
  }
  return next;
}

function ThoughtCard({
  thought,
}: {
  thought: { status: "thinking" | "done"; durationMs?: number };
}) {
  const done = thought.status === "done";
  const seconds = typeof thought.durationMs === "number"
    ? Math.max(0.1, thought.durationMs / 1000).toFixed(1)
    : null;

  const [word, setWord] = useState(() => pickThinkingWord());
  const [glyphIdx, setGlyphIdx] = useState(0);

  useEffect(() => {
    if (done) return;
    const glyphTimer = window.setInterval(() => {
      setGlyphIdx((i) => (i + 1) % THINKING_GLYPHS.length);
    }, 130);
    const wordTimer = window.setInterval(() => {
      setWord((prev) => pickThinkingWord(prev));
    }, 2400);
    return () => {
      window.clearInterval(glyphTimer);
      window.clearInterval(wordTimer);
    };
  }, [done]);

  if (done) {
    return (
      <div style={thinkingInlineStyle}>
        <span style={thinkingDoneGlyphStyle} aria-hidden>✦</span>
        <span style={thinkingDoneTextStyle}>Thought for {seconds}s</span>
      </div>
    );
  }

  return (
    <div style={thinkingInlineStyle}>
      <span style={thinkingGlyphStyle} aria-hidden>{THINKING_GLYPHS[glyphIdx]}</span>
      <span key={word} style={thinkingShimmerStyle}>{word}…</span>
    </div>
  );
}

function truncateForLine(s: string, max: number) {
  if (s.length <= max) return s;
  return s.slice(0, max) + "…";
}

function ToolLoading({ tool }: { tool: string }) {
  return (
    <div style={{ ...toolBlockStyle, animation: "tool-card-in 180ms ease-out both" }}>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 9,
          color: "rgba(255,255,255,0.82)",
          fontSize: 13,
        }}
      >
        <span style={spinnerStyle} />
        <span>Executing {tool}</span>
      </div>
      <div
        style={{
          marginTop: 8,
          height: 4,
          overflow: "hidden",
          borderRadius: 999,
          background: "rgba(255,255,255,0.08)",
        }}
      >
        <span style={progressStyle} />
      </div>
    </div>
  );
}

function ToolResult({
  tool,
  attrs = {},
  content,
  collapsible = false,
  defaultCollapsed = false,
}: {
  tool: string;
  attrs?: Record<string, string>;
  content: string;
  collapsible?: boolean;
  defaultCollapsed?: boolean;
}) {
  const [collapsed, setCollapsed] = useState(defaultCollapsed);

  const label = toolResultLabel(tool, attrs);

  return (
    <div style={toolBlockStyle}>
      <div
        onClick={collapsible ? () => setCollapsed((c) => !c) : undefined}
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 8,
          minWidth: 0,
          cursor: collapsible ? "pointer" : "default",
          userSelect: "none",
          marginBottom: collapsed ? 0 : 6,
        }}
      >
        <span
          style={{
            fontSize: 10,
            letterSpacing: "0.08em",
            textTransform: "uppercase",
            color: "rgba(255,255,255,0.45)",
            minWidth: 0,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {label}
        </span>
        {collapsible && (
          <ChevronDown
            size={13}
            strokeWidth={2}
            style={{
              color: "rgba(255,255,255,0.35)",
              transform: collapsed ? "rotate(-90deg)" : "rotate(0deg)",
              transition: "transform 160ms ease",
              flexShrink: 0,
            }}
          />
        )}
      </div>
      {!collapsed && (
        <pre
          style={{
            margin: 0,
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
            overflowWrap: "anywhere",
            fontFamily: FONT,
            fontSize: 13,
            lineHeight: 1.55,
            color: "rgba(255,255,255,0.88)",
          }}
        >
          {content}
        </pre>
      )}
    </div>
  );
}

function toolResultLabel(tool: string, attrs: Record<string, string>): string {
  switch (tool) {
    case "search": {
      const q = attrs.query;
      return q ? `searched — ${q}` : "search result";
    }
    case "python":
      return "python output";
    case "query":
      return "conversation search";
    case "end":
      return "end";
    case "done":
      return "done";
    case "createTodo":
      return "todo list";
    case "completeTodo":
      return `task ${attrs.task ?? "?"} completed`;
    case "viewTodo":
      return "todo progress";
    case "listSkills":
      return "available skills";
    case "readSkill": {
      const n = attrs.name;
      return n ? `skill — ${n}` : "skill loaded";
    }
    case "workspace": {
      const action = attrs.action;
      if (action === "CreateWorkspace") return "workspace created";
      if (action === "Command" || action === "LongRunProcess") return "command output";
      if (action === "CreateFile") return "file created";
      if (action === "AppendFile") return "file appended";
      if (action === "PatchFile") return "file patched";
      if (action === "Preview") return "preview opened";
      if (action === "ReadFile") return "file content";
      if (action === "DeleteFile") return "file deleted";
      if (action === "CreateDirectory") return "directory created";
      if (action === "DeleteDirectory") return "directory deleted";
      if (action === "WorkspaceStatus") return "workspace status";
      return "workspace";
    }
    default:
      return `${tool} result`;
  }
}

const userBubbleStyle = {
  ...pillGlass,
  borderRadius: 14,
  padding: "11px 14px",
};

const assistantBubbleStyle = {
  ...innerGlass,
  borderRadius: 14,
  padding: "11px 14px",
};

const toolBlockStyle = {
  marginTop: 8,
  padding: "10px 12px",
  borderRadius: 10,
  background: "rgba(0,0,0,0.30)",
  border: "1px solid rgba(255,255,255,0.08)",
};

const spinnerStyle = {
  width: 14,
  height: 14,
  borderRadius: 999,
  border: "2px solid rgba(255,255,255,0.16)",
  borderTopColor: "rgba(255,255,255,0.76)",
  animation: "tool-spin 760ms linear infinite",
};

const progressStyle = {
  display: "block",
  width: "48%",
  height: "100%",
  borderRadius: 999,
  background: "linear-gradient(90deg, transparent, rgba(255,255,255,0.55), transparent)",
  animation: "tool-progress 1.1s cubic-bezier(0.22,1,0.36,1) infinite",
};

/* ── Activity timeline (final-message summary) ─────────────────────── */

const timelineCardStyle: CSSProperties = {
  ...pillGlass,
  borderRadius: 11,
  marginBottom: 10,
  overflow: "hidden",
  animation: "tool-card-in 200ms cubic-bezier(0.22,1,0.36,1) both",
};

const timelineHeaderStyle: CSSProperties = {
  width: "100%",
  display: "flex",
  alignItems: "center",
  gap: 9,
  padding: "8px 12px",
  background: "transparent",
  border: "none",
  color: "inherit",
  cursor: "pointer",
  textAlign: "left",
  fontFamily: FONT,
  fontSize: 12.5,
};

const timelineHeaderGlyphStyle: CSSProperties = {
  fontFamily: "'DM Mono', ui-monospace, monospace",
  fontSize: 13,
  lineHeight: 1,
  color: "rgba(255,255,255,0.65)",
  width: 13,
  textAlign: "center",
};

const timelineHeaderTextStyle: CSSProperties = {
  flex: 1,
  display: "inline-flex",
  alignItems: "center",
  gap: 4,
  color: "rgba(255,255,255,0.65)",
  fontWeight: 500,
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
  minWidth: 0,
};

const timelineSepStyle: CSSProperties = {
  color: "rgba(255,255,255,0.28)",
  margin: "0 2px",
};

const timelineBodyStyle: CSSProperties = {
  position: "relative",
  padding: "6px 12px 10px 12px",
  borderTop: "1px solid rgba(255,255,255,0.06)",
  background: "rgba(0,0,0,0.18)",
  display: "flex",
  flexDirection: "column",
  gap: 0,
};

const timelineRailStyle: CSSProperties = {
  position: "absolute",
  left: 18,
  top: 16,
  bottom: 14,
  width: 1,
  background: "rgba(255,255,255,0.10)",
  pointerEvents: "none",
};

const timelineStepStyle: CSSProperties = {
  position: "relative",
  paddingLeft: 20,
  paddingTop: 4,
  paddingBottom: 4,
  minWidth: 0,
};

const timelineDotStyle: CSSProperties = {
  position: "absolute",
  left: 2,
  top: 9,
  width: 9,
  height: 9,
  borderRadius: 999,
  boxShadow: "inset 0 1px 0 rgba(255,255,255,0.45), 0 0 0 2.5px rgba(0,0,0,0.18)",
  flexShrink: 0,
};

const timelineStepHeaderStyle: CSSProperties = {
  width: "100%",
  display: "flex",
  alignItems: "center",
  gap: 8,
  padding: "3px 4px",
  background: "transparent",
  border: "none",
  color: "inherit",
  textAlign: "left",
  fontFamily: FONT,
  minWidth: 0,
};

const timelineStepNameStyle: CSSProperties = {
  fontFamily: "'DM Mono', ui-monospace, monospace",
  fontSize: 12,
  color: "rgba(255,255,255,0.85)",
  fontWeight: 500,
  flexShrink: 0,
};

const timelineStepArgsStyle: CSSProperties = {
  fontFamily: "'DM Mono', ui-monospace, monospace",
  fontSize: 11,
  color: "rgba(255,255,255,0.42)",
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
  flex: 1,
  minWidth: 0,
};

const timelineStepStatusStyle: CSSProperties = {
  fontSize: 10,
  letterSpacing: "0.10em",
  textTransform: "uppercase",
  fontFamily: FONT,
  flexShrink: 0,
};

const timelineStepDetailStyle: CSSProperties = {
  margin: "4px 0 6px 4px",
  padding: "8px 10px",
  borderRadius: 7,
  background: "rgba(0,0,0,0.32)",
  border: "1px solid rgba(255,255,255,0.05)",
  fontFamily: "'DM Mono', ui-monospace, monospace",
  fontSize: 11.5,
  lineHeight: 1.55,
  whiteSpace: "pre-wrap",
  wordBreak: "break-word",
  overflowWrap: "anywhere",
  maxHeight: 240,
  overflowY: "auto",
};

const timelineRunErrorStyle: CSSProperties = {
  marginTop: 6,
  padding: "6px 10px",
  borderRadius: 7,
  border: "1px solid rgba(220, 80, 80, 0.30)",
  background: "rgba(220, 80, 80, 0.10)",
  color: "rgba(255, 200, 200, 0.88)",
  fontSize: 11.5,
  fontFamily: FONT,
};

/* ── Thinking indicator (inline, no card) ─────────────────────────── */

const thinkingInlineStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 9,
  marginTop: 8,
  marginBottom: 2,
  paddingLeft: 1,
};

const thinkingGlyphStyle: CSSProperties = {
  fontFamily: "'DM Mono', ui-monospace, monospace",
  fontSize: 14,
  lineHeight: 1,
  color: "rgba(255,255,255,0.68)",
  width: 14,
  textAlign: "center",
  display: "inline-block",
};

const thinkingShimmerStyle: CSSProperties = {
  fontFamily: FONT,
  fontSize: 13.5,
  fontWeight: 500,
  letterSpacing: "0.005em",
  backgroundImage:
    "linear-gradient(90deg, rgba(255,255,255,0.32) 0%, rgba(255,255,255,0.32) 35%, rgba(255,255,255,1) 50%, rgba(255,255,255,0.32) 65%, rgba(255,255,255,0.32) 100%)",
  backgroundSize: "240% 100%",
  WebkitBackgroundClip: "text",
  backgroundClip: "text",
  WebkitTextFillColor: "transparent",
  color: "transparent",
  animation: "thinking-shimmer 2.4s ease-in-out infinite, word-in 280ms ease-out both",
  display: "inline-block",
};

const thinkingDoneGlyphStyle: CSSProperties = {
  fontFamily: "'DM Mono', ui-monospace, monospace",
  fontSize: 14,
  lineHeight: 1,
  color: "rgba(120, 220, 140, 0.85)",
  width: 14,
  textAlign: "center",
  display: "inline-block",
};

const thinkingDoneTextStyle: CSSProperties = {
  fontFamily: FONT,
  fontSize: 13,
  fontWeight: 500,
  letterSpacing: "0.005em",
  color: "rgba(255,255,255,0.55)",
};

const streamCursorStyle = {
  display: "inline-block",
  width: 6,
  height: 16,
  marginLeft: 3,
  marginBottom: -2,
  borderRadius: 999,
  background: "rgba(255,255,255,0.62)",
  animation: "cursor-pulse 820ms ease-in-out infinite",
};

const TOOL_KEYFRAMES = `
  @keyframes tool-spin { to { transform: rotate(360deg); } }
  @keyframes tool-progress {
    0% { transform: translateX(-100%); opacity: .35; }
    55% { opacity: 1; }
    100% { transform: translateX(180%); opacity: .35; }
  }
  @keyframes tool-card-in {
    from { opacity: 0; transform: translateY(4px) scale(.985); }
    to { opacity: 1; transform: translateY(0) scale(1); }
  }
  @keyframes cursor-pulse {
    0%, 100% { opacity: .25; }
    50% { opacity: 1; }
  }
  @keyframes tool-spin-pulse {
    0%, 100% { transform: scale(1); opacity: .9; }
    50% { transform: scale(1.6); opacity: .55; }
  }
  @keyframes thinking-shimmer {
    0%   { background-position: 240% 0; }
    100% { background-position: -140% 0; }
  }
  @keyframes word-in {
    from { opacity: 0; transform: translateY(1px); }
    to   { opacity: 1; transform: translateY(0); }
  }
`;

type ToolPart =
  | { kind: "text"; content: string }
  | { kind: "tool"; name: string; attrs: Record<string, string>; content: string }
  | { kind: "pending"; name: string; attrs: Record<string, string>; content: string };

function parseToolParts(content: string): ToolPart[] {
  const parts: ToolPart[] = [];
  let cursor = 0;

  while (true) {
    const start = content.indexOf("<", cursor);
    if (start === -1) break;
    const openEnd = content.indexOf(">", start);
    if (openEnd === -1) break;

    const parsed = parseOpenTag(content.slice(start + 1, openEnd));
    if (!parsed || !isToolTag(parsed.name)) {
      cursor = openEnd + 1;
      continue;
    }

    if (parsed.selfClosing) {
      if (start > cursor) {
        parts.push({ kind: "text", content: content.slice(cursor, start) });
      }
      parts.push({
        kind: "tool",
        name: parsed.name,
        attrs: parsed.attrs,
        content: "",
      });
      cursor = openEnd + 1;
      continue;
    }

    const closeTag = `</${parsed.name}>`;
    const closeStart = content.indexOf(closeTag, openEnd + 1);
    if (closeStart === -1) {
      if (start > cursor) {
        parts.push({ kind: "text", content: content.slice(cursor, start) });
      }
      parts.push({
        kind: "pending",
        name: parsed.name,
        attrs: parsed.attrs,
        content: content.slice(openEnd + 1),
      });
      cursor = content.length;
      break;
    }

    if (start > cursor) {
      parts.push({ kind: "text", content: content.slice(cursor, start) });
    }
    parts.push({
      kind: "tool",
      name: parsed.name,
      attrs: parsed.attrs,
      content: content.slice(openEnd + 1, closeStart),
    });
    cursor = closeStart + closeTag.length;
  }

  if (cursor < content.length) {
    parts.push({ kind: "text", content: content.slice(cursor) });
  }

  return parts.length > 0 ? parts : [{ kind: "text", content }];
}

function parseOpenTag(raw: string) {
  const open = raw.trim();
  if (!open || open.startsWith("/")) return null;
  const selfClosing = open.endsWith("/");
  const normalized = selfClosing ? open.slice(0, -1).trimEnd() : open;
  const match = normalized.match(/^([A-Za-z_][A-Za-z0-9_]*)([\s\S]*)$/);
  if (!match) return null;

  const attrs: Record<string, string> = {};
  const attrPattern = /([A-Za-z_][A-Za-z0-9_-]*)\s*=\s*"([^"]*)"/g;
  let attrMatch: RegExpExecArray | null;
  while ((attrMatch = attrPattern.exec(match[2])) !== null) {
    attrs[attrMatch[1]] = decodeXml(attrMatch[2]);
  }

  return { name: match[1], attrs, selfClosing };
}

function isToolTag(name: string) {
  return ["response", "python", "query", "end", "code", "html", "tool_result", "dothink", "search",
    "done",
    "createTodo", "completeTodo", "viewTodo", "listSkills", "readSkill",
    "CreateWorkspace", "WorkspaceStatus", "Command", "LongRunProcess",
    "CreateFile", "AppendFile", "PatchFile", "Preview", "ReadFile", "DeleteFile", "CreateDirectory", "DeleteDirectory"].includes(name);
}

function isExecutingTool(name: string) {
  return ["python", "query", "end", "done", "dothink", "search", "createTodo", "completeTodo", "viewTodo",
    "listSkills", "readSkill",
    "CreateWorkspace", "WorkspaceStatus", "Command", "LongRunProcess",
    "CreateFile", "AppendFile", "PatchFile", "Preview", "ReadFile", "DeleteFile", "CreateDirectory", "DeleteDirectory"].includes(name);
}

function streamingToolLabel(name: string) {
  switch (name) {
    case "code":
      return "preparing code";
    case "html":
      return "rendering html";
    case "python":
      return "executing python";
    case "query":
      return "searching conversation";
    case "end":
      return "ending conversation";
    case "dothink":
      return "ultra thinking";
    case "done":
      return "wrapping up";
    case "search":
      return "searching the web";
    case "createTodo":
      return "creating todo list";
    case "completeTodo":
      return "completing task";
    case "viewTodo":
      return "checking todo";
    case "listSkills":
      return "listing skills";
    case "readSkill":
      return "loading skill";
    case "CreateWorkspace":
      return "creating workspace";
    case "WorkspaceStatus":
      return "checking workspace";
    case "Command":
    case "LongRunProcess":
      return "running command";
    case "CreateFile":
      return "creating file";
    case "AppendFile":
      return "appending file";
    case "PatchFile":
      return "patching file";
    case "Preview":
      return "opening preview";
    case "ReadFile":
      return "reading file";
    case "DeleteFile":
      return "deleting file";
    case "CreateDirectory":
      return "creating directory";
    case "DeleteDirectory":
      return "deleting directory";
    default:
      return name;
  }
}

function HighlightedCodeBlock({
  code,
  language,
}: {
  code: string;
  language?: string;
}) {
  return (
    <Suspense fallback={<PlainCodeBlock code={code} />}>
      <LazyHighlightedCodeBlock code={code} language={language} />
    </Suspense>
  );
}

function PlainCodeBlock({ code }: { code: string }) {
  return (
    <pre
      style={{
        ...toolBlockStyle,
        margin: "8px 0 0",
        overflowX: "auto",
        whiteSpace: "pre-wrap",
        fontFamily: "'DM Mono', ui-monospace, monospace",
        fontSize: 12.5,
        color: "rgba(255,255,255,0.90)",
      }}
    >
      <code>{code}</code>
    </pre>
  );
}

function decodeXml(value: string) {
  return value
    .replace(/&quot;/g, '"')
    .replace(/&gt;/g, ">")
    .replace(/&lt;/g, "<")
    .replace(/&amp;/g, "&");
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

function isHomeData(value: unknown): value is HomeData {
  if (!value || typeof value !== "object") return false;
  const data = value as Partial<HomeData>;
  return Boolean(data.user && data.limits && Array.isArray(data.recent_chats));
}

function isChatSummary(value: unknown): value is ChatSummary {
  if (!value || typeof value !== "object") return false;
  const chat = value as Partial<ChatSummary>;
  return (
    typeof chat.id === "string" &&
    typeof chat.title === "string" &&
    typeof chat.preview === "string" &&
    typeof chat.updated_at === "string"
  );
}

function isChatMessage(value: unknown): value is ChatMessage {
  if (!value || typeof value !== "object") return false;
  const message = value as Partial<ChatMessage>;
  return (
    typeof message.id === "string" &&
    (message.role === "user" || message.role === "assistant") &&
    typeof message.content === "string" &&
    typeof message.created_at === "string"
  );
}

function isUsageData(value: unknown): value is UsageData {
  if (!value || typeof value !== "object") return false;
  const usage = value as Partial<UsageData>;
  return (
    typeof usage.messages === "number" &&
    typeof usage.tokens === "number" &&
    typeof usage.last_updated === "string"
  );
}

function isLimitsData(value: unknown): value is LimitsData {
  if (!value || typeof value !== "object") return false;
  const limits = value as Partial<LimitsData>;
  return (
    typeof limits.messages === "number" &&
    typeof limits.tokens === "number" &&
    typeof limits.resets_at === "string"
  );
}

function isChatPayload(value: unknown): value is { chat: ChatSummary; messages: ChatMessage[] } {
  if (!value || typeof value !== "object") return false;
  const payload = value as { chat?: unknown; messages?: unknown };
  return isChatSummary(payload.chat) && Array.isArray(payload.messages) && payload.messages.every(isChatMessage);
}

function isRunListPayload(value: unknown): value is { runs: RunSnapshot[] } {
  if (!value || typeof value !== "object") return false;
  const runs = (value as { runs?: unknown }).runs;
  return Array.isArray(runs);
}

function hydrateMessagesWithRuns(messages: ChatMessage[], runs: RunSnapshot[]): UiMessage[] {
  const out: UiMessage[] = messages.map((message) => ({ ...message }));
  const byAssistantId = new Map<string, RunSnapshot>();
  for (const run of runs) {
    if (run.assistant_message_uuid) {
      byAssistantId.set(run.assistant_message_uuid, run);
    }
  }

  for (let i = 0; i < out.length; i += 1) {
    const message = out[i];
    if (message.role !== "assistant") continue;
    const run = byAssistantId.get(message.id);
    if (!run) continue;
    out[i] = {
      ...message,
      runUuid: run.uuid,
      toolCalls: replayToolCalls(run.tool_calls ?? [], run.events ?? []),
      runStatus: run.status,
      runError: run.error_summary ?? undefined,
    };
  }

  for (const run of runs) {
    if (run.assistant_message_uuid || (run.status !== "running" && run.status !== "cancelled" && run.status !== "failed")) continue;
    const userIndex = out.findIndex((message) => message.id === run.user_message_uuid);
    const placeholder: UiMessage = {
      id: `run-${run.uuid}`,
      role: "assistant",
      content: run.final_text ?? (run.status === "cancelled" ? "Stopped by user" : ""),
      created_at: new Date(run.started_at * 1000).toISOString(),
      model: run.model,
      isStreaming: run.status === "running",
      runUuid: run.uuid,
      toolCalls: replayToolCalls(run.tool_calls ?? [], run.events ?? []),
      runStatus: run.status,
      runError: run.error_summary ?? undefined,
    };
    if (userIndex === -1) {
      out.push(placeholder);
    } else {
      out.splice(userIndex + 1, 0, placeholder);
    }
  }

  return out;
}

function replayToolCalls(calls: AgentToolCall[], events: AgentEventEnvelope[]) {
  const next = calls.map((call) => ({ ...call }));
  for (const event of events) {
    const payload = event.payload;
    const uuid = typeof payload.uuid === "string" ? payload.uuid : "";
    if (!uuid) continue;
    const idx = next.findIndex((call) => call.uuid === uuid);
    if (idx < 0) continue;

    if (event.type === "tool_output") {
      const chunk = typeof payload.chunk === "string" ? payload.chunk : "";
      if (chunk) {
        next[idx] = { ...next[idx], output: `${next[idx].output ?? ""}${chunk}` };
      }
    }

    if (event.type === "tool_succeeded" && typeof payload.result_xml === "string") {
      next[idx] = { ...next[idx], resultXml: payload.result_xml };
    }
  }
  return next;
}

function isStreamMetaPayload(
  value: unknown,
): value is {
  chat: ChatSummary;
  user_message: ChatMessage;
  usage: UsageData;
  limits: LimitsData;
  model?: string;
} {
  if (!value || typeof value !== "object") return false;
  const payload = value as {
    chat?: unknown;
    user_message?: unknown;
    usage?: unknown;
    limits?: unknown;
  };
  return (
    isChatSummary(payload.chat) &&
    isChatMessage(payload.user_message) &&
    isUsageData(payload.usage) &&
    isLimitsData(payload.limits)
  );
}

function isDeltaPayload(value: unknown): value is { text: string } {
  return Boolean(value && typeof value === "object" && typeof (value as { text?: unknown }).text === "string");
}

function isErrorPayload(value: unknown): value is { error: string } {
  return Boolean(value && typeof value === "object" && typeof (value as { error?: unknown }).error === "string");
}

function isToolLifecyclePayload(value: unknown): value is { tools: string[] } {
  if (!value || typeof value !== "object") return false;
  const payload = value as { tools?: unknown };
  return Array.isArray(payload.tools) && payload.tools.every((tool) => typeof tool === "string");
}

function isReplacePayload(value: unknown): value is { text: string } {
  return Boolean(value && typeof value === "object" && typeof (value as { text?: unknown }).text === "string");
}

function isStreamDonePayload(value: unknown): value is { assistant_message?: ChatMessage | null } {
  if (!value || typeof value !== "object") return false;
  const payload = value as { assistant_message?: unknown };
  return (
    payload.assistant_message === undefined ||
    payload.assistant_message === null ||
    isChatMessage(payload.assistant_message)
  );
}

/* ── Markdown rendering for assistant messages ──────────────────────────────
   react-markdown + remark-gfm gives us GitHub-flavored markdown: bold/italic,
   inline + fenced code, lists, links, tables, strikethrough. Each element is
   styled to match the liquid-glass aesthetic — code blocks get a darker glass
   panel, inline code gets a subtle chip, links inherit the warm Chillax cream.
   react-markdown handles partial/incomplete markdown gracefully during a
   stream (an unclosed `**bold` just renders as plain text until it closes). */

function Markdown({ content }: { content: string }) {
  return (
    <div style={{ display: "contents" }}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={MD_COMPONENTS}>
        {content}
      </ReactMarkdown>
    </div>
  );
}

const MD_COMPONENTS: Components = {
  p: ({ children }) => (
    <p style={{ margin: "0.45em 0", lineHeight: 1.6 }}>{children}</p>
  ),
  strong: ({ children }) => (
    <strong style={{ fontWeight: 600, color: "rgba(255,255,255,0.98)" }}>
      {children}
    </strong>
  ),
  em: ({ children }) => (
    <em style={{ fontStyle: "italic", color: "rgba(255,255,255,0.92)" }}>
      {children}
    </em>
  ),
  del: ({ children }) => (
    <del style={{ color: "rgba(255,255,255,0.50)" }}>{children}</del>
  ),
  a: ({ children, href }) => (
    <a
      href={href}
      target="_blank"
      rel="noopener noreferrer"
      style={{
        color: "#FFFAEE",
        textDecoration: "underline",
        textDecorationColor: "rgba(255,255,255,0.30)",
        textUnderlineOffset: 2,
      }}
    >
      {children}
    </a>
  ),
  code: ({ children, className }) => {
    // react-markdown distinguishes block vs inline by whether there's a language class
    const isBlock = Boolean(className?.startsWith("language-"));
    if (isBlock) {
      const language = className?.replace(/^language-/, "");
      const source = String(children).replace(/\n$/, "");
      if (isMermaidLanguage(language)) {
        return (
          <Suspense fallback={<PlainCodeBlock code={source} />}>
            <LazyMermaidBlock code={source} />
          </Suspense>
        );
      }
      return <HighlightedCodeBlock code={source} language={language} />;
    }
    return (
      <code
        style={{
          fontFamily: "'DM Mono', ui-monospace, monospace",
          fontSize: "0.9em",
          padding: "1px 6px",
          borderRadius: 5,
          background: "rgba(0,0,0,0.40)",
          border: "1px solid rgba(255,255,255,0.08)",
          color: "#FFFAEE",
          wordBreak: "break-word",
        }}
      >
        {children}
      </code>
    );
  },
  pre: ({ children }) => <>{children}</>,
  ul: ({ children }) => (
    <ul style={{ margin: "0.4em 0", paddingLeft: "1.4em" }}>{children}</ul>
  ),
  ol: ({ children }) => (
    <ol style={{ margin: "0.4em 0", paddingLeft: "1.4em" }}>{children}</ol>
  ),
  li: ({ children }) => (
    <li style={{ margin: "0.2em 0", lineHeight: 1.55 }}>{children}</li>
  ),
  h1: ({ children }) => (
    <h1
      style={{
        fontSize: "1.25em",
        fontWeight: 600,
        margin: "0.7em 0 0.3em",
        color: "#FFFAEE",
        letterSpacing: "-0.005em",
      }}
    >
      {children}
    </h1>
  ),
  h2: ({ children }) => (
    <h2
      style={{
        fontSize: "1.15em",
        fontWeight: 600,
        margin: "0.65em 0 0.3em",
        color: "#FFFAEE",
        letterSpacing: "-0.005em",
      }}
    >
      {children}
    </h2>
  ),
  h3: ({ children }) => (
    <h3
      style={{
        fontSize: "1.05em",
        fontWeight: 600,
        margin: "0.6em 0 0.3em",
        color: "#FFFAEE",
      }}
    >
      {children}
    </h3>
  ),
  hr: () => (
    <hr
      style={{
        border: "none",
        borderTop: "1px solid rgba(255,255,255,0.10)",
        margin: "0.8em 0",
      }}
    />
  ),
  blockquote: ({ children }) => (
    <blockquote
      style={{
        margin: "0.5em 0",
        paddingLeft: 12,
        borderLeft: "2px solid rgba(255,255,255,0.20)",
        color: "rgba(255,255,255,0.75)",
      }}
    >
      {children}
    </blockquote>
  ),
  table: ({ children }) => (
    <div style={{ overflowX: "auto", margin: "0.55em 0" }}>
      <table
        style={{
          borderCollapse: "collapse",
          fontSize: "0.95em",
          minWidth: "100%",
        }}
      >
        {children}
      </table>
    </div>
  ),
  th: ({ children }) => (
    <th
      style={{
        textAlign: "left",
        padding: "6px 10px",
        borderBottom: "1px solid rgba(255,255,255,0.22)",
        fontWeight: 600,
        color: "rgba(255,255,255,0.94)",
      }}
    >
      {children}
    </th>
  ),
  td: ({ children }) => (
    <td
      style={{
        padding: "6px 10px",
        borderBottom: "1px solid rgba(255,255,255,0.06)",
      }}
    >
      {children}
    </td>
  ),
};
