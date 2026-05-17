/** Shows where the current page data came from — great for the hackathon demo. */
interface Props {
  source: "ssr" | "api" | null;
  ts: number | null;
}

export default function SSRBadge({ source, ts }: Props) {
  if (!source) return null;

  const label = source === "ssr" ? "SSR" : "API";
  const time = ts ? new Date(ts).toLocaleTimeString([], { hour12: false }) : null;

  const isSSR = source === "ssr";

  return (
    <div
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        padding: "4px 10px 4px 8px",
        borderRadius: 99,
        border: `1px solid ${isSSR ? "color-mix(in srgb, var(--color-ember) 30%, transparent)" : "color-mix(in srgb, var(--color-violet) 30%, transparent)"}`,
        background: isSSR ? "var(--color-ember-dim)" : "color-mix(in srgb, var(--color-violet) 8%, transparent)",
        fontFamily: "var(--font-mono)",
        fontSize: 11,
        color: isSSR ? "var(--color-ember)" : "var(--color-violet)",
        letterSpacing: "0.05em",
        userSelect: "none",
      }}
    >
      <span
        style={{
          width: 5,
          height: 5,
          borderRadius: "50%",
          background: "currentColor",
          display: "block",
          animation: "ember-pulse 2s ease-in-out infinite",
        }}
      />
      <span>{label}</span>
      {time && (
        <>
          <span style={{ opacity: 0.4 }}>·</span>
          <span style={{ opacity: 0.7 }}>{time}</span>
        </>
      )}
    </div>
  );
}
