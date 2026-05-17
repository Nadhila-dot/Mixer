import { type CSSProperties, type MouseEvent, type ReactNode, createElement } from "react";

/* ─────────────────────────────────────────────────────────────────────────── */
/*  Mixer chat — shared liquid-glass design system                             */
/*                                                                             */
/*  Every glass surface stacks: caustic highlight + side rim + vertical depth  */
/*  + base tint, plus a 4-8 layer box-shadow (outer lift / inner top rim /     */
/*  inner bottom shadow / inner ambient glow).                                 */
/*                                                                             */
/*  The KEYFRAMES export ships once via <style>{KEYFRAMES}</style> on each     */
/*  page that animates entry — keeps the rules document-scoped instead of      */
/*  leaking into the global stylesheet.                                        */
/* ─────────────────────────────────────────────────────────────────────────── */

/** Universal font stack — Chillax everywhere */
export const FONT = "'Chillax', 'Quicksand', system-ui, sans-serif";

/* ── Surface tokens ───────────────────────────────────────────────────────── */

/** Outer card — big container. Floats off the page with deep shadows. */
export const outerGlass: CSSProperties = {
  background: [
    // 1. Top caustic — bright patch where the main light source hits
    "radial-gradient(ellipse 75% 35% at 50% -5%, rgba(255,255,255,0.10) 0%, transparent 70%)",
    // 2. Left rim light — sky bouncing in from the left
    "linear-gradient(90deg, rgba(255,255,255,0.05) 0%, transparent 12%)",
    // 3. Vertical depth — top lit, bottom in shadow
    "linear-gradient(180deg, rgba(255,255,255,0.04) 0%, transparent 35%, rgba(0,0,0,0.10) 100%)",
    // 4. Diagonal sheen — subtle glass-surface highlight
    "linear-gradient(135deg, rgba(255,255,255,0.025) 0%, transparent 30%, transparent 70%, rgba(255,255,255,0.015) 100%)",
    // 5. Base tint
    "rgba(14,14,17,0.62)",
  ].join(", "),
  // Asymmetric border — top edge brighter (catches light), bottom darker
  border: "1px solid rgba(255,255,255,0.07)",
  borderTop: "1px solid rgba(255,255,255,0.22)",
  borderBottom: "1px solid rgba(255,255,255,0.04)",
  boxShadow: [
    // Outer drop — floating off the page
    "0 32px 80px rgba(0,0,0,0.55)",
    "0 12px 28px rgba(0,0,0,0.38)",
    "0 4px 8px rgba(0,0,0,0.25)",
    // Inner top rim — sharp light reflection (the "wet" look)
    "inset 0 1px 0 rgba(255,255,255,0.22)",
    // Inner bottom shadow — gives the glass physical thickness
    "inset 0 -1px 0 rgba(0,0,0,0.40)",
    // Inner side rims
    "inset 1px 0 0 rgba(255,255,255,0.06)",
    "inset -1px 0 0 rgba(255,255,255,0.03)",
    // Internal ambient — distant caustic glow
    "inset 0 40px 80px -40px rgba(255,255,255,0.04)",
  ].join(", "),
  borderRadius: 18,
  padding: 10,
  backdropFilter: "blur(30px) saturate(170%) brightness(0.95)",
  WebkitBackdropFilter: "blur(30px) saturate(170%) brightness(0.95)",
};

/** Inner card — recessed into the outer card. */
export const innerGlass: CSSProperties = {
  background: [
    "radial-gradient(ellipse 60% 25% at 50% 0%, rgba(255,255,255,0.05) 0%, transparent 80%)",
    "linear-gradient(180deg, rgba(255,255,255,0.02) 0%, transparent 30%, rgba(0,0,0,0.18) 100%)",
    "rgba(0,0,0,0.42)",
  ].join(", "),
  border: "1px solid rgba(255,255,255,0.07)",
  borderTop: "1px solid rgba(255,255,255,0.17)",
  borderBottom: "1px solid rgba(255,255,255,0.03)",
  boxShadow: [
    "inset 0 1px 0 rgba(255,255,255,0.13)",
    "inset 0 -1px 0 rgba(0,0,0,0.30)",
    "0 2px 8px rgba(0,0,0,0.25)",
  ].join(", "),
  borderRadius: 12,
  overflow: "hidden",
};

