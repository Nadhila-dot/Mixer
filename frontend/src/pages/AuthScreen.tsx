import {
  type CSSProperties,
  type FormEvent,
  useMemo,
  useState,
} from "react";
import { ArrowRight, Loader } from "lucide-react";
import { getSsrEnvelope } from "../lib/ssr";
import AppBackground from "../components/AppBackground";
import {
  FONT,
  KEYFRAMES,
  inputGlass,
  innerGlass,
  outerGlass,
  pillGlass,
  smallPillGlass,
} from "../lib/glass";
import type { AuthScreenData } from "../types";

/* ─────────────────────────────────────────────────────────────────────────── */
/*  Mixer chat — sign-in / register screen                                     */
/*                                                                             */
/*  Reuses the Home liquid-glass system and entrance choreography.             */
/*  Backend is unchanged: POSTs JSON to /api/auth/{login,register},            */
/*  forge_session cookie is set server-side, hard-nav to `next` on success.    */
/* ─────────────────────────────────────────────────────────────────────────── */

type Mode = "login" | "register";

const FALLBACK_CONFIG: AuthScreenData = {
  next: "/",
  loginEndpoint: "/api/auth/login",
  registerEndpoint: "/api/auth/register",
  sessionEndpoint: "/api/auth/me",
};

export default function AuthScreen() {
  const ssr = getSsrEnvelope<AuthScreenData>();
  const config = useMemo<AuthScreenData>(
    () => (ssr?.page === "auth-screen" && ssr.data ? ssr.data : FALLBACK_CONFIG),
    [ssr],
  );

  const [mode, setMode] = useState<Mode>("login");
  const [error, setError] = useState<string | null>(null);
  const [isSubmitting, setIsSubmitting] = useState(false);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (isSubmitting) return;
    setIsSubmitting(true);
    setError(null);

    const form = new FormData(event.currentTarget);
    const endpoint =
      mode === "login" ? config.loginEndpoint : config.registerEndpoint;
    const body = Object.fromEntries(form.entries());

    try {
      const res = await fetch(endpoint, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "same-origin",
        body: JSON.stringify(body),
      });
      const data = await res.json().catch(() => ({}));

      if (!res.ok || data.ok === false) {
        setError(data.error || "Authentication failed");
        setIsSubmitting(false);
        return;
      }

      // Hard nav so SSR runs for the destination route
      window.location.href = config.next || "/";
    } catch (e) {
      setError(e instanceof Error ? e.message : "Network error");
      setIsSubmitting(false);
    }
  }

  function switchMode(next: Mode) {
    if (next === mode) return;
    setMode(next);
    setError(null);
  }

  const submitLabel = mode === "login" ? "Sign in" : "Create account";

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
            "radial-gradient(ellipse 100% 100% at 50% 50%, transparent 45%, rgba(0,0,0,0.22) 100%)",
          pointerEvents: "none",
        }}
      />

      {/* ── UI layer ────────────────────────────────────────────────────── */}
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
        {/* Top-left label */}
        <div
          style={{
            padding: "14px 18px",
            fontFamily: FONT,
            fontSize: 13,
            fontWeight: 400,
            color: "rgba(255,255,255,0.58)",
            letterSpacing: "0.005em",
            userSelect: "none",
            animation: "simple-fade 500ms cubic-bezier(0.16,1,0.3,1) 1500ms both",
          }}
        >
          Mixer chat
        </div>

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
          {/* ── OUTER glass card ──────────────────────────────────────── */}
          <section
            style={{
              ...outerGlass,
              width: "min(440px, 92vw)",
              animation: "card-reveal 600ms cubic-bezier(0.7,0,0.15,1) 200ms both",
            }}
          >            <header
              style={{
                display: "flex",
                alignItems: "center",
                gap: 4,
                padding: "4px 4px 8px",
                animation: "content-fade 500ms cubic-bezier(0.16,1,0.3,1) 750ms both",
              }}
            >
              <TabButton
                label="Sign in"
                active={mode === "login"}
                onClick={() => switchMode("login")}
              />
              <TabButton
                label="Create account"
                active={mode === "register"}
                onClick={() => switchMode("register")}
              />
            </header>

            {/* ── INNER glass card (form lives here) ──────────────────── */}
            <div
              style={{
                ...innerGlass,
                padding: 14,
                animation: "content-fade 500ms cubic-bezier(0.16,1,0.3,1) 850ms both",
              }}
            >
              <form
                onSubmit={submit}
                style={{ display: "flex", flexDirection: "column", gap: 10 }}
              >
                {mode === "register" && (
                  <Field
                    name="name"
                    type="text"
                    placeholder="Full name"
                    autoComplete="name"
                  />
                )}
                <Field
                  name="email"
                  type="email"
                  placeholder="Email"
                  autoComplete="email"
                  required
                />
                <Field
                  name="password"
                  type="password"
                  placeholder="Password"
                  autoComplete={
                    mode === "login" ? "current-password" : "new-password"
                  }
                  required
                  minLength={8}
                />

                {error && <ErrorBanner message={error} />}

                <SubmitButton
                  label={submitLabel}
                  loading={isSubmitting}
                />
              </form>
            </div>
          </section>

          {/* Disclaimer */}
          <p
            style={{
              marginTop: 18,
              maxWidth: 360,
              fontFamily: FONT,
              fontSize: 11.5,
              fontWeight: 400,
              color: "rgba(255,255,255,0.42)",
              textAlign: "center",
              letterSpacing: "0.005em",
              animation: "simple-fade 500ms cubic-bezier(0.16,1,0.3,1) 1500ms both",
            }}
          >
            By continuing you agree to authenticate against the local SQLite
            user store.
          </p>
        </main>
      </div>
    </div>
  );
}

