import { type CSSProperties, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  ChevronLeft,
  LogOut,
  MessageSquarePlus,
  Search,
  Settings,
  Trash2,
} from "lucide-react";
import {
  FONT,
  iconBtnGlass,
  innerGlass,
  pillGlass,
  sidebarGlass,
  smallPillGlass,
} from "../lib/glass";
import type { ChatSummary, HomeData } from "../types";

interface Props {
  open: boolean;
  onClose: () => void;
  user: HomeData["user"];
  chats: ChatSummary[];
  onDeleteChat: (id: string) => void;
}

export default function Sidebar({ open, onClose, user, chats, onDeleteChat }: Props) {
  const navigate = useNavigate();
  const [filter, setFilter] = useState("");

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  const filtered = useMemo(() => {
    const query = filter.trim().toLowerCase();
    if (!query) return chats;
    return chats.filter((chat) =>
      `${chat.title} ${chat.preview}`.toLowerCase().includes(query),
    );
  }, [chats, filter]);

  return (
    <>
      <div
        onClick={onClose}
        aria-hidden
        style={{
          position: "fixed",
          inset: 0,
          background: "rgba(0,0,0,0.40)",
          backdropFilter: "blur(2px)",
          WebkitBackdropFilter: "blur(2px)",
          zIndex: 90,
          opacity: open ? 1 : 0,
          pointerEvents: open ? "auto" : "none",
          transition: "opacity 280ms ease",
        }}
      />

      <aside
        role="dialog"
        aria-modal="true"
        aria-label="Conversations"
        style={{
          ...sidebarGlass,
          position: "fixed",
          top: 0,
          left: 0,
          bottom: 0,
          width: 336,
          maxWidth: "88vw",
          zIndex: 100,
          transform: open ? "translateX(0)" : "translateX(-105%)",
          transition:
            "transform 360ms cubic-bezier(0.22, 1, 0.36, 1), box-shadow 360ms ease",
          display: "flex",
          flexDirection: "column",
          fontFamily: FONT,
          borderRadius: 0,
          borderTopLeftRadius: 0,
          borderBottomLeftRadius: 0,
        }}
      >
        <header
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "14px 14px 10px",
          }}
        >
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            {/*<Logo />*/}
            <div>
              <div
                style={{
                  fontFamily: FONT,
                  fontWeight: 500,
                  fontSize: 15,
                  color: "rgba(255,255,255,0.92)",
                  letterSpacing: "-0.005em",
                }}
              >
                Mixer chat
              </div>
              <div
                style={{
                  fontSize: 11,
                  color: "rgba(255,255,255,0.42)",
                  textTransform: "capitalize",
                }}
              >
                {user.tier ?? "free"} workspace
              </div>
            </div>
          </div>
          <button
            title="Close sidebar"
            onClick={onClose}
            style={iconButtonStyle}
          >
            <ChevronLeft size={15} strokeWidth={1.8} />
          </button>
        </header>

        <div style={{ padding: "0 12px 8px" }}>
          <button
            onClick={() => {
              window.location.assign("/");
              onClose();
            }}
            style={{
              ...pillGlass,
              width: "100%",
              padding: "10px 14px",
              borderRadius: 10,
              display: "flex",
              alignItems: "center",
              gap: 9,
              fontFamily: FONT,
              fontSize: 13,
              fontWeight: 500,
              color: "rgba(255,255,255,0.94)",
              cursor: "pointer",
            }}
          >
            <MessageSquarePlus size={14} strokeWidth={1.8} />
            New chat
          </button>
        </div>

        <div style={{ padding: "4px 12px 12px" }}>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              padding: "7px 11px",
              borderRadius: 8,
              background: "rgba(0,0,0,0.30)",
              border: "1px solid rgba(255,255,255,0.06)",
              borderTop: "1px solid rgba(255,255,255,0.10)",
            }}
          >
            <Search size={13} strokeWidth={1.8} color="rgba(255,255,255,0.4)" />
            <input
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder="Search conversations"
              style={{
                flex: 1,
                background: "transparent",
                border: "none",
                outline: "none",
                fontFamily: FONT,
                fontSize: 12.5,
                color: "rgba(255,255,255,0.86)",
                padding: 0,
              }}
            />
          </div>
        </div>

        <SectionLabel>Recent chats</SectionLabel>
        <nav
          style={{
            flex: 1,
            overflowY: "auto",
            padding: "0 8px 12px",
            display: "flex",
            flexDirection: "column",
            gap: 2,
          }}
        >
          {filtered.length === 0 ? (
            <p
              style={{
                fontFamily: FONT,
                fontSize: 12.5,
                color: "rgba(255,255,255,0.42)",
                padding: "10px 8px",
                margin: 0,
              }}
            >
              {filter ? "No matching chats." : "No chats yet. Send a message to start one."}
            </p>
          ) : (
            filtered.map((chat) => (
              <ChatRow key={chat.id} chat={chat} onDeleteChat={onDeleteChat} />
            ))
          )}
        </nav>

        <UserPanel
          user={user}
          onSettings={() => {
            onClose();
            navigate("/settings");
          }}
        />
      </aside>
    </>
  );
}

