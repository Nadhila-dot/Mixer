import { type CSSProperties, type ReactNode, Suspense, lazy, useEffect, useMemo, useRef, useState } from "react";
import {
  Braces,
  BriefcaseBusiness,
  Check,
  ChevronDown,
  ChevronRight,
  Code,
  Code2,
  ExternalLink,
  Eye,
  File,
  FileCode,
  FileText,
  Folder,
  FolderOpen,
  GitCompareArrows,
  Image as ImageIcon,
  Monitor,
  MoreHorizontal,
  Palette,
  Pencil,
  RefreshCw,
  RotateCcw,
  Save,
  Share2,
  Smartphone,
  Tablet,
  Terminal,
  X,
} from "lucide-react";
import { PatchDiff } from "@pierre/diffs/react";
import {
  FONT,
  iconBtnGlass,
  innerGlass,
  outerGlass,
  pillGlass,
  smallPillGlass,
} from "../lib/glass";
const LazyHighlightedCodeBlock = lazy(() => import("./HighlightedCodeBlock"));

export interface WorkspaceFile {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  children?: WorkspaceFile[];
}

export interface WorkspaceCommand {
  command: string;
  output: string;
  exitCode: number;
  timestamp: number;
}

export interface WorkspaceState {
  id: string;
  name: string;
  fileTree: WorkspaceFile[];
  commands: WorkspaceCommand[];
  activeFile: string | null;
  activeFileContent: string | null;
  previewPath?: string | null;
}

interface Props {
  workspace: WorkspaceState;
  onClose: () => void;
  embedded?: boolean;
  width?: number;
  resizing?: boolean;
}

type Tab = "editor" | "preview" | "terminal" | "diff";
type ViewMode = "view" | "edit";
type DeviceSize = "mobile" | "tablet" | "desktop";

type Snapshot = {
  path: string;
  content: string;
  savedAt: number;
};

const DEVICE_WIDTHS: Record<DeviceSize, number | null> = {
  mobile: 390,
  tablet: 820,
  desktop: null,
};