/* ── Tab button ──────────────────────────────────────────────────────────── */

function TabButton({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  const base: CSSProperties = {
    flex: 1,
    padding: "6px 14px",
    borderRadius: 99,
    fontFamily: FONT,
    fontSize: 12.5,
    fontWeight: 500,
    cursor: "pointer",
    letterSpacing: "0.005em",
    textAlign: "center",
    transition: "color 0.18s ease",
  };

  if (active) {
    return (
      <button
        type="button"
        onClick={onClick}
        style={{
          ...base,
          ...smallPillGlass,
          color: "rgba(255,255,255,0.92)",
        }}
      >
        {label}
      </button>
    );
  }

  return (
    <button
      type="button"
      onClick={onClick}
      style={{
        ...base,
        background: "transparent",
        border: "1px solid transparent",
        color: "rgba(255,255,255,0.42)",
        boxShadow: "none",
      }}
      onMouseEnter={(e) => {
        (e.currentTarget as HTMLButtonElement).style.color =
          "rgba(255,255,255,0.70)";
      }}
      onMouseLeave={(e) => {
        (e.currentTarget as HTMLButtonElement).style.color =
          "rgba(255,255,255,0.42)";
      }}
    >
      {label}
    </button>
  );
}

/* ── Form field (input wrapped in glass with focus halo) ─────────────────── */

interface FieldProps {
  name: string;
  type: "text" | "email" | "password";
  placeholder: string;
  autoComplete: string;
  required?: boolean;
  minLength?: number;
}

function Field({
  name,
  type,
  placeholder,
  autoComplete,
  required,
  minLength,
}: FieldProps) {
  const [focused, setFocused] = useState(false);

  return (
    <div
      style={{
        ...inputGlass,
        borderColor: focused
          ? "rgba(255,255,255,0.22)"
          : "rgba(255,255,255,0.10)",
        boxShadow: focused
          ? [
              "inset 0 1px 0 rgba(255,255,255,0.10)",
              "inset 0 -1px 0 rgba(0,0,0,0.25)",
              "0 0 0 3px rgba(255,255,255,0.05)",
            ].join(", ")
          : inputGlass.boxShadow,
        transition: "border-color 0.18s ease, box-shadow 0.18s ease",
      }}
    >
      <input
        name={name}
        type={type}
        placeholder={placeholder}
        autoComplete={autoComplete}
        required={required}
        minLength={minLength}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        style={{
          display: "block",
          width: "100%",
          background: "transparent",
          border: "none",
          outline: "none",
          padding: "11px 14px",
          fontFamily: FONT,
          fontSize: 14,
          fontWeight: 400,
          color: "rgba(255,255,255,0.88)",
          caretColor: "rgba(255,255,255,0.7)",
        }}
      />
    </div>
  );
}

/* ── Inline error banner ─────────────────────────────────────────────────── */

function ErrorBanner({ message }: { message: string }) {
  return (
    <div
      role="alert"
      style={{
        padding: "8px 12px",
        borderRadius: 8,
        background: "rgba(255, 80, 80, 0.10)",
        border: "1px solid rgba(255, 80, 80, 0.24)",
        color: "rgba(255, 190, 190, 0.95)",
        fontFamily: FONT,
        fontSize: 12.5,
        fontWeight: 400,
        lineHeight: 1.45,
      }}
    >
      {message}
    </div>
  );
}

/* ── Full-width submit button ────────────────────────────────────────────── */

function SubmitButton({
  label,
  loading,
}: {
  label: string;
  loading: boolean;
}) {
  return (
    <button
      type="submit"
      disabled={loading}
      style={{
        ...pillGlass,
        marginTop: 6,
        width: "100%",
        padding: "11px 16px",
        borderRadius: 10,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        gap: 8,
        fontFamily: FONT,
        fontSize: 14,
        fontWeight: 500,
        letterSpacing: "0.005em",
        color: "rgba(255,255,255,0.94)",
        cursor: loading ? "wait" : "pointer",
        opacity: loading ? 0.65 : 1,
        transition: "transform 0.18s ease, opacity 0.18s ease",
      }}
      onMouseEnter={(e) => {
        if (!loading)
          (e.currentTarget as HTMLButtonElement).style.transform =
            "translateY(-1px)";
      }}
      onMouseLeave={(e) => {
        (e.currentTarget as HTMLButtonElement).style.transform = "translateY(0)";
      }}
    >
      {label}
      {loading ? (
        <Loader
          size={14}
          strokeWidth={2}
          style={{
            animation: "spin 800ms linear infinite",
          }}
        />
      ) : (
        <ArrowRight size={14} strokeWidth={2} style={{ opacity: 0.85 }} />
      )}
      {/* Inline spin keyframe — scoped via document <style>{KEYFRAMES} */}
      <style>{`@keyframes spin { to { transform: rotate(360deg); } }`}</style>
    </button>
  );
}
