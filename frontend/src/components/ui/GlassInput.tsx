/**
 * GlassInput — the centrepiece chat/command input that floats over the video.
 *
 * Composed entirely from the Glass primitives so it inherits all physical
 * glass properties (specular, refraction, chromatic aberration, noise).
 */

import { useRef, useState, type KeyboardEvent } from "react";
import { Paperclip, ArrowUp, Loader } from "lucide-react";
import { GlassPanel, GlassIconButton } from "./Glass";

interface Props {
  placeholder?: string;
  onSubmit?: (value: string) => void;
  /** Slot for status content above the input */
  statusSlot?: React.ReactNode;
}

export default function GlassInput({
  placeholder = "Ask anything…",
  onSubmit,
  statusSlot,
}: Props) {
  const [value, setValue] = useState("");
  const [loading, setLoading] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);

  const handleSubmit = async () => {
    if (!value.trim() || loading) return;
    setLoading(true);
    await onSubmit?.(value.trim());
    setValue("");
    setLoading(false);
    ref.current?.focus();
  };

  const handleKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSubmit();
    }
  };

  const hasText = value.trim().length > 0;

  return (
    <GlassPanel
      blur="lg"
      tint="smoke"
      radius={18}
      style={{ width: "100%", maxWidth: 680 }}
    >
      {/* Status row */}
      {statusSlot && (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "10px 16px 0",
          }}
        >
          {statusSlot}
        </div>
      )}

      {/* Textarea */}
      <textarea
        ref={ref}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={handleKey}
        placeholder={placeholder}
        rows={3}
        style={{
          width: "100%",
          background: "transparent",
          border: "none",
          outline: "none",
          resize: "none",
          padding: "14px 16px 8px",
          fontFamily: "'DM Sans', system-ui, sans-serif",
          fontSize: 15,
          lineHeight: 1.6,
          color: "rgba(255,255,255,0.88)",
          caretColor: "rgba(255,255,255,0.7)",
        }}
      />

      {/* Action row */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "6px 10px 10px",
          gap: 8,
        }}
      >
        {/* Left icons */}
        <div style={{ display: "flex", gap: 6 }}>
          <GlassIconButton title="Attach file">
            <Paperclip size={15} strokeWidth={1.8} />
          </GlassIconButton>
        </div>

        {/* Send */}
        <GlassIconButton
          title="Send (Enter)"
          active={hasText && !loading}
          onClick={handleSubmit}
          style={{
            opacity: hasText ? 1 : 0.35,
            transition: "all 0.2s ease",
          }}
        >
          {loading ? (
            <Loader
              size={15}
              strokeWidth={1.8}
              style={{ animation: "spin 1s linear infinite" }}
            />
          ) : (
            <ArrowUp size={15} strokeWidth={2} />
          )}
        </GlassIconButton>
      </div>
    </GlassPanel>
  );
}