export default function WorkspacePanel({ workspace, onClose, embedded = false, width, resizing = false }: Props) {
  const [liveFileTree, setLiveFileTree] = useState<WorkspaceFile[]>(workspace.fileTree);
  const firstFile = useMemo(() => firstEditableFile(liveFileTree), [liveFileTree]);
  const [selectedFile, setSelectedFile] = useState<string | null>(workspace.previewPath ?? firstFile);
  const [activeTab, setActiveTab] = useState<Tab>("editor");
  const [editorContent, setEditorContent] = useState("");
  const [lastSavedContent, setLastSavedContent] = useState("");
  const [isDirty, setIsDirty] = useState(false);
  const [loadingFile, setLoadingFile] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [viewMode, setViewMode] = useState<ViewMode>("view");
  const [railCollapsed, setRailCollapsed] = useState(false);
  const [deviceSize, setDeviceSize] = useState<DeviceSize>("desktop");
  const [lastSavedAt, setLastSavedAt] = useState<number | null>(null);
  const [savedTick, setSavedTick] = useState(0);
  const [copyHint, setCopyHint] = useState<string | null>(null);

  const firstPreviewable = useMemo(() => findPreviewablePath(liveFileTree), [liveFileTree]);
  const previewPath = workspace.previewPath
    ?? (selectedFile && isPreviewable(selectedFile) ? selectedFile : null)
    ?? firstPreviewable
    ?? null;
  const previewBlocked = !previewPath;
  const snapshot = selectedFile ? latestSnapshot(workspace.id, selectedFile) : null;
  const language = useMemo(() => languageForPath(selectedFile), [selectedFile]);
  const fileSize = useMemo(() => sizeForPath(liveFileTree, selectedFile), [liveFileTree, selectedFile]);

  // Track which (workspace, path, size) we last fetched so we can detect changes from the agent.
  const lastFetchedRef = useRef<{ id: string; path: string; size: number | null } | null>(null);

  useEffect(() => {
    setLiveFileTree(workspace.fileTree);
  }, [workspace.fileTree]);

  useEffect(() => {
    if (!selectedFile && firstFile) setSelectedFile(firstFile);
  }, [firstFile, selectedFile]);

  useEffect(() => {
    if (workspace.previewPath) {
      setSelectedFile(workspace.previewPath);
      setActiveTab("preview");
    }
  }, [workspace.previewPath]);

  // Reset view-mode whenever the user navigates to a different file.
  useEffect(() => {
    setViewMode("view");
  }, [workspace.id, selectedFile]);

  // Load file content. Re-fires when:
  //   • a different file is selected
  //   • the same file's size on disk changes (agent wrote/appended/patched)
  // Skips refetch while the user has unsaved edits so we don't clobber typing.
  useEffect(() => {
    if (!selectedFile) return;
    if (isDirty) return;

    const last = lastFetchedRef.current;
    const sameTarget = last && last.id === workspace.id && last.path === selectedFile;
    const sizeUnchanged = sameTarget && last.size === fileSize;
    if (sameTarget && sizeUnchanged) return;

    const isInitialLoad = !sameTarget;

    let cancelled = false;
    if (isInitialLoad) setLoadingFile(true);
    setError(null);
    void loadWorkspaceFile(workspace.id, selectedFile)
      .then((content) => {
        if (cancelled) return;
        setEditorContent(content);
        setLastSavedContent(content);
        lastFetchedRef.current = { id: workspace.id, path: selectedFile, size: fileSize };
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : "Could not load file");
      })
      .finally(() => {
        if (!cancelled && isInitialLoad) setLoadingFile(false);
      });
    return () => {
      cancelled = true;
    };
  }, [workspace.id, selectedFile, fileSize, isDirty]);

  useEffect(() => {
    if (!selectedFile) return;
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(`${protocol}//${window.location.host}/api/workspaces/${encodeURIComponent(workspace.id)}/ws`);
    socket.onopen = () => {
      socket.send(JSON.stringify({ path: selectedFile }));
    };
    socket.onmessage = (event) => {
      const message = parseWorkspaceSocketMessage(event.data);
      if (!message) return;
      if (Array.isArray(message.fileTree)) setLiveFileTree(message.fileTree);
      if (!isDirty && message.path === selectedFile && typeof message.content === "string") {
        setEditorContent(message.content);
        setLastSavedContent(message.content);
      }
    };
    return () => socket.close();
  }, [workspace.id, selectedFile, isDirty]);

  // Keep saved-relative-time ticking once a second.
  useEffect(() => {
    if (!lastSavedAt) return;
    const id = window.setInterval(() => setSavedTick((t) => t + 1), 1000);
    return () => window.clearInterval(id);
  }, [lastSavedAt]);

  async function saveFile() {
    if (!selectedFile || !isDirty) return;
    setError(null);
    storeSnapshot(workspace.id, selectedFile, lastSavedContent);
    try {
      await saveWorkspaceFile(workspace.id, selectedFile, editorContent);
      setLastSavedContent(editorContent);
      setIsDirty(false);
      setLastSavedAt(Date.now());
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not save file");
    }
  }

  function rollbackFile() {
    if (!snapshot) return;
    setEditorContent(snapshot.content);
    setIsDirty(snapshot.content !== lastSavedContent);
    setActiveTab("diff");
  }

  // ⌘S / Ctrl-S → save
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      const isSave = (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s";
      if (!isSave) return;
      event.preventDefault();
      void saveFile();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedFile, editorContent, isDirty, lastSavedContent]);

  const previewSrc = previewPath
    ? `/api/workspaces/${encodeURIComponent(workspace.id)}/preview?path=${encodeURIComponent(previewPath)}`
    : null;

  const computedPanelStyle: CSSProperties = embedded
    ? {
        ...embeddedPanelStyle,
        ...(width !== undefined ? { flex: `0 0 ${width}px` } : null),
        ...(resizing ? { transition: "none", animation: "none" } : null),
      }
    : panelStyle;

  return (
    <div style={computedPanelStyle}>
      <style>{LOCAL_KEYFRAMES}</style>

      {/* ── Top Toolbar ─────────────────────────────────────────── */}
      <header style={{ ...toolbarStyle, animation: "content-fade 0.32s ease-out 0ms both" }}>
        <div style={toolbarLeftStyle}>
          <div style={projectTileStyle}>
            <BriefcaseBusiness size={14} strokeWidth={1.8} />
          </div>
          <div style={{ minWidth: 0 }}>
            <div style={projectNameStyle} title={workspace.name}>{workspace.name}</div>
            <div style={projectIdStyle} title={workspace.id}>{workspace.id}</div>
          </div>
        </div>

        <div style={toolbarCenterStyle}>
          <SegmentedTabs
            active={activeTab}
            onChange={setActiveTab}
            options={[
              { value: "editor", label: "Code", Icon: Code2 },
              { value: "preview", label: "Preview", Icon: Eye },
              { value: "terminal", label: "Console", Icon: Terminal },
              { value: "diff", label: "Diff", Icon: GitCompareArrows },
            ]}
          />
        </div>

        <div style={toolbarRightStyle}>
          <ToolbarIconBtn
            title="Refresh preview"
            onClick={() => {
              if (selectedFile) {
                setEditorContent((c) => c);
                setSavedTick((t) => t + 1);
              }
            }}
          >
            <RefreshCw size={14} strokeWidth={1.8} />
          </ToolbarIconBtn>
          <ToolbarIconBtn
            title={previewSrc ? "Open preview in new tab" : "No previewable file in workspace"}
            onClick={() => previewSrc && window.open(previewSrc, "_blank")}
          >
            <ExternalLink size={14} strokeWidth={1.8} />
          </ToolbarIconBtn>
          <ToolbarIconBtn
            title={previewSrc ? "Copy workspace URL" : "No previewable file in workspace"}
            onClick={() => {
              if (!previewSrc) return;
              void navigator.clipboard?.writeText(window.location.origin + previewSrc);
              setCopyHint("URL copied");
              window.setTimeout(() => setCopyHint(null), 1400);
            }}
          >
            <Share2 size={14} strokeWidth={1.8} />
          </ToolbarIconBtn>
          <ToolbarIconBtn title="More" onClick={() => {}}>
            <MoreHorizontal size={14} strokeWidth={1.8} />
          </ToolbarIconBtn>
          <span style={toolbarDividerStyle} />
          <ToolbarIconBtn title="Close workspace" onClick={onClose}>
            <X size={14} strokeWidth={1.9} />
          </ToolbarIconBtn>
        </div>
      </header>

      {copyHint && <div style={copyHintStyle}>{copyHint}</div>}

      {/* ── Body Grid ──────────────────────────────────────────── */}
      <div style={mainStyle(railCollapsed)}>
        {/* Left rail */}
        <aside style={{ ...explorerStyle, animation: "content-fade 0.32s ease-out 80ms both" }}>
          <div style={railHeaderStyle}>
            {!railCollapsed && <span>Explorer</span>}
            <button
              type="button"
              onClick={() => setRailCollapsed((v) => !v)}
              style={railToggleStyle}
              title={railCollapsed ? "Expand explorer" : "Collapse explorer"}
            >
              {railCollapsed ? <ChevronRight size={12} /> : <ChevronDown size={12} />}
            </button>
          </div>
          {liveFileTree.length === 0 ? (
            <EmptyState icon={<FolderOpen size={18} />} label={railCollapsed ? "" : "Empty workspace"} />
          ) : (
            <FileTree
              entries={liveFileTree}
              selected={selectedFile}
              onSelect={(path) => {
                setSelectedFile(path);
                setActiveTab(isPreviewable(path) ? "preview" : "editor");
              }}
              depth={0}
              collapsed={railCollapsed}
            />
          )}
        </aside>

        {/* Workbench */}
        <section style={{ ...workbenchStyle, animation: "content-fade 0.32s ease-out 140ms both" }}>
          {/* Sub-header strip */}
          <div style={subHeaderStyle}>
            <div style={subHeaderLeftStyle}>
              {selectedFile ? (
                <>
                  <span style={crumbStyle} title={selectedFile}>{selectedFile}</span>
                  <span style={smallPillGlass as CSSProperties}>
                    <span style={languagePillTextStyle}>{language.toUpperCase()}</span>
                  </span>
                </>
              ) : (
                <span style={{ ...crumbStyle, opacity: 0.5 }}>No file selected</span>
              )}
            </div>
            <div style={subHeaderRightStyle}>
              {activeTab === "editor" && selectedFile && (
                <button
                  type="button"
                  onClick={() => setViewMode((m) => (m === "view" ? "edit" : "view"))}
                  style={modeTogglePillStyle(viewMode === "edit")}
                  title={viewMode === "view" ? "Edit file" : "Preview rendered code"}
                >
                  {viewMode === "view" ? <Pencil size={12} strokeWidth={1.8} /> : <Eye size={12} strokeWidth={1.8} />}
                  {viewMode === "view" ? "Edit" : "View"}
                </button>
              )}
              {selectedFile && (
                <SavedIndicator isDirty={isDirty} savedAt={lastSavedAt} tick={savedTick} />
              )}
            </div>
          </div>

          {error && <div style={errorStyle}>{error}</div>}

          {/* Content area */}
          <div style={contentStyle}>
            {activeTab === "editor" && (
              <EditorPane
                content={editorContent}
                loading={loadingFile}
                language={language}
                mode={viewMode}
                onChange={(value) => {
                  setEditorContent(value);
                  setIsDirty(value !== lastSavedContent);
                }}
                disabled={!selectedFile}
              />
            )}
            {activeTab === "preview" && (
              previewBlocked || !previewSrc || !previewPath ? (
                <PreviewEmpty
                  selectedFile={selectedFile}
                  onPickPreviewable={(path) => {
                    setSelectedFile(path);
                  }}
                  firstPreviewable={firstPreviewable}
                />
              ) : (
                <PreviewPane
                  key={`${workspace.id}:${previewPath}:${lastSavedContent.length}:${savedTick}`}
                  src={previewSrc}
                  workspaceId={workspace.id}
                  path={previewPath}
                  fallbackHint={
                    selectedFile && selectedFile !== previewPath
                      ? `Showing ${previewPath} — '${selectedFile}' isn't a previewable file type.`
                      : null
                  }
                  deviceSize={deviceSize}
                  onDeviceChange={setDeviceSize}
                  onRefresh={() => setSavedTick((t) => t + 1)}
                  onExternal={() => window.open(previewSrc, "_blank")}
                />
              )
            )}
            {activeTab === "terminal" && <TerminalPane commands={workspace.commands} />}
            {activeTab === "diff" && (
              <DiffPane
                path={selectedFile ?? "workspace-file"}
                before={snapshot?.content ?? lastSavedContent}
                after={editorContent}
                snapshotAt={snapshot?.savedAt}
                onRollback={rollbackFile}
                canRollback={!!snapshot}
              />
            )}
          </div>

          {/* Status bar */}
          <StatusBar
            path={selectedFile}
            size={fileSize}
            isDirty={isDirty}
            savedAt={lastSavedAt}
            tick={savedTick}
            language={language}
            onSave={() => void saveFile()}
            canSave={isDirty && !!selectedFile}
          />
        </section>
      </div>
    </div>
  );
}

/* ─────────────────────────────────────────────────────────────────── */
/*  Sub-components                                                      */
/* ─────────────────────────────────────────────────────────────────── */

