import { ChevronDown } from "lucide-react";
import { FONT, smallPillGlass } from "../lib/glass";
import type { AiModel } from "../types";

interface Props {
  models: AiModel[];
  selected: string;
  onChange: (model: string) => void;
}

export default function ModelSelector({ models, selected, onChange }: Props) {
  if (models.length === 0) return null;

  return (
    <label
      style={{
        ...smallPillGlass,
        position: "relative",
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        padding: "5px 28px 5px 10px",
        borderRadius: 999,
        color: "rgba(255,255,255,0.78)",
        fontFamily: FONT,
        fontSize: 11.5,
        cursor: "pointer",
      }}
    >
      <span style={{ opacity: 0.52 }}>Model</span>
      <select
        value={selected}
        onChange={(event) => onChange(event.target.value)}
        aria-label="AI model"
        style={{
          position: "absolute",
          inset: 0,
          opacity: 0,
          cursor: "pointer",
        }}
      >
        {models.map((model) => (
          <option key={model.id} value={model.id}>
            {model.label || model.id}
          </option>
        ))}
      </select>
      <span
        style={{
          maxWidth: 180,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
          color: "rgba(255,255,255,0.88)",
        }}
      >
        {models.find((model) => model.id === selected)?.label || selected}
      </span>
      <ChevronDown
        size={12}
        strokeWidth={1.8}
        style={{ position: "absolute", right: 9, opacity: 0.55 }}
      />
    </label>
  );
}
