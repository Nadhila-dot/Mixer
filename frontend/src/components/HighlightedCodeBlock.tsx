import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
import { oneDark } from "react-syntax-highlighter/dist/esm/styles/prism";

const codeBlockStyle = {
  margin: "8px 0 0",
  padding: "10px 12px",
  borderRadius: 10,
  background: "rgba(0,0,0,0.30)",
  border: "1px solid rgba(255,255,255,0.08)",
  overflowX: "auto" as const,
  whiteSpace: "pre" as const,
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
  fontSize: 12.5,
  color: "rgba(255,255,255,0.90)",
};

const syntaxTheme = {
  ...oneDark,
  'pre[class*="language-"]': {
    ...(oneDark['pre[class*="language-"]'] ?? {}),
    margin: 0,
    padding: 0,
    background: "transparent",
    fontFamily: "'DM Mono', ui-monospace, monospace",
    fontSize: "12.5px",
    lineHeight: 1.65,
  },
  'code[class*="language-"]': {
    ...(oneDark['code[class*="language-"]'] ?? {}),
    fontFamily: "'DM Mono', ui-monospace, monospace",
    background: "transparent",
    textShadow: "none",
  },
} as const;

export default function HighlightedCodeBlock({
  code,
  language,
}: {
  code: string;
  language?: string;
}) {
  return (
    <div style={codeBlockStyle}>
      <SyntaxHighlighter
        language={normalizeCodeLanguage(language)}
        style={syntaxTheme}
        PreTag="div"
        customStyle={{
          margin: 0,
          padding: 0,
          background: "transparent",
          overflow: "visible",
        }}
        codeTagProps={{
          style: {
            fontFamily: "'DM Mono', ui-monospace, monospace",
          },
        }}
        wrapLongLines
      >
        {code}
      </SyntaxHighlighter>
    </div>
  );
}

function normalizeCodeLanguage(language?: string) {
  const value = language?.trim().toLowerCase();
  if (!value) return "text";
  if (value === "py") return "python";
  if (value === "ts") return "typescript";
  if (value === "js") return "javascript";
  if (value === "sh") return "bash";
  if (value === "yml") return "yaml";
  return value;
}