const iconButtonStyle: CSSProperties = {
  ...iconBtnGlass,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  width: 30,
  height: 30,
  borderRadius: 8,
  color: "rgba(255,255,255,0.62)",
  cursor: "pointer",
  padding: 0,
};

function SectionLabel({ children }: { children: string }) {
  return (
    <div
      style={{
        padding: "8px 18px 6px",
        fontFamily: FONT,
        fontSize: 10.5,
        fontWeight: 500,
        letterSpacing: "0.08em",
        textTransform: "uppercase",
        color: "rgba(255,255,255,0.36)",
      }}
    >
      {children}
    </div>
  );
}

function ChatRow({
  chat,
  onDeleteChat,
}: {
  chat: ChatSummary;
  onDeleteChat: (id: string) => void;
}) {
  const [hover, setHover] = useState(false);

  return (
    <div
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      style={{
        display: "grid",
        gridTemplateColumns: "1fr auto auto",
        gap: 8,
        padding: "8px 10px",
        borderRadius: 7,
        fontFamily: FONT,
        cursor: "pointer",
        transition: "background 0.15s ease, color 0.15s ease",
        border: "1px solid transparent",
        background: hover ? "rgba(255,255,255,0.04)" : "transparent",
        color: hover ? "rgba(255,255,255,0.88)" : "rgba(255,255,255,0.66)",
        textAlign: "left",
      }}
    >
      <a
        href={`/chat/${chat.id}`}
        style={{
          minWidth: 0,
          color: "inherit",
          textDecoration: "none",
        }}
      >
        <span
          style={{
            display: "block",
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
            fontSize: 13,
          }}
        >
          {chat.title}
        </span>
        <span
          style={{
            display: "block",
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
            fontSize: 11,
            color: "rgba(255,255,255,0.34)",
          }}
        >
          {chat.preview}
        </span>
      </a>
      <span
        style={{
          fontSize: 10.5,
          color: "rgba(255,255,255,0.32)",
          paddingTop: 1,
        }}
      >
        {relativeTime(chat.updated_at)}
      </span>
      <button
        title="Delete chat"
        onClick={() => onDeleteChat(chat.id)}
        style={{
          width: 22,
          height: 22,
          border: "none",
          borderRadius: 6,
          display: "grid",
          placeItems: "center",
          background: hover ? "rgba(255,255,255,0.06)" : "transparent",
          color: hover ? "rgba(255,160,160,0.9)" : "rgba(255,255,255,0.0)",
          cursor: "pointer",
          padding: 0,
          transition: "color 0.14s ease, background 0.14s ease",
        }}
      >
        <Trash2 size={12} strokeWidth={1.8} />
      </button>
    </div>
  );
}

function UserPanel({
  user,
  onSettings,
}: {
  user: HomeData["user"];
  onSettings: () => void;
}) {
  return (
    <footer
      style={{
        padding: "10px 12px 14px",
        borderTop: "1px solid rgba(255,255,255,0.06)",
        marginTop: "auto",
      }}
    >
      <div
        style={{
          ...innerGlass,
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "9px 11px",
          borderRadius: 10,
        }}
      >
        <Avatar initials={initials(user.name)} />
        <div style={{ flex: 1, minWidth: 0 }}>
          <div
            style={{
              fontSize: 13,
              fontWeight: 500,
              color: "rgba(255,255,255,0.92)",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {user.name}
          </div>
          <div
            style={{
              fontSize: 11,
              color: "rgba(255,255,255,0.45)",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {user.email}
          </div>
        </div>
        <a
          title="Settings"
          href="/settings"
          onClick={(e) => {
            e.preventDefault();
            onSettings();
          }}
          style={{
            ...iconButtonStyle,
            width: 28,
            height: 28,
            textDecoration: "none",
            color: "rgba(255,255,255,0.58)",
          }}
        >
          <Settings size={13} strokeWidth={1.8} />
        </a>
        <a
          title="Sign out"
          href="/usr/logout"
          style={{
            ...iconButtonStyle,
            width: 28,
            height: 28,
            textDecoration: "none",
            color: "rgba(255,255,255,0.58)",
          }}
        >
          <LogOut size={13} strokeWidth={1.8} />
        </a>
      </div>
    </footer>
  );
}

function Avatar({ initials }: { initials: string }) {
  return (
    <div
      style={{
        ...smallPillGlass,
        width: 32,
        height: 32,
        borderRadius: 8,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        fontWeight: 600,
        fontSize: 12.5,
        color: "#FFFAEE",
        letterSpacing: "0.02em",
        flexShrink: 0,
      }}
    >
      {initials}
    </div>
  );
}

function initials(name: string) {
  const value = name
    .split(/\s+/)
    .filter(Boolean)
    .map((part) => part[0])
    .join("")
    .slice(0, 2)
    .toUpperCase();
  return value || "U";
}

function relativeTime(value: string) {
  const ms = Date.now() - new Date(value).getTime();
  const minutes = Math.max(0, Math.floor(ms / 60000));
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}