function SegmentedTabs({
  active,
  onChange,
  options,
}: {
  active: Tab;
  onChange: (tab: Tab) => void;
  options: { value: Tab; label: string; Icon: typeof Code2 }[];
}) {
  return (
    <div style={segmentedTrackStyle}>
      {options.map(({ value, label, Icon }) => {
        const isActive = active === value;
        return (
          <button
            key={value}
            type="button"
            onClick={() => onChange(value)}
            style={isActive ? segmentedActiveStyle : segmentedStyle}
            onMouseEnter={(e) => {
              if (!isActive) (e.currentTarget as HTMLButtonElement).style.color = "rgba(255,255,255,0.80)";
            }}
            onMouseLeave={(e) => {
              if (!isActive) (e.currentTarget as HTMLButtonElement).style.color = "rgba(255,255,255,0.50)";
            }}
          >
            <Icon size={12.5} strokeWidth={1.8} />
            {label}
          </button>
        );
      })}
    </div>
  );
}

function ToolbarIconBtn({
  children,
  title,
  onClick,
}: {
  children: ReactNode;
  title?: string;
  onClick?: () => void;
}) {
  return (
    <button
      type="button"
      title={title}
      onClick={onClick}
      style={toolbarIconButtonStyle}
      onMouseEnter={(e) => {
        (e.currentTarget as HTMLButtonElement).style.color = "rgba(255,255,255,0.95)";
        (e.currentTarget as HTMLButtonElement).style.transform = "translateY(-1px)";
      }}
      onMouseLeave={(e) => {
        (e.currentTarget as HTMLButtonElement).style.color = "rgba(255,255,255,0.62)";
        (e.currentTarget as HTMLButtonElement).style.transform = "translateY(0)";
      }}
    >
      {children}
    </button>
  );
}

function SavedIndicator({
  isDirty,
  savedAt,
  tick,
}: {
  isDirty: boolean;
  savedAt: number | null;
  tick: number;
}) {
  void tick; // forces re-render each second
  if (isDirty) {
    return (
      <span style={savedChipStyle}>
        <span style={{ ...savedDotStyle, background: "rgba(245, 190, 90, 0.95)" }} />
        <span style={{ color: "rgba(245, 190, 90, 0.95)" }}>Unsaved</span>
      </span>
    );
  }
  if (savedAt) {
    return (
      <span style={savedChipStyle}>
        <span style={{ ...savedDotStyle, background: "rgba(120, 220, 140, 0.95)" }} />
        <span style={{ color: "rgba(255,255,255,0.55)" }}>Saved {formatRelative(savedAt)}</span>
      </span>
    );
  }
  return (
    <span style={savedChipStyle}>
      <span style={{ ...savedDotStyle, background: "rgba(255,255,255,0.30)" }} />
      <span style={{ color: "rgba(255,255,255,0.40)" }}>Clean</span>
    </span>
  );
}

function FileTree({
  entries,
  selected,
  onSelect,
  depth,
  collapsed,
}: {
  entries: WorkspaceFile[];
  selected: string | null;
  onSelect: (path: string) => void;
  depth: number;
  collapsed: boolean;
}) {
  return (
    <>
      {entries.map((entry) => (
        <FileTreeEntry
          key={entry.path}
          entry={entry}
          selected={selected}
          onSelect={onSelect}
          depth={depth}
          collapsed={collapsed}
        />
      ))}
    </>
  );
}

function FileTreeEntry({
  entry,
  selected,
  onSelect,
  depth,
  collapsed,
}: {
  entry: WorkspaceFile;
  selected: string | null;
  onSelect: (path: string) => void;
  depth: number;
  collapsed: boolean;
}) {
  const [open, setOpen] = useState(depth === 0);
  const isSelected = selected === entry.path;
  const meta = entry.is_dir ? null : iconForFile(entry.name);

  return (
    <>
      <button
        type="button"
        onClick={() => {
          if (entry.is_dir) setOpen((value) => !value);
          else onSelect(entry.path);
        }}
        style={{
          ...treeRowStyle,
          paddingLeft: collapsed ? 12 : 14 + depth * 14,
          background: isSelected ? "rgba(255,255,255,0.09)" : "transparent",
          color: isSelected ? "rgba(255,255,255,0.96)" : "rgba(255,255,255,0.62)",
          borderLeft: isSelected ? "2px solid rgba(255,255,255,0.45)" : "2px solid transparent",
          fontWeight: isSelected ? 500 : 400,
        }}
        onMouseEnter={(e) => {
          if (!isSelected) (e.currentTarget as HTMLButtonElement).style.background = "rgba(255,255,255,0.04)";
        }}
        onMouseLeave={(e) => {
          if (!isSelected) (e.currentTarget as HTMLButtonElement).style.background = "transparent";
        }}
        title={collapsed ? entry.name : undefined}
      >
        {entry.is_dir ? (
          <>
            {!collapsed && (open ? <ChevronDown size={12} /> : <ChevronRight size={12} />)}
            {open ? <FolderOpen size={13} style={{ color: "rgba(245, 210, 120, 0.85)" }} /> : <Folder size={13} style={{ color: "rgba(245, 210, 120, 0.85)" }} />}
          </>
        ) : (
          <>
            {!collapsed && <span style={{ width: 12, flexShrink: 0 }} />}
            {meta && <meta.Icon size={13} style={{ color: meta.color, flexShrink: 0 }} />}
          </>
        )}
        {!collapsed && (
          <>
            <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{entry.name}</span>
            {!entry.is_dir && entry.size > 0 && <span style={sizeStyle}>{formatSize(entry.size)}</span>}
          </>
        )}
      </button>
      {entry.is_dir && open && entry.children && !collapsed && (
        <FileTree entries={entry.children} selected={selected} onSelect={onSelect} depth={depth + 1} collapsed={collapsed} />
      )}
    </>
  );
}

function EditorPane({
  content,
  loading,
  language,
  mode,
  onChange,
  disabled,
}: {
  content: string;
  loading: boolean;
  language: string;
  mode: ViewMode;
  onChange: (value: string) => void;
  disabled: boolean;
}) {
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const gutterRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<HTMLDivElement | null>(null);

  const lineCount = useMemo(() => {
    if (loading) return 1;
    const lines = content.split("\n");
    if (lines[lines.length - 1] === "" && lines.length > 1) lines.pop();
    return Math.max(lines.length, 1);
  }, [content, loading]);

  // Scroll-sync the gutter with the body
  useEffect(() => {
    const body = mode === "edit" ? textareaRef.current : viewRef.current;
    const gutter = gutterRef.current;
    if (!body || !gutter) return;
    const sync = () => {
      gutter.scrollTop = body.scrollTop;
    };
    body.addEventListener("scroll", sync, { passive: true });
    return () => body.removeEventListener("scroll", sync);
  }, [mode, content]);

  return (
    <div style={editorWrapStyle}>
      <div ref={gutterRef} style={gutterStyle}>
        {Array.from({ length: lineCount }).map((_, i) => (
          <div key={i} style={gutterLineStyle}>{i + 1}</div>
        ))}
      </div>
      {mode === "edit" ? (
        <textarea
          ref={textareaRef}
          value={loading ? "Loading file..." : content}
          disabled={disabled || loading}
          spellCheck={false}
          onChange={(e) => onChange(e.target.value)}
          style={editorTextareaStyle}
        />
      ) : (
        <div ref={viewRef} style={editorViewStyle}>
          {loading ? (
            <div style={editorPlaceholderStyle}>Loading file...</div>
          ) : (
            <Suspense fallback={<div style={editorPlaceholderStyle}>Rendering...</div>}>
              <LazyHighlightedCodeBlock code={content} language={language} />
            </Suspense>
          )}
        </div>
      )}
    </div>
  );
}