/** Pill button — pronounced top rim, lifted feel. */
export const pillGlass: CSSProperties = {
  background: [
    "linear-gradient(180deg, rgba(255,255,255,0.10) 0%, rgba(255,255,255,0.03) 50%, rgba(0,0,0,0.10) 100%)",
    "rgba(22,22,26,0.72)",
  ].join(", "),
  border: "1px solid rgba(255,255,255,0.08)",
  borderTop: "1px solid rgba(255,255,255,0.26)",
  borderBottom: "1px solid rgba(255,255,255,0.04)",
  boxShadow: [
    "0 8px 20px rgba(0,0,0,0.4)",
    "0 2px 6px rgba(0,0,0,0.30)",
    "inset 0 1px 0 rgba(255,255,255,0.24)",
    "inset 0 -1px 0 rgba(0,0,0,0.28)",
  ].join(", "),
  backdropFilter: "blur(16px) saturate(160%)",
  WebkitBackdropFilter: "blur(16px) saturate(160%)",
};

/** Small inline chip pill. */
export const smallPillGlass: CSSProperties = {
  background: [
    "linear-gradient(180deg, rgba(255,255,255,0.12) 0%, rgba(255,255,255,0.03) 50%, rgba(0,0,0,0.08) 100%)",
    "rgba(255,255,255,0.06)",
  ].join(", "),
  border: "1px solid rgba(255,255,255,0.10)",
  borderTop: "1px solid rgba(255,255,255,0.30)",
  borderBottom: "1px solid rgba(255,255,255,0.04)",
  boxShadow: [
    "0 2px 5px rgba(0,0,0,0.30)",
    "inset 0 1px 0 rgba(255,255,255,0.28)",
    "inset 0 -1px 0 rgba(0,0,0,0.20)",
  ].join(", "),
};

/** Icon button — small square glass tile. */
export const iconBtnGlass: CSSProperties = {
  background: [
    "linear-gradient(180deg, rgba(255,255,255,0.10) 0%, rgba(255,255,255,0.02) 50%, rgba(0,0,0,0.08) 100%)",
    "rgba(255,255,255,0.04)",
  ].join(", "),
  border: "1px solid rgba(255,255,255,0.08)",
  borderTop: "1px solid rgba(255,255,255,0.22)",
  borderBottom: "1px solid rgba(255,255,255,0.04)",
  boxShadow: [
    "0 3px 8px rgba(0,0,0,0.30)",
    "0 1px 2px rgba(0,0,0,0.20)",
    "inset 0 1px 0 rgba(255,255,255,0.20)",
    "inset 0 -1px 0 rgba(0,0,0,0.20)",
  ].join(", "),
};

/**
 * Sidebar panel — tall vertical glass surface.
 * Borders / shadows are weighted for a left-anchored panel: the right edge
 * is the visible "seam" so it gets the brighter rim + cast shadow.
 */
export const sidebarGlass: CSSProperties = {
  background: [
    "radial-gradient(ellipse 100% 18% at 50% 0%, rgba(255,255,255,0.08) 0%, transparent 70%)",
    "linear-gradient(90deg, transparent 90%, rgba(255,255,255,0.04) 100%)",
    "linear-gradient(180deg, rgba(255,255,255,0.03) 0%, transparent 30%, rgba(0,0,0,0.10) 100%)",
    "rgba(8,8,11,0.78)",
  ].join(", "),
  border: "1px solid rgba(255,255,255,0.04)",
  borderTop: "1px solid rgba(255,255,255,0.10)",
  borderRight: "1px solid rgba(255,255,255,0.12)",
  borderLeft: "none",
  borderBottom: "1px solid rgba(255,255,255,0.04)",
  boxShadow: [
    "32px 0 80px rgba(0,0,0,0.55)",
    "8px 0 24px rgba(0,0,0,0.35)",
    "inset 0 1px 0 rgba(255,255,255,0.12)",
    "inset 0 -1px 0 rgba(0,0,0,0.30)",
    "inset -1px 0 0 rgba(255,255,255,0.08)",
  ].join(", "),
  backdropFilter: "blur(40px) saturate(180%) brightness(0.85)",
  WebkitBackdropFilter: "blur(40px) saturate(180%) brightness(0.85)",
};

