import { type CSSProperties, useEffect, useId, useState } from "react";
import { FONT, smallPillGlass } from "../lib/glass";

// Lazy-import mermaid + initialize exactly once. The library is ~500kB so we
// don't want it in the main bundle; the first `<code language="mermaid">` block
// in a session triggers the chunk fetch via this promise.
let mermaidPromise: Promise<typeof import("mermaid").default> | null = null;
function loadMermaid() {
  if (!mermaidPromise) {
    mermaidPromise = import("mermaid").then((mod) => {
      const mermaid = mod.default;
      mermaid.initialize({
        startOnLoad: false,
        theme: "dark",
        securityLevel: "loose",
        fontFamily: "'DM Mono', ui-monospace, monospace",
        themeVariables: {
          background: "transparent",
          primaryColor: "rgba(255,255,255,0.08)",
          primaryTextColor: "rgba(255,255,255,0.92)",
          primaryBorderColor: "rgba(255,255,255,0.30)",
          lineColor: "rgba(255,255,255,0.45)",
          secondaryColor: "rgba(120, 220, 140, 0.22)",
          tertiaryColor: "rgba(100, 180, 255, 0.20)",
          mainBkg: "rgba(255,255,255,0.06)",
          edgeLabelBackground: "rgba(0,0,0,0.40)",
          textColor: "rgba(255,255,255,0.88)",
          nodeBorder: "rgba(255,255,255,0.28)",
          clusterBkg: "rgba(255,255,255,0.03)",
          clusterBorder: "rgba(255,255,255,0.12)",
        },
      });
      return mermaid;
    });
  }
  return mermaidPromise;
}

export default function MermaidBlock({ code }: { code: string }) {
  const rawId = useId();
  const id = `mermaid-${rawId.replace(/[^a-zA-Z0-9]/g, "")}`;
  const [svg, setSvg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showSource, setShowSource] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    setSvg(null);
    const source = code.trim();
    if (!source) {
      return;
    }
    void loadMermaid().then(async (mermaid) => {
      try {
        const result = await mermaid.render(id, source);
        if (!cancelled) setSvg(result.svg);
      } catch (e) {
        if (!cancelled) {
          const message = e instanceof Error ? e.message : "Failed to render diagram";
          setError(message);
        }
      }
    });
    return () => {
      cancelled = true;
    };
  }, [code, id]);

  return (
    <div style={cardStyle}>
      <div style={headerStyle}>
        <span style={badgeStyle}>MERMAID</span>
        <span style={{ flex: 1 }} />
        <button type="button" onClick={() => setShowSource((s) => !s)} style={toggleStyle}>
          {showSource ? "Hide source" : "View source"}
        </button>
      </div>
      <div style={bodyStyle}>
        {error ? (
          <div style={errorStyle}>
            <strong style={{ color: "rgba(255, 200, 200, 0.95)" }}>Diagram error</strong>
            <div style={{ marginTop: 4, fontSize: 11.5 }}>{error}</div>
          </div>
        ) : svg ? (
          <div
            dangerouslySetInnerHTML={{ __html: svg }}
            style={diagramStyle}
          />
        ) : (
          <div style={loadingStyle}>Rendering diagram…</div>
        )}
        {showSource && <pre style={sourceStyle}>{code.trim()}</pre>}
      </div>
    </div>
  );
}

/* ─────────────────────────────────────────────────────────────────── */
/*  Styles                                                              */
/* ─────────────────────────────────────────────────────────────────── */

const cardStyle: CSSProperties = {
  marginTop: 8,
  borderRadius: 12,
  background: "rgba(0,0,0,0.30)",
  border: "1px solid rgba(255,255,255,0.08)",
  overflow: "hidden",
};

const headerStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  padding: "6px 10px 6px 12px",
  borderBottom: "1px solid rgba(255,255,255,0.06)",
  background: "rgba(255,255,255,0.02)",
};

const badgeStyle: CSSProperties = {
  ...smallPillGlass,
  padding: "2px 8px",
  borderRadius: 99,
  fontSize: 9.5,
  letterSpacing: "0.12em",
  fontWeight: 500,
  color: "rgba(255,255,255,0.72)",
  fontFamily: FONT,
};

const toggleStyle: CSSProperties = {
  background: "transparent",
  border: "none",
  color: "rgba(255,255,255,0.55)",
  fontSize: 11,
  fontFamily: FONT,
  cursor: "pointer",
  padding: "2px 6px",
  borderRadius: 5,
};

const bodyStyle: CSSProperties = {
  padding: "14px 14px 16px",
  display: "flex",
  flexDirection: "column",
  gap: 10,
  minHeight: 60,
};

const diagramStyle: CSSProperties = {
  width: "100%",
  overflow: "auto",
  display: "flex",
  justifyContent: "center",
  alignItems: "center",
};

const loadingStyle: CSSProperties = {
  fontSize: 12,
  color: "rgba(255,255,255,0.40)",
  fontFamily: FONT,
  textAlign: "center",
  padding: "16px 0",
};

const errorStyle: CSSProperties = {
  padding: "10px 12px",
  borderRadius: 8,
  border: "1px solid rgba(220, 80, 80, 0.30)",
  background: "rgba(220, 80, 80, 0.08)",
  color: "rgba(255, 180, 180, 0.88)",
  fontSize: 12,
  fontFamily: FONT,
};

const sourceStyle: CSSProperties = {
  margin: 0,
  padding: "10px 12px",
  borderRadius: 8,
  background: "rgba(0,0,0,0.36)",
  border: "1px solid rgba(255,255,255,0.05)",
  fontFamily: "'DM Mono', ui-monospace, monospace",
  fontSize: 11.5,
  lineHeight: 1.55,
  color: "rgba(255,255,255,0.78)",
  whiteSpace: "pre-wrap",
  wordBreak: "break-word",
  overflowX: "auto",
};