function PreviewPane({
  src,
  workspaceId,
  path,
  fallbackHint,
  deviceSize,
  onDeviceChange,
  onRefresh,
  onExternal,
}: {
  src: string;
  workspaceId: string;
  path: string;
  fallbackHint: string | null;
  deviceSize: DeviceSize;
  onDeviceChange: (size: DeviceSize) => void;
  onRefresh: () => void;
  onExternal: () => void;
}) {
  const deviceWidth = DEVICE_WIDTHS[deviceSize];
  return (
    <div style={previewWrapStyle}>
      {fallbackHint && <div style={fallbackHintStyle}>{fallbackHint}</div>}
      <div style={previewChromeStyle}>
        <button type="button" onClick={onRefresh} style={chromeIconBtnStyle} title="Refresh preview">
          <RefreshCw size={13} strokeWidth={1.8} />
        </button>
        <div style={urlPillStyle}>
          <span style={{ opacity: 0.45 }}>localhost</span>
          <span style={{ opacity: 0.30, margin: "0 6px" }}>/</span>
          <span style={{ opacity: 0.55 }}>{workspaceId}</span>
          <span style={{ opacity: 0.30, margin: "0 6px" }}>/</span>
          <span style={{ color: "rgba(255,255,255,0.85)" }}>{path}</span>
        </div>
        <div style={deviceGroupStyle}>
          <DeviceBtn active={deviceSize === "mobile"} onClick={() => onDeviceChange("mobile")} title="Mobile (390px)">
            <Smartphone size={12.5} strokeWidth={1.8} />
          </DeviceBtn>
          <DeviceBtn active={deviceSize === "tablet"} onClick={() => onDeviceChange("tablet")} title="Tablet (820px)">
            <Tablet size={12.5} strokeWidth={1.8} />
          </DeviceBtn>
          <DeviceBtn active={deviceSize === "desktop"} onClick={() => onDeviceChange("desktop")} title="Desktop (full)">
            <Monitor size={12.5} strokeWidth={1.8} />
          </DeviceBtn>
        </div>
        <button type="button" onClick={onExternal} style={chromeIconBtnStyle} title="Open in new tab">
          <ExternalLink size={13} strokeWidth={1.8} />
        </button>
      </div>
      <div style={previewStageStyle}>
        <iframe
          title="Workspace preview"
          sandbox="allow-scripts allow-forms allow-modals"
          src={src}
          style={{
            ...previewIframeStyle,
            maxWidth: deviceWidth ?? "100%",
            width: deviceWidth ? `${deviceWidth}px` : "100%",
          }}
        />
      </div>
    </div>
  );
}

function PreviewEmpty({
  selectedFile,
  firstPreviewable,
  onPickPreviewable,
}: {
  selectedFile: string | null;
  firstPreviewable: string | null;
  onPickPreviewable: (path: string) => void;
}) {
  return (
    <div style={previewEmptyStyle}>
      <div style={emptyIconStyle}>
        <Eye size={20} />
      </div>
      <div style={{ fontSize: 13, color: "rgba(255,255,255,0.70)", fontWeight: 500 }}>
        Preview not available
      </div>
      <div style={{ fontSize: 12, color: "rgba(255,255,255,0.42)", maxWidth: 360, textAlign: "center" }}>
        {selectedFile
          ? `'${selectedFile}' isn't a previewable file. Preview works for .html, .htm, and .svg files.`
          : "No previewable file selected. Preview works for .html, .htm, and .svg files."}
      </div>
      {firstPreviewable && (
        <button
          type="button"
          onClick={() => onPickPreviewable(firstPreviewable)}
          style={{
            ...pillGlass,
            display: "inline-flex",
            alignItems: "center",
            gap: 6,
            padding: "6px 12px",
            borderRadius: 8,
            color: "rgba(255,255,255,0.88)",
            cursor: "pointer",
            fontFamily: FONT,
            fontSize: 12,
            border: "none",
            marginTop: 4,
          }}
        >
          <Eye size={12} strokeWidth={1.8} />
          Preview {firstPreviewable}
        </button>
      )}
    </div>
  );
}

function DeviceBtn({
  active,
  onClick,
  title,
  children,
}: {
  active: boolean;
  onClick: () => void;
  title: string;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      style={active ? deviceBtnActiveStyle : deviceBtnStyle}
      onMouseEnter={(e) => {
        if (!active) (e.currentTarget as HTMLButtonElement).style.color = "rgba(255,255,255,0.78)";
      }}
      onMouseLeave={(e) => {
        if (!active) (e.currentTarget as HTMLButtonElement).style.color = "rgba(255,255,255,0.45)";
      }}
    >
      {children}
    </button>
  );
}

function TerminalPane({ commands }: { commands: WorkspaceCommand[] }) {
  if (commands.length === 0) {
    return <EmptyState icon={<Terminal size={20} />} label="No commands run yet" />;
  }
  return (
    <div style={terminalWrapStyle}>
      {commands.map((cmd, i) => (
        <CommandCard key={i} cmd={cmd} />
      ))}
    </div>
  );
}

function CommandCard({ cmd }: { cmd: WorkspaceCommand }) {
  const [expanded, setExpanded] = useState(false);
  const ok = cmd.exitCode === 0;
  const hasOutput = cmd.output.length > 0;
  return (
    <div style={commandCardStyle}>
      <button
        type="button"
        onClick={() => hasOutput && setExpanded((e) => !e)}
        style={commandHeaderStyle}
      >
        <span
          style={{
            ...statusDotStyle,
            background: ok ? "rgba(120, 220, 140, 0.95)" : "rgba(255, 120, 120, 0.95)",
          }}
        />
        <span style={commandTextStyle}>$ {cmd.command}</span>
        <span style={{ ...statusLabelStyle, color: ok ? "rgba(120,220,140,0.90)" : "rgba(255,120,120,0.95)" }}>
          exit {cmd.exitCode}
        </span>
        {hasOutput && (
          <ChevronDown
            size={13}
            style={{
              color: "rgba(255,255,255,0.40)",
              transform: expanded ? "rotate(180deg)" : "rotate(0deg)",
              transition: "transform 0.16s ease",
              flexShrink: 0,
            }}
          />
        )}
      </button>
      {expanded && hasOutput && <pre style={commandOutputStyle}>{cmd.output}</pre>}
    </div>
  );
}

function DiffPane({
  path,
  before,
  after,
  snapshotAt,
  onRollback,
  canRollback,
}: {
  path: string;
  before: string;
  after: string;
  snapshotAt: number | undefined;
  onRollback: () => void;
  canRollback: boolean;
}) {
  const patch = makeUnifiedPatch(path, before, after);
  return (
    <div style={diffWrapStyle}>
      <div style={diffHeaderStyle}>
        <div style={{ display: "flex", alignItems: "center", gap: 10, minWidth: 0 }}>
          <GitCompareArrows size={13} strokeWidth={1.8} style={{ color: "rgba(255,255,255,0.55)" }} />
          <span style={diffHeaderTextStyle}>
            {snapshotAt
              ? `Snapshot ${formatRelative(snapshotAt)} ↔ Working copy`
              : "No snapshot · Working copy only"}
          </span>
        </div>
        <button
          type="button"
          onClick={onRollback}
          disabled={!canRollback}
          style={{ ...rollbackButtonStyle, opacity: canRollback ? 1 : 0.35, cursor: canRollback ? "pointer" : "not-allowed" }}
          title="Restore last manual-save snapshot"
        >
          <RotateCcw size={12} strokeWidth={1.8} />
          Restore snapshot
        </button>
      </div>
      <div style={diffBodyStyle}>
        {!patch ? (
          <EmptyState icon={<GitCompareArrows size={20} />} label="No local edits to diff" />
        ) : (
          <PatchDiff
            patch={patch}
            disableWorkerPool
            options={{
              diffStyle: "split",
              theme: { dark: "github-dark", light: "github-light" },
              themeType: "dark",
              overflow: "wrap",
            }}
            style={pierreDiffStyle}
          />
        )}
      </div>
    </div>
  );
}