/**
 * Form input — flat dark glass, recessed feel.
 * Combines with focus state on the wrapper for an interactive halo.
 */
export const inputGlass: CSSProperties = {
  background: [
    "linear-gradient(180deg, rgba(0,0,0,0.30) 0%, rgba(0,0,0,0.18) 100%)",
    "rgba(0,0,0,0.22)",
  ].join(", "),
  border: "1px solid rgba(255,255,255,0.10)",
  borderTop: "1px solid rgba(255,255,255,0.16)",
  borderBottom: "1px solid rgba(255,255,255,0.04)",
  boxShadow: [
    "inset 0 1px 0 rgba(255,255,255,0.08)",
    "inset 0 -1px 0 rgba(0,0,0,0.25)",
  ].join(", "),
  borderRadius: 10,
};

/* ── Entry-choreography keyframes ─────────────────────────────────────────── */
/*
 * Drop into a page once via <style>{KEYFRAMES}</style>.
 * Total entrance ≈ 1.9s. Sequence:
 *   video fades in            → 0–800ms
 *   outer card opens center→edges (clip-path) → 200–800ms
 *   inside-card content fades  → 750ms onwards (staggered)
 *   hero title pops up         → 1250ms
 *   peripheral chrome eases in → 1500ms
 */
export const KEYFRAMES = `
  @keyframes card-reveal {
    from { clip-path: inset(0 50% 0 50% round 18px); }
    to   { clip-path: inset(0 0%  0 0%  round 18px); }
  }
  @keyframes content-fade {
    from { opacity: 0; transform: translateY(6px); }
    to   { opacity: 1; transform: translateY(0); }
  }
  @keyframes title-pop {
    0%   { opacity: 0; transform: translateY(60px) scale(0.92); }
    60%  { opacity: 1; transform: translateY(-4px) scale(1.01); }
    100% { opacity: 1; transform: translateY(0)    scale(1);    }
  }
  @keyframes video-fade-in {
    from { opacity: 0; transform: scale(1.06); }
    to   { opacity: 1; transform: scale(1.04); }
  }
  @keyframes simple-fade {
    from { opacity: 0; transform: translateY(6px); }
    to   { opacity: 1; transform: translateY(0); }
  }
  @keyframes workspace-slide-in {
    from { opacity: 0; transform: translateX(24px); }
    to   { opacity: 1; transform: translateX(0); }
  }
  @keyframes pulse-dot {
    0%, 100% { opacity: 1; transform: scale(1); }
    50%      { opacity: 0.45; transform: scale(0.82); }
  }
  @keyframes tool-card-in {
    from { opacity: 0; transform: translate3d(4px, 4px, 0); }
    to   { opacity: 1; transform: translate3d(0, 0, 0); }
  }
`;

/* ── IconBtn — small raised glass tile, used on Home + AuthScreen ─────────── */

/**
 * Returns a React element (without JSX so this file can stay .ts).
 * Hover state lifts the button by 1px and brightens the icon colour.
 */
export function IconBtn({
  children,
  title,
  onClick,
}: {
  children: ReactNode;
  title?: string;
  onClick?: () => void;
}) {
  return createElement(
    "button",
    {
      title,
      onClick,
      style: {
        ...iconBtnGlass,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        width: 34,
        height: 34,
        borderRadius: 8,
        color: "rgba(255,255,255,0.62)",
        cursor: "pointer",
        padding: 0,
        transition: "color 0.15s ease, transform 0.15s ease",
      },
      onMouseEnter: (e: MouseEvent<HTMLButtonElement>) => {
        const el = e.currentTarget;
        el.style.color = "rgba(255,255,255,0.95)";
        el.style.transform = "translateY(-1px)";
      },
      onMouseLeave: (e: MouseEvent<HTMLButtonElement>) => {
        const el = e.currentTarget;
        el.style.color = "rgba(255,255,255,0.62)";
        el.style.transform = "translateY(0)";
      },
    },
    children,
  );
}
