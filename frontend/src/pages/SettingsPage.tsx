import { type CSSProperties, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { ArrowLeft, Check, ChevronDown, Trash2 } from "lucide-react";
import {
  FONT,
  IconBtn,
  KEYFRAMES,
  innerGlass,
  outerGlass,
  pillGlass,
  smallPillGlass,
} from "../lib/glass";
import AppBackground from "../components/AppBackground";

interface Settings {
  base_style: string;
  characteristics: Record<string, string>;
  custom_instructions: string;
}

interface VultrConnectionStatus {
  status: "connected" | "missing" | "invalid" | "rate_limited" | "server_misconfigured";
  connected: boolean;
  label?: string | null;
  api_key_last4?: string | null;
  verified_at?: number | null;
  last_error?: string | null;
  api_access_url: string;
}

const BASE_STYLES: { value: string; label: string; hint: string }[] = [
  { value: "default", label: "Default", hint: "Balanced and adaptive" },
  { value: "concise", label: "Concise", hint: "Short, direct sentences" },
  { value: "detailed", label: "Detailed", hint: "Thorough explanations" },
  { value: "friendly", label: "Friendly", hint: "Warm and conversational" },
  { value: "professional", label: "Professional", hint: "Polished, business register" },
  { value: "witty", label: "Witty", hint: "Light wordplay, dry humor" },
  { value: "direct", label: "Direct", hint: "Blunt, no hedging" },
];

const CHARACTERISTICS: { key: string; label: string }[] = [
  { key: "warmth", label: "Warmth" },
  { key: "enthusiasm", label: "Enthusiasm" },
  { key: "headers_lists", label: "Headers & Lists" },
  { key: "emoji", label: "Emoji" },
  { key: "verbosity", label: "Verbosity" },
  { key: "technicality", label: "Technical depth" },
];

const PREF_LEVELS = ["less", "default", "more"];

const MAX_INSTRUCTIONS = 2000;
const MAX_NAME_LEN = 80;

export default function SettingsPage() {
  const navigate = useNavigate();
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedAt, setSavedAt] = useState<number | null>(null);

  const [userName, setUserName] = useState("");
  const [userEmail, setUserEmail] = useState("");
  const [originalName, setOriginalName] = useState("");

  const [settings, setSettings] = useState<Settings>({
    base_style: "default",
    characteristics: {},
    custom_instructions: "",
  });
  const [originalSettings, setOriginalSettings] = useState<Settings>(settings);

  const [confirmingChats, setConfirmingChats] = useState(false);
  const [confirmingWorkspaces, setConfirmingWorkspaces] = useState(false);
  const [destructive, setDestructive] = useState<string | null>(null);
  const [vultrConnection, setVultrConnection] = useState<VultrConnectionStatus | null>(null);
  const [vultrApiKey, setVultrApiKey] = useState("");
  const [vultrLabel, setVultrLabel] = useState("");
  const [vultrBusy, setVultrBusy] = useState<null | "save" | "delete" | "verify">(null);

  useEffect(() => {
    let cancelled = false;
    void fetch("/api/settings", { credentials: "same-origin" })
      .then(async (res) => {
        const body = await res.json().catch(() => ({}));
        if (cancelled) return;
        if (!res.ok || !body.ok) {
          setError(body.error || "Could not load settings");
          return;
        }
        const next: Settings = {
          base_style: body.settings?.base_style ?? "default",
          characteristics: (body.settings?.characteristics ?? {}) as Record<string, string>,
          custom_instructions: body.settings?.custom_instructions ?? "",
        };
        setSettings(next);
        setOriginalSettings(next);
        setUserName(body.user?.name ?? "");
        setOriginalName(body.user?.name ?? "");
        setUserEmail(body.user?.email ?? "");
      })
      .catch((err: Error) => {
        if (!cancelled) setError(err.message);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    void fetch("/api/integrations/vultr", { credentials: "same-origin" })
      .then(async (res) => {
        const body = await res.json().catch(() => ({}));
        if (cancelled) return;
        if (!res.ok || !body.ok) return;
        setVultrConnection(body.connection ?? null);
        setVultrLabel(body.connection?.label ?? "");
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  const settingsDirty = useMemo(() => {
    return (
      settings.base_style !== originalSettings.base_style ||
      settings.custom_instructions !== originalSettings.custom_instructions ||
      JSON.stringify(settings.characteristics) !== JSON.stringify(originalSettings.characteristics)
    );
  }, [settings, originalSettings]);

  const nameDirty = userName.trim() !== originalName.trim() && userName.trim().length > 0;

  async function saveAll() {
    setSaving(true);
    setError(null);
    try {
      if (nameDirty) {
        const res = await fetch("/api/user/name", {
          method: "PATCH",
          credentials: "same-origin",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ name: userName.trim() }),
        });
        const body = await res.json().catch(() => ({}));
        if (!res.ok || !body.ok) throw new Error(body.error || "Could not update name");
        setOriginalName(userName.trim());
      }

      if (settingsDirty) {
        const res = await fetch("/api/settings", {
          method: "PUT",
          credentials: "same-origin",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(settings),
        });
        const body = await res.json().catch(() => ({}));
        if (!res.ok || !body.ok) throw new Error(body.error || "Could not save personalization");
        setOriginalSettings(settings);
      }

      setSavedAt(Date.now());
    } catch (err) {
      setError(err instanceof Error ? err.message : "Save failed");
    } finally {
      setSaving(false);
    }
  }

  async function deleteAllChats() {
    setDestructive("chats");
    setError(null);
    try {
      const res = await fetch("/api/chats", { method: "DELETE", credentials: "same-origin" });
      const body = await res.json().catch(() => ({}));
      if (!res.ok || !body.ok) throw new Error(body.error || "Could not delete chats");
      setConfirmingChats(false);
      setSavedAt(Date.now());
    } catch (err) {
      setError(err instanceof Error ? err.message : "Delete failed");
    } finally {
      setDestructive(null);
    }
  }

  async function deleteAllWorkspaces() {
    setDestructive("workspaces");
    setError(null);
    try {
      const res = await fetch("/api/workspaces", { method: "DELETE", credentials: "same-origin" });
      const body = await res.json().catch(() => ({}));
      if (!res.ok || !body.ok) throw new Error(body.error || "Could not delete workspaces");
      setConfirmingWorkspaces(false);
      setSavedAt(Date.now());
    } catch (err) {
      setError(err instanceof Error ? err.message : "Delete failed");
    } finally {
      setDestructive(null);
    }
  }

  async function connectVultr() {
    setVultrBusy("save");
    setError(null);
    try {
      const res = await fetch("/api/integrations/vultr", {
        method: "PUT",
        credentials: "same-origin",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          api_key: vultrApiKey,
          label: vultrLabel,
        }),
      });
      const body = await res.json().catch(() => ({}));
      if (!res.ok || !body.ok) throw new Error(body.error || "Could not connect Vultr");
      setVultrConnection(body.connection ?? null);
      setVultrLabel(body.connection?.label ?? "");
      setVultrApiKey("");
      setSavedAt(Date.now());
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not connect Vultr");
    } finally {
      setVultrBusy(null);
    }
  }

  async function verifyVultr() {
    setVultrBusy("verify");
    setError(null);
    try {
      const res = await fetch("/api/integrations/vultr/verify", {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({}),
      });
      const body = await res.json().catch(() => ({}));
      if (!res.ok || !body.ok) throw new Error(body.error || "Could not verify Vultr");
      const refreshed = await fetch("/api/integrations/vultr", { credentials: "same-origin" });
      const refreshedBody = await refreshed.json().catch(() => ({}));
      if (refreshed.ok && refreshedBody.ok) {
        setVultrConnection(refreshedBody.connection ?? null);
      }
      setSavedAt(Date.now());
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not verify Vultr");
    } finally {
      setVultrBusy(null);
    }
  }

  async function disconnectVultr() {
    setVultrBusy("delete");
    setError(null);
    try {
      const res = await fetch("/api/integrations/vultr", {
        method: "DELETE",
        credentials: "same-origin",
      });
      const body = await res.json().catch(() => ({}));
      if (!res.ok || !body.ok) throw new Error(body.error || "Could not disconnect Vultr");
      setVultrConnection({
        status: "missing",
        connected: false,
        api_access_url: "https://console.vultr.com/user/apiaccess/",
      });
      setVultrApiKey("");
      setSavedAt(Date.now());
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not disconnect Vultr");
    } finally {
      setVultrBusy(null);
    }
  }

  function setCharacteristic(key: string, value: string) {
    setSettings((s) => {
      const next = { ...s.characteristics };
      if (value === "default") delete next[key];
      else next[key] = value;
      return { ...s, characteristics: next };
    });
  }

  const dirty = nameDirty || settingsDirty;

  return (
    <div style={pageStyle}>
      <style>{KEYFRAMES}</style>
      <AppBackground />
      <div aria-hidden style={vignetteStyle} />

      <div style={contentWrapStyle}>
        {/* Top bar */}
        <header style={topBarStyle}>
          <IconBtn title="Back" onClick={() => navigate(-1)}>
            <ArrowLeft size={15} strokeWidth={1.8} />
          </IconBtn>
          <h1 style={titleStyle}>Settings</h1>
          <span style={{ flex: 1 }} />
          {savedAt && !dirty && (
            <span style={{ ...smallPillGlass, padding: "5px 11px", borderRadius: 999, fontSize: 11, color: "rgba(120, 220, 140, 0.92)" }}>
              <Check size={11} strokeWidth={2} style={{ marginRight: 5, marginBottom: -1 }} />
              Saved
            </span>
          )}
          <button
            type="button"
            onClick={() => void saveAll()}
            disabled={!dirty || saving}
            style={{
              ...pillGlass,
              padding: "8px 16px",
              borderRadius: 999,
              border: "none",
              color: dirty ? "rgba(255,255,255,0.92)" : "rgba(255,255,255,0.42)",
              fontFamily: FONT,
              fontSize: 13,
              fontWeight: 500,
              cursor: dirty && !saving ? "pointer" : "default",
            }}
          >
            {saving ? "Saving…" : "Save changes"}
          </button>
        </header>

        <main style={mainStyle}>
          {loading ? (
            <div style={{ padding: 32, color: "rgba(255,255,255,0.50)", fontSize: 13 }}>Loading…</div>
          ) : (
            <>
              {/* PROFILE */}
              <Section title="Profile" hint="How you appear inside Mixer.">
                <Field label="Display name">
                  <input
                    type="text"
                    value={userName}
                    maxLength={MAX_NAME_LEN}
                    onChange={(e) => setUserName(e.target.value)}
                    style={inputStyle}
                  />
                </Field>
                <Field label="Email">
                  <input
                    type="text"
                    value={userEmail}
                    readOnly
                    style={{ ...inputStyle, opacity: 0.55, cursor: "not-allowed" }}
                  />
                </Field>
              </Section>

              {/* PERSONALIZATION */}
              <Section
                title="Personalization"
                hint="Tunes how Mixer responds. Applied to every reply across every chat."
              >
                <Field label="Base style and tone" hint="The overall register Mixer adopts.">
                  <Select
                    value={settings.base_style}
                    onChange={(v) => setSettings((s) => ({ ...s, base_style: v }))}
                    options={BASE_STYLES.map((s) => ({
                      value: s.value,
                      label: s.label,
                      hint: s.hint,
                    }))}
                  />
                </Field>

                <div style={charsHeaderStyle}>
                  <span style={charsTitleStyle}>Characteristics</span>
                  <span style={charsHintStyle}>
                    Layer on top of the base style. "Default" leaves it untouched.
                  </span>
                </div>

                <div style={charsGridStyle}>
                  {CHARACTERISTICS.map(({ key, label }) => {
                    const current = settings.characteristics[key] ?? "default";
                    return (
                      <div key={key} style={charsRowStyle}>
                        <span style={charsRowLabelStyle}>{label}</span>
                        <Select
                          compact
                          value={current}
                          onChange={(v) => setCharacteristic(key, v)}
                          options={PREF_LEVELS.map((level) => ({
                            value: level,
                            label: level.charAt(0).toUpperCase() + level.slice(1),
                          }))}
                        />
                      </div>
                    );
                  })}
                </div>

                <Field
                  label="Custom instructions"
                  hint={`Any extra behavior, style, or tone preferences. ${settings.custom_instructions.length}/${MAX_INSTRUCTIONS}`}
                >
                  <textarea
                    value={settings.custom_instructions}
                    maxLength={MAX_INSTRUCTIONS}
                    onChange={(e) =>
                      setSettings((s) => ({ ...s, custom_instructions: e.target.value }))
                    }
                    placeholder="e.g. always include a short summary at the top, prefer Bun over Node, never write JSON in code fences..."
                    rows={5}
                    style={textareaStyle}
                  />
                </Field>
              </Section>

              <Section
                title="Vultr"
                hint="Connect your Vultr account so Mixer can list, deploy, inspect, and operate cloud infrastructure directly."
              >
                <Field label="Connection status">
                  <div style={{ ...innerGlass, padding: 14, color: "rgba(255,255,255,0.82)", fontSize: 13.5 }}>
                    <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
                      <span
                        style={{
                          ...smallPillGlass,
                          padding: "5px 10px",
                          borderRadius: 999,
                          color:
                            vultrConnection?.status === "connected"
                              ? "rgba(120,220,140,0.92)"
                              : vultrConnection?.status === "rate_limited"
                                ? "rgba(255,210,120,0.92)"
                                : "rgba(255,150,150,0.92)",
                        }}
                      >
                        {vultrConnection?.connected
                          ? "Connected"
                          : vultrConnection?.status === "missing"
                            ? "Not connected"
                            : vultrConnection?.status === "rate_limited"
                              ? "Rate limited"
                              : "Invalid or unavailable"}
                      </span>
                      {vultrConnection?.api_key_last4 && (
                        <span style={{ color: "rgba(255,255,255,0.58)" }}>
                          API key ending in {vultrConnection.api_key_last4}
                        </span>
                      )}
                    </div>
                    {vultrConnection?.last_error && (
                      <div style={{ marginTop: 10, color: "rgba(255,210,120,0.92)", lineHeight: 1.5 }}>
                        {vultrConnection.last_error}
                      </div>
                    )}
                  </div>
                </Field>

                <Field label="Account label" hint="Optional nickname for this Vultr account inside Mixer.">
                  <input
                    type="text"
                    value={vultrLabel}
                    onChange={(e) => setVultrLabel(e.target.value)}
                    style={inputStyle}
                  />
                </Field>

                <Field
                  label="Vultr API key"
                  hint={
                    vultrConnection?.connected
                      ? "Paste a new key only when rotating credentials."
                      : "Create the key in Vultr API Access, then paste it here."
                  }
                >
                  <textarea
                    value={vultrApiKey}
                    onChange={(e) => setVultrApiKey(e.target.value)}
                    placeholder="Paste your Vultr API key"
                    rows={3}
                    style={textareaStyle}
                  />
                </Field>

                <div style={{ display: "flex", gap: 10, flexWrap: "wrap" }}>
                  <button
                    type="button"
                    onClick={() => void connectVultr()}
                    disabled={
                      vultrBusy !== null ||
                      (vultrApiKey.trim().length === 0 &&
                        !(vultrConnection?.connected && vultrLabel.trim() !== (vultrConnection.label ?? "").trim()))
                    }
                    style={settingsActionButtonStyle}
                  >
                    {vultrBusy === "save"
                      ? "Saving…"
                      : vultrConnection?.connected
                        ? vultrApiKey.trim().length > 0
                          ? "Rotate key"
                          : "Update label"
                        : "Connect Vultr"}
                  </button>
                  <button
                    type="button"
                    onClick={() => void verifyVultr()}
                    disabled={vultrBusy !== null || !vultrConnection?.connected}
                    style={secondaryActionButtonStyle}
                  >
                    {vultrBusy === "verify" ? "Verifying…" : "Verify connection"}
                  </button>
                  <button
                    type="button"
                    onClick={() => void disconnectVultr()}
                    disabled={vultrBusy !== null || !vultrConnection?.connected}
                    style={dangerActionButtonStyle}
                  >
                    {vultrBusy === "delete" ? "Disconnecting…" : "Disconnect"}
                  </button>
                </div>

                <div style={{ color: "rgba(255,255,255,0.54)", fontSize: 12.5, lineHeight: 1.6 }}>
                  Vultr API keys are effectively full-account credentials. Generate them in{" "}
                  <a
                    href={vultrConnection?.api_access_url ?? "https://console.vultr.com/user/apiaccess/"}
                    target="_blank"
                    rel="noreferrer"
                    style={{ color: "rgba(180,220,255,0.94)" }}
                  >
                    Vultr API Access
                  </a>
                  . Mixer stores them encrypted server-side and never returns the raw key in chat or API responses.
                </div>
              </Section>

              {/* DANGER ZONE */}
              <Section
                title="Danger zone"
                hint="These actions are permanent. Take a deep breath first."
                tone="danger"
              >
                <DangerRow
                  title="Delete all chats"
                  description="Permanently removes every conversation, message, and agent run on this account."
                  buttonLabel={confirmingChats ? "Confirm delete" : "Delete chats"}
                  busy={destructive === "chats"}
                  confirming={confirmingChats}
                  onConfirm={() => setConfirmingChats(true)}
                  onCancel={() => setConfirmingChats(false)}
                  onAction={() => void deleteAllChats()}
                />
                <DangerRow
                  title="Delete all workspace files"
                  description="Removes every sandbox project the agent has built for you, including all files inside them."
                  buttonLabel={confirmingWorkspaces ? "Confirm delete" : "Delete workspaces"}
                  busy={destructive === "workspaces"}
                  confirming={confirmingWorkspaces}
                  onConfirm={() => setConfirmingWorkspaces(true)}
                  onCancel={() => setConfirmingWorkspaces(false)}
                  onAction={() => void deleteAllWorkspaces()}
                />
              </Section>

              {error && <div style={errorStyle}>{error}</div>}
            </>
          )}
        </main>
      </div>
    </div>
  );
}

/* ─────────────────────────────────────────────────────────────────── */
/*  Sub-components                                                      */
/* ─────────────────────────────────────────────────────────────────── */

function Section({
  title,
  hint,
  tone,
  children,
}: {
  title: string;
  hint?: string;
  tone?: "danger";
  children: React.ReactNode;
}) {
  return (
    <section
      style={{
        ...outerGlass,
        padding: 22,
        marginBottom: 16,
        ...(tone === "danger"
          ? { border: "1px solid rgba(255, 110, 110, 0.18)", borderTop: "1px solid rgba(255, 130, 130, 0.32)" }
          : {}),
      }}
    >
      <div style={sectionHeaderStyle}>
        <h2 style={{ ...sectionTitleStyle, color: tone === "danger" ? "rgba(255, 170, 170, 0.95)" : sectionTitleStyle.color }}>
          {title}
        </h2>
        {hint && <p style={sectionHintStyle}>{hint}</p>}
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>{children}</div>
    </section>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div style={fieldWrapStyle}>
      <label style={fieldLabelStyle}>{label}</label>
      {children}
      {hint && <span style={fieldHintStyle}>{hint}</span>}
    </div>
  );
}

function Select({
  value,
  onChange,
  options,
  compact,
}: {
  value: string;
  onChange: (v: string) => void;
  options: { value: string; label: string; hint?: string }[];
  compact?: boolean;
}) {
  const current = options.find((o) => o.value === value) ?? options[0];
  return (
    <div style={{ position: "relative", display: "inline-block", width: compact ? 130 : "100%" }}>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        style={compact ? selectCompactStyle : selectStyle}
      >
        {options.map((opt) => (
          <option key={opt.value} value={opt.value} style={{ background: "rgba(20,20,24,1)", color: "rgba(255,255,255,0.92)" }}>
            {opt.label}
          </option>
        ))}
      </select>
      <ChevronDown
        size={13}
        style={{
          position: "absolute",
          right: 12,
          top: "50%",
          transform: "translateY(-50%)",
          color: "rgba(255,255,255,0.50)",
          pointerEvents: "none",
        }}
      />
      {!compact && current?.hint && (
        <span style={{ ...fieldHintStyle, display: "block", marginTop: 6 }}>{current.hint}</span>
      )}
    </div>
  );
}

function DangerRow({
  title,
  description,
  buttonLabel,
  busy,
  confirming,
  onConfirm,
  onCancel,
  onAction,
}: {
  title: string;
  description: string;
  buttonLabel: string;
  busy: boolean;
  confirming: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  onAction: () => void;
}) {
  return (
    <div style={dangerRowStyle}>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={dangerTitleStyle}>{title}</div>
        <div style={dangerDescStyle}>{description}</div>
      </div>
      <div style={{ display: "flex", gap: 6, flexShrink: 0 }}>
        {confirming && (
          <button
            type="button"
            onClick={onCancel}
            disabled={busy}
            style={{
              ...smallPillGlass,
              padding: "6px 12px",
              borderRadius: 999,
              border: "none",
              color: "rgba(255,255,255,0.72)",
              fontFamily: FONT,
              fontSize: 12,
              cursor: "pointer",
            }}
          >
            Cancel
          </button>
        )}
        <button
          type="button"
          onClick={confirming ? onAction : onConfirm}
          disabled={busy}
          style={{
            ...pillGlass,
            padding: "7px 14px",
            borderRadius: 999,
            border: "none",
            color: confirming ? "rgba(255, 170, 170, 0.98)" : "rgba(255, 170, 170, 0.88)",
            fontFamily: FONT,
            fontSize: 12.5,
            fontWeight: 500,
            cursor: busy ? "default" : "pointer",
            display: "inline-flex",
            alignItems: "center",
            gap: 6,
          }}
        >
          {!confirming && <Trash2 size={12} strokeWidth={1.9} />}
          {busy ? "Deleting…" : buttonLabel}
        </button>
      </div>
    </div>
  );
}

/* ─────────────────────────────────────────────────────────────────── */
/*  Styles                                                              */
/* ─────────────────────────────────────────────────────────────────── */

const pageStyle: CSSProperties = {
  position: "relative",
  width: "100vw",
  minHeight: "100vh",
  overflowX: "hidden",
  fontFamily: FONT,
  color: "rgba(255,255,255,0.92)",
};

const vignetteStyle: CSSProperties = {
  position: "absolute",
  inset: 0,
  zIndex: 1,
  background:
    "radial-gradient(ellipse 100% 100% at 50% 50%, transparent 40%, rgba(0,0,0,0.38) 100%)",
  pointerEvents: "none",
};

const contentWrapStyle: CSSProperties = {
  position: "relative",
  zIndex: 2,
  width: "100%",
  minHeight: "100vh",
  display: "flex",
  flexDirection: "column",
  alignItems: "center",
  padding: "0 20px 60px",
};

const topBarStyle: CSSProperties = {
  width: "min(780px, 95vw)",
  display: "flex",
  alignItems: "center",
  gap: 12,
  padding: "18px 4px 22px",
};

const titleStyle: CSSProperties = {
  margin: 0,
  fontFamily: FONT,
  fontSize: 22,
  fontWeight: 600,
  letterSpacing: "-0.01em",
  color: "rgba(255,255,255,0.95)",
};

const mainStyle: CSSProperties = {
  width: "min(780px, 95vw)",
  display: "flex",
  flexDirection: "column",
};

const sectionHeaderStyle: CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 4,
  marginBottom: 18,
  paddingBottom: 14,
  borderBottom: "1px solid rgba(255,255,255,0.06)",
};

const sectionTitleStyle: CSSProperties = {
  margin: 0,
  fontFamily: FONT,
  fontSize: 16,
  fontWeight: 600,
  letterSpacing: "-0.005em",
  color: "rgba(255,255,255,0.92)",
};

const sectionHintStyle: CSSProperties = {
  margin: 0,
  fontSize: 12.5,
  color: "rgba(255,255,255,0.48)",
  lineHeight: 1.5,
};

const fieldWrapStyle: CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 8,
};