function StatusBar({
  path,
  size,
  isDirty,
  savedAt,
  tick,
  language,
  onSave,
  canSave,
}: {
  path: string | null;
  size: number | null;
  isDirty: boolean;
  savedAt: number | null;
  tick: number;
  language: string;
  onSave: () => void;
  canSave: boolean;
}) {
  void tick;
  return (
    <footer style={statusBarStyle}>
      <div style={statusBarLeftStyle}>
        <File size={11} strokeWidth={1.8} style={{ color: "rgba(255,255,255,0.42)" }} />
        <span style={statusPathStyle}>{path ?? "—"}</span>
        {size !== null && size > 0 && (
          <span style={smallPillGlass as CSSProperties}>
            <span style={languagePillTextStyle}>{formatSize(size)}</span>
          </span>
        )}
      </div>
      <div style={statusBarCenterStyle}>
        <span
          style={{
            ...savedDotStyle,
            background: isDirty
              ? "rgba(245, 190, 90, 0.95)"
              : savedAt
              ? "rgba(120, 220, 140, 0.95)"
              : "rgba(255,255,255,0.25)",
          }}
        />
        <span style={statusSavedTextStyle}>
          {isDirty ? "Unsaved changes" : savedAt ? `Saved ${formatRelative(savedAt)}` : "No changes"}
        </span>
      </div>
      <div style={statusBarRightStyle}>
        <button
          type="button"
          onClick={onSave}
          disabled={!canSave}
          style={{ ...saveShortcutStyle, opacity: canSave ? 1 : 0.45, cursor: canSave ? "pointer" : "default" }}
          title="Save file"
        >
          {canSave ? <Save size={11} strokeWidth={1.9} /> : <Check size={11} strokeWidth={2} />}
          <span style={{ fontFamily: "'DM Mono', ui-monospace, monospace" }}>⌘S</span>
          <span style={{ opacity: 0.55 }}>{canSave ? "save" : "saved"}</span>
        </button>
        <span style={smallPillGlass as CSSProperties}>
          <span style={languagePillTextStyle}>{language.toUpperCase()}</span>
        </span>
      </div>
    </footer>
  );
}

function EmptyState({ icon, label }: { icon: ReactNode; label: string }) {
  return (
    <div style={emptyStateStyle}>
      <div style={emptyIconStyle}>{icon}</div>
      {label && <div>{label}</div>}
    </div>
  );
}

/* ─────────────────────────────────────────────────────────────────── */
/*  Helpers                                                             */
/* ─────────────────────────────────────────────────────────────────── */

function makeUnifiedPatch(path: string, before: string, after: string) {
  if (before === after) return "";
  const oldLines = splitPatchLines(before);
  const newLines = splitPatchLines(after);
  const oldStart = oldLines.length === 0 ? 0 : 1;
  const newStart = newLines.length === 0 ? 0 : 1;
  const header = [
    `diff --git a/${path} b/${path}`,
    `--- a/${path}`,
    `+++ b/${path}`,
    `@@ -${oldStart},${oldLines.length} +${newStart},${newLines.length} @@`,
  ];
  return [
    ...header,
    ...oldLines.map((line) => `-${line}`),
    ...newLines.map((line) => `+${line}`),
    "",
  ].join("\n");
}

function splitPatchLines(content: string) {
  if (content.length === 0) return [];
  const lines = content.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

async function loadWorkspaceFile(workspaceId: string, path: string) {
  const response = await fetch(`/api/workspaces/${encodeURIComponent(workspaceId)}/file?path=${encodeURIComponent(path)}`, {
    credentials: "same-origin",
  });
  const payload = await response.json().catch(() => ({}));
  if (!response.ok || !payload.ok) throw new Error(payload.error || "Could not load file");
  return String(payload.content ?? "");
}

async function saveWorkspaceFile(workspaceId: string, path: string, content: string) {
  const response = await fetch(`/api/workspaces/${encodeURIComponent(workspaceId)}/file`, {
    method: "PUT",
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path, content }),
  });
  const payload = await response.json().catch(() => ({}));
  if (!response.ok || !payload.ok) throw new Error(payload.error || "Could not save file");
}

function parseWorkspaceSocketMessage(raw: string): { fileTree: WorkspaceFile[]; path: string; content: string | null } | null {
  try {
    const message = JSON.parse(raw);
    if (message?.event !== "workspace_snapshot") return null;
    return {
      fileTree: Array.isArray(message.data?.file_tree) ? message.data.file_tree : [],
      path: String(message.data?.path ?? ""),
      content: typeof message.data?.content === "string" ? message.data.content : null,
    };
  } catch {
    return null;
  }
}

function firstEditableFile(entries: WorkspaceFile[]): string | null {
  for (const entry of entries) {
    if (!entry.is_dir) return entry.path;
    const child = firstEditableFile(entry.children ?? []);
    if (child) return child;
  }
  return null;
}

function isPreviewable(path: string) {
  return /\.(html?|svg)$/i.test(path);
}

