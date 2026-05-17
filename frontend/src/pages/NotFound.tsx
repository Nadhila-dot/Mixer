import { useLocation, useNavigate } from "react-router-dom";
import { ArrowLeft } from "lucide-react";
import { getSsrEnvelope } from "../lib/ssr";
import AppBackground from "../components/AppBackground";
import { FONT, KEYFRAMES, outerGlass, innerGlass, pillGlass } from "../lib/glass";

/* ─────────────────────────────────────────────────────────────────────────── */
/*  404 — Route not found                                                      */
/*  Standalone fullscreen page that re-uses the Mixer-chat glass system        */
/*  (video bg, Chillax title, nested cards, entry choreography).               */
/* ─────────────────────────────────────────────────────────────────────────── */

export default function NotFound() {
  const navigate = useNavigate();
  const location = useLocation();
  const ssr = getSsrEnvelope();

  const requestedPath =
    (ssr?.page === "not-found" &&
      (ssr.data as Record<string, string> | undefined)?.requestedPath) ||
    location.pathname;

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
            "radial-gradient(ellipse 100% 100% at 50% 50%, transparent 45%, rgba(0,0,0,0.30) 100%)",
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
        {/* Top-left wordmark */}
        <div
          style={{
            padding: "14px 18px",
            fontFamily: FONT,
            fontSize: 13,
            fontWeight: 400,
            color: "rgba(255,255,255,0.58)",
            letterSpacing: "0.005em",
            userSelect: "none",
            animation:
              "simple-fade 500ms cubic-bezier(0.16,1,0.3,1) 1500ms both",
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
          {/* Giant 404 via SVG — same width-locking trick as Home title */}
          <svg
            viewBox="0 0 1000 220"
            width="min(440px, 92vw)"
            preserveAspectRatio="xMidYMid meet"
            aria-label="Oops!"
            style={{
              display: "block",
              margin: "0 auto -1.1rem",
              position: "relative",
              zIndex: 1,
              pointerEvents: "none",
              filter: "drop-shadow(0 1px 30px rgba(0,0,0,0.30))",
              animation:
                "title-pop 650ms cubic-bezier(0.34,1.56,0.64,1) 1250ms both",
            }}
          >
            <text
              x="500"
              y="170"
              textAnchor="middle"
              textLength="800"
              lengthAdjust="spacingAndGlyphs"
              fontFamily={FONT}
              fontWeight={400}
              fontSize="200"
              letterSpacing="-4"
              fill="#FFFAEE"
            >
              Whoops!
            </text>
          </svg>

          {/* Outer glass card */}
          <section
            style={{
              ...outerGlass,
              width: "min(440px, 92vw)",
              animation:
                "card-reveal 600ms cubic-bezier(0.7,0,0.15,1) 200ms both",
            }}
          >
            <header
              style={{
                padding: "6px 10px 8px",
                fontFamily: FONT,
                fontSize: 12.5,
                fontWeight: 400,
                color: "rgba(255,255,255,0.55)",
                textAlign: "center",
                animation:
                  "content-fade 500ms cubic-bezier(0.16,1,0.3,1) 750ms both",
              }}
            >
              Route not found
            </header>

            {/* Inner glass card holds the requested path + back button */}
            <div
              style={{
                ...innerGlass,
                padding: 16,
                display: "flex",
                flexDirection: "column",
                gap: 12,
                animation:
                  "content-fade 500ms cubic-bezier(0.16,1,0.3,1) 850ms both",
              }}
            >
              <p
                style={{
                  margin: 0,
                  fontFamily: FONT,
                  fontSize: 13,
                  color: "rgba(255,255,255,0.62)",
                  lineHeight: 1.55,
                  textAlign: "center",
                }}
              >
                The page
                <code
                  style={{
                    display: "inline-block",
                    margin: "0 6px",
                    padding: "1px 7px",
                    borderRadius: 6,
                    background: "rgba(0,0,0,0.40)",
                    border: "1px solid rgba(255,255,255,0.10)",
                    fontFamily: "'DM Mono', ui-monospace, monospace",
                    fontSize: 12,
                    color: "#FFFAEE",
                  }}
                >
                  {requestedPath}
                </code>
                isn&apos;t mapped to any handler.
              </p>

              <button
                type="button"
                onClick={() => navigate("/")}
                style={{
                  ...pillGlass,
                  width: "100%",
                  padding: "11px 16px",
                  borderRadius: 10,
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  gap: 8,
                  fontFamily: FONT,
                  fontSize: 13.5,
                  fontWeight: 500,
                  color: "rgba(255,255,255,0.94)",
                  cursor: "pointer",
                  transition: "transform 0.18s ease",
                }}
                onMouseEnter={(e) => {
                  (e.currentTarget as HTMLButtonElement).style.transform =
                    "translateY(-1px)";
                }}
                onMouseLeave={(e) => {
                  (e.currentTarget as HTMLButtonElement).style.transform =
                    "translateY(0)";
                }}
              >
                <ArrowLeft size={14} strokeWidth={1.8} />
                Back to Mixer chat
              </button>
            </div>
          </section>
        </main>
      </div>
    </div>
  );
}