const fieldLabelStyle: CSSProperties = {
  fontSize: 12.5,
  fontWeight: 500,
  color: "rgba(255,255,255,0.78)",
  letterSpacing: "0.005em",
};

const fieldHintStyle: CSSProperties = {
  fontSize: 11.5,
  color: "rgba(255,255,255,0.42)",
  lineHeight: 1.5,
};

const inputStyle: CSSProperties = {
  ...innerGlass,
  width: "100%",
  padding: "10px 13px",
  border: "1px solid rgba(255,255,255,0.08)",
  borderRadius: 10,
  background: "rgba(0,0,0,0.30)",
  color: "rgba(255,255,255,0.92)",
  fontFamily: FONT,
  fontSize: 13.5,
  outline: "none",
};

const textareaStyle: CSSProperties = {
  ...inputStyle,
  resize: "vertical",
  minHeight: 90,
  lineHeight: 1.55,
};

const selectStyle: CSSProperties = {
  ...inputStyle,
  appearance: "none",
  paddingRight: 32,
  cursor: "pointer",
};

const selectCompactStyle: CSSProperties = {
  ...selectStyle,
  padding: "7px 30px 7px 11px",
  fontSize: 12.5,
};

const charsHeaderStyle: CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 2,
  marginTop: 4,
  paddingTop: 4,
};

