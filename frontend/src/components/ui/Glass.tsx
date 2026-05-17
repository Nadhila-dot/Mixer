/**
 * Glass — physics-accurate glass morphism primitives.
 *
 * What makes real glass look different from "Windows-7 frosted blur":
 *
 *   1. SPECULAR HIGHLIGHTS  — the front surface of glass reflects a bright
 *      highlight where light catches the rim.  We simulate this with an
 *      asymmetric top-border and a pseudo ::after highlight line.
 *
 *   2. INTERNAL REFLECTION  — the back surface of the glass also reflects
 *      ambient light back inward, creating a faint gradient from the bottom.
 *
 *   3. CHROMATIC ABERRATION — glass disperses white light into its spectrum
 *      at the edges. We add a very subtle red-cyan fringe on opposing corners.
 *
 *   4. IOR REFRACTION       — objects seen through glass appear slightly
 *      shifted. Applied via SVG filter="url(#glass-refract)" on the video
 *      layer in VideoBackground; the GlassPanel itself is transparent to it.
 *
 *   5. MICRO-SURFACE NOISE  — frosted glass has a rough surface that scatters
 *      light. We layer a low-opacity SVG noise texture on top.
 *
 *   6. THICKNESS             — a real glass pane has physical depth. We hint
 *      at this with asymmetric box-shadows (deep outer + shallow inner).
 */

import type { CSSProperties, ReactNode } from "react";

// ── Tokens ────────────────────────────────────────────────────────────────────

const BLUR = {
  sm: "blur(16px) saturate(160%)",
  md: "blur(28px) saturate(180%) brightness(0.92)",
  lg: "blur(48px) saturate(200%) brightness(0.88)",
} as const;

const TINT = {
  dark:  "rgba(6, 7, 9, 0.58)",
  smoke: "rgba(18, 20, 24, 0.52)",
  dusk:  "rgba(12, 10, 18, 0.55)",
  ash:   "rgba(22, 22, 26, 0.48)",
} as const;

// ── Shared style builder ──────────────────────────────────────────────────────

function glassStyle(
  blur: keyof typeof BLUR = "md",
  tint: keyof typeof TINT = "smoke",
  radius = 16,
  extra?: CSSProperties
): CSSProperties {
  return {
    position: "relative",
    isolation: "isolate",          // new stacking context for pseudo-layers
    backdropFilter: BLUR[blur],
    WebkitBackdropFilter: BLUR[blur],

    background: [
      // ① Specular highlight — bright patch where main light source (top-left) hits front surface
      "radial-gradient(ellipse 70% 35% at 25% 0%, rgba(255,255,255,0.09) 0%, transparent 100%)",
      // ② Secondary specular — softer fill from top-right (sky bounce)
      "radial-gradient(ellipse 50% 25% at 80% 0%, rgba(255,255,255,0.04) 0%, transparent 100%)",
      // ③ Internal back-surface reflection — faint warm glow from the bottom
      "linear-gradient(180deg, transparent 55%, rgba(255,255,255,0.025) 100%)",
      // ④ Base glass tint
      TINT[tint],
    ].join(", "),

    // Asymmetric border simulates how light catches different edges of the pane
    borderTop:    "1px solid rgba(255,255,255,0.22)",
    borderLeft:   "1px solid rgba(255,255,255,0.11)",
    borderRight:  "1px solid rgba(255,255,255,0.06)",
    borderBottom: "1px solid rgba(255,255,255,0.06)",

    boxShadow: [
      // Outer: glass casting deep shadow on the background
      "0 24px 80px rgba(0,0,0,0.55)",
      "0 8px 24px rgba(0,0,0,0.35)",
      "0 2px 6px rgba(0,0,0,0.25)",
      // Inner top: bright reflection off the inner face of the front surface
      "inset 0 1px 0 rgba(255,255,255,0.20)",
      // Inner sides: softer lateral reflections (light entering from sides)
      "inset 1px 0 0 rgba(255,255,255,0.07)",
      "inset -1px 0 0 rgba(255,255,255,0.04)",
      // Inner bottom: slight darkening (light absorbed by lower edge)
      "inset 0 -1px 0 rgba(0,0,0,0.18)",
      // Micro inner glow — caustic light pattern
      "inset 0 0 40px rgba(255,255,255,0.02)",
    ].join(", "),

    borderRadius: radius,
    overflow: "hidden",
    ...extra,
  };
}