function findPreviewablePath(entries: WorkspaceFile[]): string | null {
  for (const entry of entries) {
    if (!entry.is_dir && isPreviewable(entry.path)) return entry.path;
    if (entry.is_dir && entry.children) {
      const inner = findPreviewablePath(entry.children);
      if (inner) return inner;
    }
  }
  return null;
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)}MB`;
}

function sizeForPath(entries: WorkspaceFile[], path: string | null): number | null {
  if (!path) return null;
  for (const entry of entries) {
    if (entry.path === path) return entry.size;
    if (entry.is_dir && entry.children) {
      const inner = sizeForPath(entry.children, path);
      if (inner !== null) return inner;
    }
  }
  return null;
}

function snapshotKey(workspaceId: string, path: string) {
  return `mixer:workspace:${workspaceId}:snapshots:${path}`;
}

function latestSnapshot(workspaceId: string, path: string): Snapshot | null {
  try {
    const raw = localStorage.getItem(snapshotKey(workspaceId, path));
    const snapshots = raw ? (JSON.parse(raw) as Snapshot[]) : [];
    return snapshots.length > 0 ? snapshots[snapshots.length - 1] : null;
  } catch {
    return null;
  }
}

function storeSnapshot(workspaceId: string, path: string, content: string) {
  try {
    const key = snapshotKey(workspaceId, path);
    const raw = localStorage.getItem(key);
    const snapshots = raw ? (JSON.parse(raw) as Snapshot[]) : [];
    snapshots.push({ path, content, savedAt: Date.now() });
    localStorage.setItem(key, JSON.stringify(snapshots.slice(-25)));
  } catch {}
}

function iconForFile(name: string): { Icon: typeof Code; color: string } {
  const ext = name.split(".").pop()?.toLowerCase();
  switch (ext) {
    case "html":
    case "htm":
      return { Icon: Code, color: "rgba(255, 160, 100, 0.85)" };
    case "css":
    case "scss":
    case "less":
      return { Icon: Palette, color: "rgba(120, 180, 255, 0.85)" };
    case "js":
    case "mjs":
    case "cjs":
      return { Icon: FileCode, color: "rgba(245, 220, 95, 0.85)" };
    case "ts":
    case "tsx":
    case "jsx":
      return { Icon: FileCode, color: "rgba(100, 180, 230, 0.85)" };
    case "json":
      return { Icon: Braces, color: "rgba(160, 200, 130, 0.85)" };
    case "md":
    case "mdx":
      return { Icon: FileText, color: "rgba(220, 220, 220, 0.75)" };
    case "svg":
    case "png":
    case "jpg":
    case "jpeg":
    case "gif":
    case "webp":
      return { Icon: ImageIcon, color: "rgba(220, 160, 220, 0.85)" };
    default:
      return { Icon: File, color: "rgba(255,255,255,0.55)" };
  }
}

function languageForPath(path: string | null): string {
  if (!path) return "plaintext";
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  const map: Record<string, string> = {
    html: "html",
    htm: "html",
    css: "css",
    scss: "scss",
    less: "less",
    js: "javascript",
    mjs: "javascript",
    cjs: "javascript",
    ts: "typescript",
    tsx: "tsx",
    jsx: "jsx",
    json: "json",
    md: "markdown",
    mdx: "markdown",
    svg: "xml",
    xml: "xml",
    yaml: "yaml",
    yml: "yaml",
    py: "python",
    rs: "rust",
    go: "go",
    sh: "bash",
    bash: "bash",
  };
  return map[ext] ?? "plaintext";
}

function formatRelative(ts: number): string {
  const secs = Math.floor((Date.now() - ts) / 1000);
  if (secs < 5) return "just now";
  if (secs < 60) return `${secs}s ago`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  return `${Math.floor(secs / 3600)}h ago`;
}

/* ─────────────────────────────────────────────────────────────────── */
/*  Exported helpers (consumed by ChatPage)                             */
/* ─────────────────────────────────────────────────────────────────── */

export function parseWorkspaceToolResult(
  attrs: Record<string, string>,
  content: string,
): { workspaceId: string; action: string; fileTree: WorkspaceFile[]; commandInfo: WorkspaceCommand | null; previewPath: string | null } | null {
  const workspaceId = attrs.workspace_id || "";
  const action = attrs.action || "";
  if (!workspaceId) return null;

  let fileTree: WorkspaceFile[] = [];
  try {
    fileTree = JSON.parse(attrs.file_tree || "[]");
  } catch {}

  let commandInfo: WorkspaceCommand | null = null;
  if (action === "Command" || action === "LongRunProcess") {
    const match = content.match(/^\$ (.+)\n([\s\S]*)\nExit: (\d+)/);
    if (match) {
      commandInfo = {
        command: match[1],
        output: match[2].trim(),
        exitCode: parseInt(match[3], 10),
        timestamp: Date.now(),
      };
    }
  }

  const previewPath = action === "Preview"
    ? content.match(/^Preview: (.+)$/m)?.[1]?.trim() ?? null
    : null;

  return { workspaceId, action, fileTree, commandInfo, previewPath };
}

export function applyWorkspaceUpdate(
  current: WorkspaceState | null,
  workspaceId: string,
  workspaceName: string,
  fileTree: WorkspaceFile[],
  commandInfo: WorkspaceCommand | null,
  action: string,
  content: string,
  previewPath: string | null = null,
): WorkspaceState {
  const base: WorkspaceState = current ?? {
    id: workspaceId,
    name: workspaceName,
    fileTree: [],
    commands: [],
    activeFile: null,
    activeFileContent: null,
    previewPath: null,
  };

  const commands = commandInfo ? [...base.commands, commandInfo].slice(-50) : base.commands;
  let activeFile = base.activeFile;
  let activeFileContent = base.activeFileContent;

  if (action === "ReadFile") {
    const pathMatch = content.match(/^File: (.+)\n\n([\s\S]*)$/);
    if (pathMatch) {
      activeFile = pathMatch[1].trim();
      activeFileContent = pathMatch[2];
    }
  }

  return {
    ...base,
    id: workspaceId,
    fileTree: fileTree.length > 0 ? fileTree : base.fileTree,
    commands,
    activeFile,
    activeFileContent,
    previewPath: previewPath ?? base.previewPath,
  };
}

/* ─────────────────────────────────────────────────────────────────── */
/*  Local keyframes (scrollbar, scoped fades)                           */
/* ─────────────────────────────────────────────────────────────────── */

const LOCAL_KEYFRAMES = `
.workspace-scroll::-webkit-scrollbar { width: 6px; height: 6px; }
.workspace-scroll::-webkit-scrollbar-track { background: transparent; }
.workspace-scroll::-webkit-scrollbar-thumb { background: rgba(255,255,255,0.10); border-radius: 999px; }
.workspace-scroll::-webkit-scrollbar-thumb:hover { background: rgba(255,255,255,0.18); }
`;

/* ─────────────────────────────────────────────────────────────────── */
/*  Styles                                                              */
/* ─────────────────────────────────────────────────────────────────── */

const panelStyle: CSSProperties = {
  position: "fixed",
  top: 0,
  right: 0,
  bottom: 0,
  width: "min(980px, 68vw)",
  zIndex: 50,
  display: "flex",
  flexDirection: "column",
  ...outerGlass,
  borderRadius: 0,
  padding: 0,
  borderLeft: "1px solid rgba(255,255,255,0.14)",
  borderTop: "none",
  borderRight: "none",
  borderBottom: "none",
  fontFamily: FONT,
};

const embeddedPanelStyle: CSSProperties = {
  ...outerGlass,
  padding: 0,
  flex: "0 0 min(720px, 52vw)",
  minWidth: 380,
  display: "flex",
  flexDirection: "column",
  overflow: "hidden",
  margin: "4px 10px 14px 4px",
  fontFamily: FONT,
  animation: "workspace-slide-in 0.36s cubic-bezier(0.22,1,0.36,1) both",
};

/* ── Toolbar ───────────────────────────────────────────────────────── */

const toolbarStyle: CSSProperties = {
  position: "relative",
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  height: 56,
  padding: "0 14px",
  borderBottom: "1px solid rgba(255,255,255,0.08)",
  background: [
    "radial-gradient(ellipse 100% 80% at 50% -20%, rgba(255,255,255,0.07) 0%, transparent 100%)",
    "linear-gradient(180deg, rgba(255,255,255,0.02) 0%, transparent 100%)",
  ].join(", "),
  flexShrink: 0,
};

const toolbarLeftStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 10,
  minWidth: 0,
  flex: "0 1 auto",
  zIndex: 1,
};

const projectTileStyle: CSSProperties = {
  ...iconBtnGlass,
  width: 28,
  height: 28,
  borderRadius: 8,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  color: "rgba(245, 210, 120, 0.92)",
  flexShrink: 0,
};

const projectNameStyle: CSSProperties = {
  fontSize: 14,
  fontWeight: 600,
  color: "rgba(255,255,255,0.92)",
  letterSpacing: "-0.005em",
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
  maxWidth: 220,
};

const projectIdStyle: CSSProperties = {
  fontSize: 10.5,
  color: "rgba(255,255,255,0.32)",
  fontFamily: "'DM Mono', ui-monospace, monospace",
  letterSpacing: "0.02em",
  marginTop: 1,
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
  maxWidth: 220,
};

const toolbarCenterStyle: CSSProperties = {
  position: "absolute",
  left: "50%",
  top: "50%",
  transform: "translate(-50%, -50%)",
  display: "flex",
  alignItems: "center",
  zIndex: 0,
};

const toolbarRightStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 4,
  flexShrink: 0,
  zIndex: 1,
};

const toolbarIconButtonStyle: CSSProperties = {
  ...iconBtnGlass,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  width: 30,
  height: 30,
  borderRadius: 8,
  color: "rgba(255,255,255,0.62)",
  padding: 0,
  cursor: "pointer",
  transition: "color 0.15s ease, transform 0.15s ease",
};

const toolbarDividerStyle: CSSProperties = {
  width: 1,
  height: 18,
  background: "rgba(255,255,255,0.10)",
  margin: "0 4px",
};

const copyHintStyle: CSSProperties = {
  position: "absolute",
  top: 60,
  right: 14,
  ...smallPillGlass,
  padding: "5px 11px",
  borderRadius: 99,
  fontSize: 11,
  color: "rgba(255,255,255,0.85)",
  zIndex: 5,
  animation: "content-fade 0.18s ease-out both",
};

/* ── Segmented control (Code/Preview/Console/Diff) ─────────────────── */

const segmentedTrackStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 2,
  padding: 3,
  borderRadius: 10,
  background: "rgba(0,0,0,0.35)",
  border: "1px solid rgba(255,255,255,0.06)",
  boxShadow: "inset 0 1px 0 rgba(0,0,0,0.4), inset 0 -1px 0 rgba(255,255,255,0.04)",
};

const segmentedStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 6,
  border: "1px solid transparent",
  borderRadius: 7,
  padding: "5px 12px",
  background: "transparent",
  color: "rgba(255,255,255,0.50)",
  cursor: "pointer",
  fontFamily: FONT,
  fontSize: 12.5,
  fontWeight: 500,
  letterSpacing: "0.005em",
  transition: "color 0.15s ease, background 0.15s ease",
};

const segmentedActiveStyle: CSSProperties = {
  ...segmentedStyle,
  ...pillGlass,
  borderRadius: 7,
  padding: "5px 12px",
  color: "rgba(255,255,255,0.95)",
};

/* ── Body grid ─────────────────────────────────────────────────────── */

const mainStyle = (railCollapsed: boolean): CSSProperties => ({
  flex: 1,
  minHeight: 0,
  display: "grid",
  gridTemplateColumns: `${railCollapsed ? 44 : 220}px minmax(0, 1fr)`,
  transition: "grid-template-columns 0.24s cubic-bezier(0.22,1,0.36,1)",
});

/* ── File tree rail ────────────────────────────────────────────────── */

const explorerStyle: CSSProperties = {
  minWidth: 0,
  borderRight: "1px solid rgba(255,255,255,0.08)",
  overflowY: "auto",
  padding: "8px 0",
  background: [
    "radial-gradient(ellipse 100% 18% at 50% 0%, rgba(255,255,255,0.04) 0%, transparent 100%)",
    "rgba(0,0,0,0.22)",
  ].join(", "),
};

const railHeaderStyle: CSSProperties = {
  padding: "4px 12px 10px",
  color: "rgba(255,255,255,0.30)",
  fontSize: 10,
  letterSpacing: "0.14em",
  textTransform: "uppercase",
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: 6,
};

const railToggleStyle: CSSProperties = {
  background: "transparent",
  border: "none",
  color: "rgba(255,255,255,0.40)",
  padding: 4,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  cursor: "pointer",
  borderRadius: 5,
};

const treeRowStyle: CSSProperties = {
  width: "100%",
  display: "flex",
  alignItems: "center",
  gap: 7,
  color: "rgba(255,255,255,0.62)",
  padding: "5px 12px 5px 12px",
  background: "transparent",
  cursor: "pointer",
  fontFamily: FONT,
  fontSize: 12.5,
  textAlign: "left" as const,
  transition: "background 0.12s ease, color 0.12s ease",
  borderLeft: "2px solid transparent",
};

const sizeStyle: CSSProperties = {
  marginLeft: "auto",
  fontSize: 10,
  color: "rgba(255,255,255,0.22)",
  flexShrink: 0,
  fontFamily: "'DM Mono', ui-monospace, monospace",
};

/* ── Workbench ─────────────────────────────────────────────────────── */

const workbenchStyle: CSSProperties = {
  minWidth: 0,
  minHeight: 0,
  display: "flex",
  flexDirection: "column",
  background: "rgba(0,0,0,0.14)",
};

const subHeaderStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  padding: "8px 14px",
  borderBottom: "1px solid rgba(255,255,255,0.06)",
  gap: 12,
  flexShrink: 0,
  minHeight: 38,
};

const subHeaderLeftStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 10,
  minWidth: 0,
  flex: 1,
};

const subHeaderRightStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  flexShrink: 0,
};

const crumbStyle: CSSProperties = {
  fontFamily: "'DM Mono', ui-monospace, monospace",
  fontSize: 11,
  color: "rgba(255,255,255,0.62)",
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
  minWidth: 0,
};

const languagePillTextStyle: CSSProperties = {
  fontSize: 9.5,
  letterSpacing: "0.10em",
  padding: "2px 7px",
  color: "rgba(255,255,255,0.72)",
  fontWeight: 500,
  display: "inline-block",
};

const modeTogglePillStyle = (editMode: boolean): CSSProperties => ({
  ...pillGlass,
  display: "inline-flex",
  alignItems: "center",
  gap: 5,
  padding: "4px 10px",
  borderRadius: 7,
  fontFamily: FONT,
  fontSize: 11.5,
  fontWeight: 500,
  color: editMode ? "rgba(245, 190, 90, 0.95)" : "rgba(255,255,255,0.78)",
  cursor: "pointer",
  border: "none",
});

const savedChipStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 6,
  fontSize: 11,
  fontFamily: FONT,
};

const savedDotStyle: CSSProperties = {
  width: 6,
  height: 6,
  borderRadius: 999,
  flexShrink: 0,
  boxShadow: "inset 0 1px 0 rgba(255,255,255,0.40)",
};

const contentStyle: CSSProperties = {
  flex: 1,
  minHeight: 0,
  display: "flex",
};

/* ── Editor pane ───────────────────────────────────────────────────── */

const editorWrapStyle: CSSProperties = {
  flex: 1,
  display: "flex",
  minHeight: 0,
  minWidth: 0,
  position: "relative",
};

const gutterStyle: CSSProperties = {
  flexShrink: 0,
  width: 44,
  paddingTop: 18,
  paddingBottom: 18,
  background: "rgba(0,0,0,0.32)",
  borderRight: "1px solid rgba(255,255,255,0.04)",
  overflow: "hidden",
  fontFamily: "'DM Mono', ui-monospace, monospace",
  fontSize: 11,
  lineHeight: 1.7,
  color: "rgba(255,255,255,0.22)",
  textAlign: "right",
  userSelect: "none",
};

const gutterLineStyle: CSSProperties = {
  height: "calc(1em * 1.7)",
  padding: "0 10px 0 6px",
  fontFamily: "'DM Mono', ui-monospace, monospace",
  fontSize: 11,
};

const editorTextareaStyle: CSSProperties = {
  flex: 1,
  resize: "none",
  border: "none",
  outline: "none",
  background: "rgba(0,0,0,0.28)",
  color: "rgba(235,245,255,0.88)",
  padding: "18px 18px",
  fontFamily: "'DM Mono', ui-monospace, monospace",
  fontSize: 12.5,
  lineHeight: 1.7,
  tabSize: 2,
  caretColor: "rgba(255,220,140,0.85)",
};

const editorViewStyle: CSSProperties = {
  flex: 1,
  overflow: "auto",
  background: "rgba(0,0,0,0.28)",
  fontFamily: "'DM Mono', ui-monospace, monospace",
  fontSize: 12.5,
  lineHeight: 1.7,
  padding: 0,
};

const editorPlaceholderStyle: CSSProperties = {
  padding: 18,
  color: "rgba(255,255,255,0.40)",
  fontSize: 12.5,
  fontFamily: "'DM Mono', ui-monospace, monospace",
};

/* ── Preview pane ──────────────────────────────────────────────────── */

const previewWrapStyle: CSSProperties = {
  flex: 1,
  display: "flex",
  flexDirection: "column",
  minHeight: 0,
  background: "rgba(0,0,0,0.28)",
};

const previewChromeStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  padding: "8px 12px",
  borderBottom: "1px solid rgba(255,255,255,0.06)",
  background: "rgba(0,0,0,0.20)",
  flexShrink: 0,
};

const chromeIconBtnStyle: CSSProperties = {
  ...iconBtnGlass,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  width: 26,
  height: 26,
  borderRadius: 7,
  color: "rgba(255,255,255,0.62)",
  padding: 0,
  cursor: "pointer",
  flexShrink: 0,
};

const urlPillStyle: CSSProperties = {
  flex: 1,
  ...innerGlass,
  borderRadius: 7,
  padding: "5px 12px",
  fontFamily: "'DM Mono', ui-monospace, monospace",
  fontSize: 11,
  color: "rgba(255,255,255,0.62)",
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
  minWidth: 0,
  display: "flex",
  alignItems: "center",
  cursor: "text",
  userSelect: "text",
};

const deviceGroupStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 2,
  padding: 2,
  borderRadius: 8,
  background: "rgba(0,0,0,0.34)",
  border: "1px solid rgba(255,255,255,0.06)",
  flexShrink: 0,
};

const deviceBtnStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  width: 24,
  height: 22,
  border: "1px solid transparent",
  borderRadius: 6,
  background: "transparent",
  color: "rgba(255,255,255,0.45)",
  cursor: "pointer",
  padding: 0,
  transition: "all 0.15s ease",
};

const deviceBtnActiveStyle: CSSProperties = {
  ...deviceBtnStyle,
  background: [
    "linear-gradient(180deg, rgba(255,255,255,0.10) 0%, rgba(255,255,255,0.04) 100%)",
    "rgba(255,255,255,0.06)",
  ].join(", "),
  border: "1px solid rgba(255,255,255,0.12)",
  borderTopColor: "rgba(255,255,255,0.22)",
  boxShadow: "inset 0 1px 0 rgba(255,255,255,0.18)",
  color: "rgba(255,255,255,0.92)",
};

const previewStageStyle: CSSProperties = {
  flex: 1,
  display: "flex",
  alignItems: "stretch",
  justifyContent: "center",
  padding: 16,
  background: "rgba(0,0,0,0.30)",
  overflow: "auto",
};

const fallbackHintStyle: CSSProperties = {
  padding: "6px 14px",
  fontSize: 11,
  color: "rgba(245, 200, 120, 0.85)",
  background: "rgba(245, 200, 120, 0.06)",
  borderBottom: "1px solid rgba(245, 200, 120, 0.15)",
  fontFamily: FONT,
  flexShrink: 0,
};

const previewEmptyStyle: CSSProperties = {
  flex: 1,
  display: "flex",
  flexDirection: "column",
  alignItems: "center",
  justifyContent: "center",
  gap: 12,
  padding: 32,
  color: "rgba(255,255,255,0.55)",
  background: "rgba(0,0,0,0.22)",
  fontFamily: FONT,
};

const previewIframeStyle: CSSProperties = {
  width: "100%",
  height: "100%",
  border: "none",
  background: "white",
  borderRadius: 10,
  boxShadow: "0 20px 50px rgba(0,0,0,0.40), 0 0 0 1px rgba(255,255,255,0.06)",
};

/* ── Terminal pane ─────────────────────────────────────────────────── */

const terminalWrapStyle: CSSProperties = {
  flex: 1,
  overflowY: "auto",
  padding: "10px 14px",
  display: "flex",
  flexDirection: "column",
  gap: 8,
};

const commandCardStyle: CSSProperties = {
  ...pillGlass,
  borderRadius: 9,
  overflow: "hidden",
  animation: "tool-card-in 0.22s ease-out both",
};

const commandHeaderStyle: CSSProperties = {
  width: "100%",
  background: "transparent",
  border: "none",
  padding: "8px 12px",
  display: "flex",
  alignItems: "center",
  gap: 10,
  cursor: "pointer",
  color: "inherit",
  textAlign: "left",
  fontFamily: FONT,
};

const statusDotStyle: CSSProperties = {
  width: 7,
  height: 7,
  borderRadius: 999,
  flexShrink: 0,
  boxShadow: "inset 0 1px 0 rgba(255,255,255,0.45), 0 0 8px rgba(0,0,0,0.3)",
};

const commandTextStyle: CSSProperties = {
  fontFamily: "'DM Mono', ui-monospace, monospace",
  fontSize: 12,
  color: "rgba(255,255,255,0.82)",
  flex: 1,
  minWidth: 0,
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
};

const statusLabelStyle: CSSProperties = {
  fontSize: 10,
  letterSpacing: "0.08em",
  textTransform: "uppercase",
  fontFamily: FONT,
  flexShrink: 0,
};

const commandOutputStyle: CSSProperties = {
  margin: 0,
  whiteSpace: "pre-wrap",
  wordBreak: "break-word",
  color: "rgba(255,255,255,0.65)",
  lineHeight: 1.6,
  fontSize: 11.5,
  padding: "0 14px 12px 26px",
  fontFamily: "'DM Mono', ui-monospace, monospace",
  borderTop: "1px solid rgba(255,255,255,0.05)",
  paddingTop: 10,
  maxHeight: 280,
  overflow: "auto",
};

/* ── Diff pane ─────────────────────────────────────────────────────── */

const diffWrapStyle: CSSProperties = {
  flex: 1,
  display: "flex",
  flexDirection: "column",
  minHeight: 0,
  padding: 14,
  gap: 10,
};

const diffHeaderStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: 10,
  flexShrink: 0,
};

const diffHeaderTextStyle: CSSProperties = {
  fontSize: 12,
  color: "rgba(255,255,255,0.72)",
  fontFamily: FONT,
};

const rollbackButtonStyle: CSSProperties = {
  ...pillGlass,
  display: "inline-flex",
  alignItems: "center",
  gap: 6,
  borderRadius: 7,
  padding: "5px 10px",
  color: "rgba(255,255,255,0.85)",
  fontFamily: FONT,
  fontSize: 11.5,
  fontWeight: 500,
  border: "none",
};

const diffBodyStyle: CSSProperties = {
  ...innerGlass,
  flex: 1,
  minHeight: 0,
  padding: 12,
  borderRadius: 12,
  overflow: "auto",
};

const pierreDiffStyle = {
  minWidth: "720px",
  border: "1px solid rgba(255,255,255,0.08)",
  borderRadius: 8,
  overflow: "hidden",
  background: "rgba(0,0,0,0.24)",
};

/* ── Status bar ────────────────────────────────────────────────────── */

const statusBarStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: 12,
  padding: "6px 14px",
  borderTop: "1px solid rgba(255,255,255,0.06)",
  background: [
    "linear-gradient(180deg, transparent 0%, rgba(0,0,0,0.18) 100%)",
    "rgba(0,0,0,0.10)",
  ].join(", "),
  flexShrink: 0,
  minHeight: 32,
};

const statusBarLeftStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  minWidth: 0,
  flex: 1,
};

const statusBarCenterStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 6,
  flexShrink: 0,
};

const statusBarRightStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  flexShrink: 0,
};

const statusPathStyle: CSSProperties = {
  fontFamily: "'DM Mono', ui-monospace, monospace",
  fontSize: 11,
  color: "rgba(255,255,255,0.55)",
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
  minWidth: 0,
};

const statusSavedTextStyle: CSSProperties = {
  fontSize: 11,
  color: "rgba(255,255,255,0.55)",
  fontFamily: FONT,
};

const saveShortcutStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 6,
  background: "transparent",
  border: "none",
  fontSize: 11,
  color: "rgba(255,255,255,0.65)",
  fontFamily: FONT,
  padding: "4px 6px",
  borderRadius: 6,
  transition: "color 0.15s ease",
};

/* ── Misc ──────────────────────────────────────────────────────────── */

const emptyStateStyle: CSSProperties = {
  flex: 1,
  display: "flex",
  flexDirection: "column",
  alignItems: "center",
  justifyContent: "center",
  gap: 10,
  color: "rgba(255,255,255,0.38)",
  fontSize: 12,
  padding: 24,
};

const emptyIconStyle: CSSProperties = {
  width: 36,
  height: 36,
  borderRadius: 10,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  background: "rgba(255,255,255,0.04)",
  border: "1px solid rgba(255,255,255,0.06)",
  color: "rgba(255,255,255,0.45)",
};

const errorStyle: CSSProperties = {
  padding: "8px 14px",
  color: "rgba(255,160,160,0.92)",
  borderBottom: "1px solid rgba(255,80,80,0.20)",
  fontSize: 12,
  flexShrink: 0,
  fontFamily: FONT,
  background: "rgba(255,80,80,0.06)",
};