const charsTitleStyle: CSSProperties = {
  fontSize: 12.5,
  fontWeight: 500,
  color: "rgba(255,255,255,0.78)",
};

const charsHintStyle: CSSProperties = {
  fontSize: 11.5,
  color: "rgba(255,255,255,0.42)",
};

const charsGridStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "repeat(auto-fit, minmax(220px, 1fr))",
  gap: 8,
  marginTop: 4,
};

const charsRowStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  padding: "8px 12px",
  background: "rgba(0,0,0,0.20)",
  border: "1px solid rgba(255,255,255,0.05)",
  borderRadius: 9,
  gap: 10,
};

const charsRowLabelStyle: CSSProperties = {
  fontSize: 12.5,
  color: "rgba(255,255,255,0.82)",
  fontWeight: 500,
};

const dangerRowStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: 16,
  padding: "12px 0",
};

const dangerTitleStyle: CSSProperties = {
  fontSize: 13,
  fontWeight: 500,
  color: "rgba(255,255,255,0.88)",
  marginBottom: 3,
};

const dangerDescStyle: CSSProperties = {
  fontSize: 11.5,
  color: "rgba(255,255,255,0.45)",
  lineHeight: 1.45,
  maxWidth: 480,
};

const settingsActionButtonStyle: CSSProperties = {
  ...pillGlass,
  padding: "9px 15px",
  borderRadius: 999,
  border: "none",
  color: "rgba(255,255,255,0.9)",
  fontFamily: FONT,
  fontSize: 13,
  cursor: "pointer",
};

const secondaryActionButtonStyle: CSSProperties = {
  ...smallPillGlass,
  padding: "9px 15px",
  borderRadius: 999,
  border: "1px solid rgba(255,255,255,0.08)",
  color: "rgba(255,255,255,0.84)",
  fontFamily: FONT,
  fontSize: 13,
  cursor: "pointer",
};

const dangerActionButtonStyle: CSSProperties = {
  ...smallPillGlass,
  padding: "9px 15px",
  borderRadius: 999,
  border: "1px solid rgba(255,130,130,0.18)",
  color: "rgba(255,170,170,0.94)",
  fontFamily: FONT,
  fontSize: 13,
  cursor: "pointer",
};

const errorStyle: CSSProperties = {
  padding: "10px 14px",
  borderRadius: 9,
  border: "1px solid rgba(220, 80, 80, 0.30)",
  background: "rgba(220, 80, 80, 0.10)",
  color: "rgba(255, 200, 200, 0.88)",
  fontSize: 12.5,
  marginTop: 8,
};