// ── Chromatic aberration overlay ──────────────────────────────────────────────
// Sits on top as an absolutely-positioned sibling via a wrapper pattern.

function ChromaticFringe({ radius }: { radius: number }) {
  return (
    <span
      aria-hidden
      style={{
        position: "absolute",
        inset: 0,
        borderRadius: radius,
        pointerEvents: "none",
        zIndex: 10,
        // Red fringe top-left, cyan fringe bottom-right — glass dispersion
        background:
          "linear-gradient(135deg, rgba(255,80,60,0.055) 0%, transparent 18%, transparent 82%, rgba(60,120,255,0.045) 100%)",
        mixBlendMode: "screen",
      }}
    />
  );
}

// ── Noise texture overlay ─────────────────────────────────────────────────────
// Simulates micro-surface roughness of frosted glass.

const NOISE_SVG = `<svg xmlns='http://www.w3.org/2000/svg' width='200' height='200'>
  <filter id='n'><feTurbulence type='fractalNoise' baseFrequency='0.75' numOctaves='4' stitchTiles='stitch'/></filter>
  <rect width='200' height='200' filter='url(%23n)' opacity='1'/>
</svg>`;

function GlassNoise({ radius }: { radius: number }) {
  return (
    <span
      aria-hidden
      style={{
        position: "absolute",
        inset: 0,
        borderRadius: radius,
        pointerEvents: "none",
        zIndex: 9,
        backgroundImage: `url("data:image/svg+xml,${NOISE_SVG}")`,
        backgroundRepeat: "repeat",
        opacity: 0.022,
        mixBlendMode: "overlay",
      }}
    />
  );
}

// ── Public components ─────────────────────────────────────────────────────────

interface GlassPanelProps {
  children: ReactNode;
  blur?: keyof typeof BLUR;
  tint?: keyof typeof TINT;
  radius?: number;
  style?: CSSProperties;
  className?: string;
  onClick?: () => void;
}

/** The core glass surface. Wrap any content in this for the full effect. */
export function GlassPanel({
  children,
  blur = "md",
  tint = "smoke",
  radius = 16,
  style,
  className,
  onClick,
}: GlassPanelProps) {
  return (
    <div
      className={className}
      onClick={onClick}
      style={{ ...glassStyle(blur, tint, radius), ...style }}
    >
      <ChromaticFringe radius={radius} />
      <GlassNoise radius={radius} />
      {children}
    </div>
  );
}

/** Pill / badge variant — smaller radius, lighter tint */
export function GlassPill({
  children,
  style,
  onClick,
}: {
  children: ReactNode;
  style?: CSSProperties;
  onClick?: () => void;
}) {
  return (
    <div
      onClick={onClick}
      style={{
        ...glassStyle("sm", "ash", 99),
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        padding: "6px 14px",
        cursor: onClick ? "pointer" : "default",
        ...style,
      }}
    >
      <ChromaticFringe radius={99} />
      {children}
    </div>
  );
}

/** Icon button that sits on a glass surface — inherits the glass context */
export function GlassIconButton({
  children,
  onClick,
  style,
  title,
  active,
}: {
  children: ReactNode;
  onClick?: () => void;
  style?: CSSProperties;
  title?: string;
  active?: boolean;
}) {
  return (
    <button
      title={title}
      onClick={onClick}
      style={{
        ...glassStyle("sm", active ? "dusk" : "ash", 10),
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        width: 36,
        height: 36,
        border: "none",
        padding: 0,
        cursor: "pointer",
        color: active ? "rgba(255,255,255,0.9)" : "rgba(255,255,255,0.55)",
        transition: "all 0.2s ease",
        flexShrink: 0,
        ...style,
      }}
      onMouseEnter={(e) => {
        (e.currentTarget as HTMLButtonElement).style.color = "rgba(255,255,255,0.9)";
      }}
      onMouseLeave={(e) => {
        (e.currentTarget as HTMLButtonElement).style.color = active
          ? "rgba(255,255,255,0.9)"
          : "rgba(255,255,255,0.55)";
      }}
    >
      <ChromaticFringe radius={10} />
      {children}
    </button>
  );
}
