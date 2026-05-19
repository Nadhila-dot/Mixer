import { useEffect, useMemo, useState, type CSSProperties } from "react";
import { Cloud, KeyRound, Power, RefreshCw, Rocket, Server, ShieldCheck, X } from "lucide-react";
import { FONT, innerGlass, outerGlass, pillGlass, smallPillGlass } from "../lib/glass";

interface ConnectionStatus {
  status: "connected" | "missing" | "invalid" | "rate_limited" | "server_misconfigured";
  connected: boolean;
  label?: string | null;
  api_key_last4?: string | null;
  verified_at?: number | null;
  last_error?: string | null;
  api_access_url: string;
}

interface CatalogEntry {
  id: string;
  label: string;
  description: string;
}

interface InstanceSummary {
  id: string;
  label: string;
  region: string;
  plan: string;
  os: string;
  main_ip: string;
  status: string;
  power_status: string;
  server_status: string;
  ssh_state: "ssh_ready" | "ssh_missing" | "ssh_failed";
}

interface AccessProfileView {
  instance_id: string;
  instance_label: string;
  host: string;
  port: number;
  username: string;
  auth_mode: "password" | "ssh_key";
  public_key: string;
  has_secret: boolean;
  ssh_state: "ssh_ready" | "ssh_missing" | "ssh_failed";
  last_verified_at?: number | null;
  last_error?: string | null;
}

interface InstanceDetail extends InstanceSummary {
  os_id?: number | null;
  internal_ip: string;
  vcpu_count?: number | null;
  ram_mb?: number | null;
  disk_gb?: number | null;
  date_created?: string | null;
  access_profile?: AccessProfileView | null;
}

type Props = {
  open: boolean;
  onClose: () => void;
};

type ActivityItem = {
  id: string;
  text: string;
};

export default function VultrPanel({ open, onClose }: Props) {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [connection, setConnection] = useState<ConnectionStatus | null>(null);
  const [instances, setInstances] = useState<InstanceSummary[]>([]);
  const [selectedInstanceId, setSelectedInstanceId] = useState<string | null>(null);
  const [selectedInstance, setSelectedInstance] = useState<InstanceDetail | null>(null);
  const [regions, setRegions] = useState<CatalogEntry[]>([]);
  const [plans, setPlans] = useState<CatalogEntry[]>([]);
  const [osList, setOsList] = useState<CatalogEntry[]>([]);
  const [sshKeys, setSshKeys] = useState<CatalogEntry[]>([]);
  const [activity, setActivity] = useState<ActivityItem[]>([]);
  const [deploying, setDeploying] = useState(false);
  const [savingAccess, setSavingAccess] = useState(false);
  const [testingAccess, setTestingAccess] = useState(false);
  const [deletingAccess, setDeletingAccess] = useState(false);
  const [importingKey, setImportingKey] = useState(false);

  const [deployForm, setDeployForm] = useState({
    label: "",
    region: "",
    plan: "",
    os_id: "",
    ssh_key_ids: [] as string[],
  });
  const [accessForm, setAccessForm] = useState({
    instance_label: "",
    host: "",
    port: "22",
    username: "root",
    auth_mode: "password" as "password" | "ssh_key",
    password: "",
    private_key: "",
    public_key: "",
  });
  const [sshKeyForm, setSshKeyForm] = useState({
    name: "",
    public_key: "",
  });

  useEffect(() => {
    if (!open) return;
    void reloadAll();
  }, [open]);

  useEffect(() => {
    if (!selectedInstanceId || !open || !connection?.connected) {
      setSelectedInstance(null);
      return;
    }
    void loadInstance(selectedInstanceId);
  }, [selectedInstanceId, open, connection?.connected]);

  const connectionBadge = useMemo(() => {
    const status = connection?.status ?? "missing";
    if (status === "connected") return { label: "Connected", color: "rgba(120,220,140,0.92)" };
    if (status === "rate_limited") return { label: "Rate limited", color: "rgba(255,205,120,0.92)" };
    if (status === "server_misconfigured") return { label: "Server setup issue", color: "rgba(255,140,140,0.92)" };
    if (status === "invalid") return { label: "Invalid key", color: "rgba(255,140,140,0.92)" };
    return { label: "Not connected", color: "rgba(255,255,255,0.64)" };
  }, [connection]);

  async function readJson<T>(url: string, init?: RequestInit): Promise<T> {
    const res = await fetch(url, {
      credentials: "same-origin",
      ...init,
      headers: {
        "content-type": "application/json",
        ...(init?.headers ?? {}),
      },
    });
    const body = await res.json().catch(() => ({}));
    if (!res.ok || !body.ok) throw new Error(body.error || "Request failed");
    return body as T;
  }

  function pushActivity(text: string) {
    setActivity((items) => [
      { id: `${Date.now()}-${Math.random()}`, text },
      ...items,
    ].slice(0, 12));
  }

  async function reloadAll() {
    setLoading(true);
    setError(null);
    try {
      const connectionBody = await readJson<{ connection: ConnectionStatus }>("/api/integrations/vultr");
      setConnection(connectionBody.connection);
      if (!connectionBody.connection.connected) {
        setInstances([]);
        setSelectedInstance(null);
        return;
      }
      const [instancesBody, regionsBody, plansBody, osBody, sshKeysBody] = await Promise.all([
        readJson<{ instances: InstanceSummary[] }>("/api/vultr/instances"),
        readJson<{ regions: CatalogEntry[] }>("/api/vultr/catalog/regions"),
        readJson<{ plans: CatalogEntry[] }>("/api/vultr/catalog/plans"),
        readJson<{ os: CatalogEntry[] }>("/api/vultr/catalog/os"),
        readJson<{ ssh_keys: CatalogEntry[] }>("/api/vultr/ssh-keys"),
      ]);
      setInstances(instancesBody.instances);
      setRegions(regionsBody.regions);
      setPlans(plansBody.plans);
      setOsList(osBody.os);
      setSshKeys(sshKeysBody.ssh_keys);
      const firstId = selectedInstanceId ?? instancesBody.instances[0]?.id ?? null;
      setSelectedInstanceId(firstId);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not load Vultr data");
    } finally {
      setLoading(false);
    }
  }

  async function loadInstance(instanceId: string) {
    try {
      const body = await readJson<{ instance: InstanceDetail }>(`/api/vultr/instances/${encodeURIComponent(instanceId)}`);
      setSelectedInstance(body.instance);
      setAccessForm({
        instance_label: body.instance.access_profile?.instance_label || body.instance.label || "",
        host: body.instance.access_profile?.host || body.instance.main_ip || "",
        port: String(body.instance.access_profile?.port || 22),
        username: body.instance.access_profile?.username || "root",
        auth_mode: body.instance.access_profile?.auth_mode || "password",
        password: "",
        private_key: "",
        public_key: body.instance.access_profile?.public_key || "",
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not load instance");
    }
  }

  async function runPowerAction(action: "start" | "stop" | "reboot") {
    if (!selectedInstanceId) return;
    try {
      setError(null);
      await readJson(`/api/vultr/instances/${encodeURIComponent(selectedInstanceId)}/power`, {
        method: "POST",
        body: JSON.stringify({ action }),
      });
      pushActivity(`${action} submitted for ${selectedInstanceId}`);
      await reloadAll();
      await loadInstance(selectedInstanceId);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Power action failed");
    }
  }

  async function deployInstance() {
    try {
      setDeploying(true);
      setError(null);
      const body = await readJson<{ instance: InstanceDetail }>("/api/vultr/instances", {
        method: "POST",
        body: JSON.stringify({
          ...deployForm,
          os_id: Number(deployForm.os_id),
        }),
      });
      pushActivity(`Deployed ${body.instance.label || body.instance.id}`);
      setDeployForm((prev) => ({ ...prev, label: "" }));
      await reloadAll();
      setSelectedInstanceId(body.instance.id);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Deploy failed");
    } finally {
      setDeploying(false);
    }
  }

  async function saveAccessProfile() {
    if (!selectedInstanceId) return;
    try {
      setSavingAccess(true);
      setError(null);
      await readJson(`/api/vultr/access-profiles/${encodeURIComponent(selectedInstanceId)}`, {
        method: "PUT",
        body: JSON.stringify({
          ...accessForm,
          port: Number(accessForm.port || 22),
        }),
      });
      pushActivity(`Saved SSH access for ${selectedInstanceId}`);
      await loadInstance(selectedInstanceId);
      await reloadAll();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not save SSH access");
    } finally {
      setSavingAccess(false);
    }
  }

  async function testAccessProfile() {
    if (!selectedInstanceId) return;
    try {
      setTestingAccess(true);
      setError(null);
      await readJson(`/api/vultr/access-profiles/${encodeURIComponent(selectedInstanceId)}/test`, {
        method: "POST",
        body: JSON.stringify({}),
      });
      pushActivity(`SSH test passed for ${selectedInstanceId}`);
      await loadInstance(selectedInstanceId);
      await reloadAll();
    } catch (err) {
      setError(err instanceof Error ? err.message : "SSH test failed");
    } finally {
      setTestingAccess(false);
    }
  }

  async function deleteAccessProfile() {
    if (!selectedInstanceId) return;
    try {
      setDeletingAccess(true);
      setError(null);
      await readJson(`/api/vultr/access-profiles/${encodeURIComponent(selectedInstanceId)}`, {
        method: "DELETE",
      });
      pushActivity(`Removed SSH access for ${selectedInstanceId}`);
      setAccessForm((form) => ({
        ...form,
        password: "",
        private_key: "",
      }));
      await loadInstance(selectedInstanceId);
      await reloadAll();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not remove SSH access");
    } finally {
      setDeletingAccess(false);
    }
  }

  async function importSshKey() {
    try {
      setImportingKey(true);
      setError(null);
      await readJson("/api/vultr/ssh-keys", {
        method: "POST",
        body: JSON.stringify(sshKeyForm),
      });
      pushActivity(`Imported SSH key ${sshKeyForm.name}`);
      setSshKeyForm({ name: "", public_key: "" });
      await reloadAll();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not import SSH key");
    } finally {
      setImportingKey(false);
    }
  }

  function toggleSshKey(id: string) {
    setDeployForm((form) => ({
      ...form,
      ssh_key_ids: form.ssh_key_ids.includes(id)
        ? form.ssh_key_ids.filter((value) => value !== id)
        : [...form.ssh_key_ids, id],
    }));
  }

  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        pointerEvents: open ? "auto" : "none",
        opacity: open ? 1 : 0,
        transition: "opacity 0.18s ease",
        zIndex: 40,
      }}
    >
      <div
        onClick={onClose}
        style={{
          position: "absolute",
          inset: 0,
          background: "rgba(0,0,0,0.28)",
          backdropFilter: "blur(12px)",
        }}
      />
      <aside
        style={{
          position: "absolute",
          top: 16,
          right: 16,
          bottom: 16,
          width: "min(540px, calc(100vw - 32px))",
          ...outerGlass,
          padding: 18,
          display: "flex",
          flexDirection: "column",
          gap: 14,
          overflow: "hidden",
          transform: open ? "translateX(0)" : "translateX(18px)",
          transition: "transform 0.18s ease",
        }}
      >
        <header style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <div style={{ ...smallPillGlass, padding: 8, borderRadius: 12 }}>
            <Cloud size={16} strokeWidth={1.8} />
          </div>
          <div style={{ minWidth: 0 }}>
            <div style={{ fontSize: 18, color: "rgba(255,255,255,0.92)", fontWeight: 600 }}>Vultr control plane</div>
            <div style={{ fontSize: 12, color: "rgba(255,255,255,0.58)" }}>
              Connect once, inspect live fleet state, deploy, and attach SSH access.
            </div>
          </div>
          <span style={{ flex: 1 }} />
          <span style={{ ...smallPillGlass, padding: "6px 10px", borderRadius: 999, fontSize: 11.5, color: connectionBadge.color }}>
            {connectionBadge.label}
          </span>
          <button type="button" onClick={() => void reloadAll()} style={chromeBtnStyle} title="Refresh">
            <RefreshCw size={14} strokeWidth={1.8} />
          </button>
          <button type="button" onClick={onClose} style={chromeBtnStyle} title="Close Vultr panel">
            <X size={14} strokeWidth={1.8} />
          </button>
        </header>

        {error && <div style={errorStyle}>{error}</div>}

        <div className="workspace-scroll" style={{ display: "flex", flexDirection: "column", gap: 14, overflowY: "auto", paddingRight: 4 }}>
          <section style={sectionStyle}>
            <div style={sectionHeaderStyle}>
              <div style={sectionTitleStyle}><ShieldCheck size={14} strokeWidth={1.8} /> Connection</div>
            </div>
            {!connection?.connected ? (
              <div style={{ ...innerGlass, padding: 14 }}>
                <p style={copyStyle}>
                  Vultr is not connected for this account. Create or view your API key in{" "}
                  <a href={connection?.api_access_url || "https://console.vultr.com/user/apiaccess/"} target="_blank" rel="noreferrer" style={linkStyle}>
                    Vultr API Access
                  </a>{" "}
                  and paste it into <a href="/settings" style={linkStyle}>Settings &gt; Vultr</a>.
                </p>
              </div>
            ) : (
              <div style={{ ...innerGlass, padding: 14, display: "grid", gap: 8 }}>
                <InfoRow label="Account label" value={connection.label || "Vultr account"} />
                <InfoRow label="API key" value={`••••${connection.api_key_last4 || ""}`} />
                <InfoRow label="Last verify" value={formatDate(connection.verified_at)} />
                {connection.last_error && <div style={mutedWarningStyle}>{connection.last_error}</div>}
              </div>
            )}
          </section>

          {connection?.connected && (
            <>
              <section style={sectionStyle}>
                <div style={sectionHeaderStyle}>
                  <div style={sectionTitleStyle}><Server size={14} strokeWidth={1.8} /> Instances</div>
                </div>
                {loading ? (
                  <div style={copyStyle}>Loading fleet…</div>
                ) : instances.length === 0 ? (
                  <div style={copyStyle}>No instances yet.</div>
                ) : (
                  <div style={{ display: "grid", gap: 10 }}>
                    {instances.map((instance) => (
                      <button
                        key={instance.id}
                        type="button"
                        onClick={() => setSelectedInstanceId(instance.id)}
                        style={{
                          ...innerGlass,
                          padding: 12,
                          textAlign: "left",
                          border: selectedInstanceId === instance.id
                            ? "1px solid rgba(160,220,255,0.34)"
                            : "1px solid rgba(255,255,255,0.08)",
                          cursor: "pointer",
                        }}
                      >
                        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                          <span style={{ fontSize: 14, fontWeight: 600, color: "rgba(255,255,255,0.9)" }}>
                            {instance.label || instance.id}
                          </span>
                          <span style={{ ...smallPillGlass, padding: "4px 8px", borderRadius: 999, fontSize: 11, color: sshColor(instance.ssh_state) }}>
                            {instance.ssh_state.replace("ssh_", "")}
                          </span>
                        </div>
                        <div style={metaTextStyle}>{instance.main_ip} • {instance.region} • {instance.plan}</div>
                        <div style={metaTextStyle}>{instance.power_status} • {instance.server_status}</div>
                      </button>
                    ))}
                  </div>
                )}
              </section>

              <section style={sectionStyle}>
                <div style={sectionHeaderStyle}>
                  <div style={sectionTitleStyle}><Rocket size={14} strokeWidth={1.8} /> Deploy</div>
                </div>
                <div style={{ ...innerGlass, padding: 14, display: "grid", gap: 10 }}>
                  <Input label="Label" value={deployForm.label} onChange={(value) => setDeployForm((form) => ({ ...form, label: value }))} />
                  <Select label="Region" value={deployForm.region} options={regions} onChange={(value) => setDeployForm((form) => ({ ...form, region: value }))} />
                  <Select label="Plan" value={deployForm.plan} options={plans} onChange={(value) => setDeployForm((form) => ({ ...form, plan: value }))} />
                  <Select label="OS image" value={deployForm.os_id} options={osList.map((entry) => ({ ...entry, id: entry.id.toString() }))} onChange={(value) => setDeployForm((form) => ({ ...form, os_id: value }))} />
                  <div style={{ display: "grid", gap: 8 }}>
                    <span style={labelStyle}>SSH keys</span>
                    <div style={{ display: "grid", gap: 6, maxHeight: 120, overflowY: "auto" }}>
                      {sshKeys.map((key) => (
                        <label key={key.id} style={checkboxRowStyle}>
                          <input
                            type="checkbox"
                            checked={deployForm.ssh_key_ids.includes(key.id)}
                            onChange={() => toggleSshKey(key.id)}
                          />
                          <span>{key.label}</span>
                        </label>
                      ))}
                    </div>
                  </div>
                  <button type="button" onClick={() => void deployInstance()} disabled={deploying} style={actionBtnStyle}>
                    {deploying ? "Deploying…" : "Deploy instance"}
                  </button>
                </div>
              </section>

              <section style={sectionStyle}>
                <div style={sectionHeaderStyle}>
                  <div style={sectionTitleStyle}><KeyRound size={14} strokeWidth={1.8} /> SSH keys</div>
                </div>
                <div style={{ ...innerGlass, padding: 14, display: "grid", gap: 10 }}>
                  <Input label="Key name" value={sshKeyForm.name} onChange={(value) => setSshKeyForm((form) => ({ ...form, name: value }))} />
                  <Textarea label="Public key" value={sshKeyForm.public_key} onChange={(value) => setSshKeyForm((form) => ({ ...form, public_key: value }))} placeholder="ssh-ed25519 AAAA..." rows={4} />
                  <button type="button" onClick={() => void importSshKey()} disabled={importingKey} style={actionBtnStyle}>
                    {importingKey ? "Importing…" : "Import SSH key"}
                  </button>
                </div>
              </section>

              {selectedInstance && (
                <section style={sectionStyle}>
                  <div style={sectionHeaderStyle}>
                    <div style={sectionTitleStyle}><Server size={14} strokeWidth={1.8} /> Instance detail</div>
                    <div style={{ display: "inline-flex", gap: 8 }}>
                      <button type="button" onClick={() => void runPowerAction("start")} style={miniBtnStyle}><Power size={13} strokeWidth={1.8} /> Start</button>
                      <button type="button" onClick={() => void runPowerAction("reboot")} style={miniBtnStyle}><Power size={13} strokeWidth={1.8} /> Reboot</button>
                      <button type="button" onClick={() => void runPowerAction("stop")} style={miniBtnStyle}><Power size={13} strokeWidth={1.8} /> Stop</button>
                    </div>
                  </div>
                  <div style={{ ...innerGlass, padding: 14, display: "grid", gap: 8 }}>
                    <InfoRow label="Label" value={selectedInstance.label || selectedInstance.id} />
                    <InfoRow label="Public IP" value={selectedInstance.main_ip || "pending"} />
                    <InfoRow label="OS" value={selectedInstance.os} />
                    <InfoRow label="Region" value={selectedInstance.region} />
                    <InfoRow label="Plan" value={selectedInstance.plan} />
                    <InfoRow label="Power" value={selectedInstance.power_status} />
                    <InfoRow label="Server" value={selectedInstance.server_status} />
                    <InfoRow label="SSH" value={selectedInstance.ssh_state.replace("ssh_", "")} />
                  </div>
                </section>
              )}

              {selectedInstance && (
                <section style={sectionStyle}>
                  <div style={sectionHeaderStyle}>
                    <div style={sectionTitleStyle}><ShieldCheck size={14} strokeWidth={1.8} /> SSH Access</div>
                  </div>
                  <div style={{ ...innerGlass, padding: 14, display: "grid", gap: 10 }}>
                    <Input label="Instance label" value={accessForm.instance_label} onChange={(value) => setAccessForm((form) => ({ ...form, instance_label: value }))} />
                    <Input label="Host / IP" value={accessForm.host} onChange={(value) => setAccessForm((form) => ({ ...form, host: value }))} />
                    <div style={{ display: "grid", gridTemplateColumns: "1fr 110px", gap: 10 }}>
                      <Input label="Username" value={accessForm.username} onChange={(value) => setAccessForm((form) => ({ ...form, username: value }))} />
                      <Input label="Port" value={accessForm.port} onChange={(value) => setAccessForm((form) => ({ ...form, port: value }))} />
                    </div>
                    <div style={{ display: "grid", gap: 6 }}>
                      <span style={labelStyle}>Auth mode</span>
                      <div style={{ display: "inline-flex", gap: 8 }}>
                        {(["password", "ssh_key"] as const).map((mode) => (
                          <button
                            key={mode}
                            type="button"
                            onClick={() => setAccessForm((form) => ({ ...form, auth_mode: mode }))}
                            style={{
                              ...smallPillGlass,
                              padding: "7px 12px",
                              borderRadius: 999,
                              border: mode === accessForm.auth_mode
                                ? "1px solid rgba(160,220,255,0.34)"
                                : "1px solid rgba(255,255,255,0.08)",
                              color: "rgba(255,255,255,0.84)",
                              cursor: "pointer",
                            }}
                          >
                            {mode === "password" ? "Password" : "SSH key"}
                          </button>
                        ))}
                      </div>
                    </div>
                    {accessForm.auth_mode === "password" ? (
                      <Input label="Password" value={accessForm.password} onChange={(value) => setAccessForm((form) => ({ ...form, password: value }))} secret />
                    ) : (
                      <>
                        <Textarea label="Private key" value={accessForm.private_key} onChange={(value) => setAccessForm((form) => ({ ...form, private_key: value }))} rows={5} />
                        <Textarea label="Public key (optional metadata)" value={accessForm.public_key} onChange={(value) => setAccessForm((form) => ({ ...form, public_key: value }))} rows={3} />
                      </>
                    )}
                    {selectedInstance.access_profile?.last_error && (
                      <div style={mutedWarningStyle}>{selectedInstance.access_profile.last_error}</div>
                    )}
                    <div style={{ display: "flex", gap: 10, flexWrap: "wrap" }}>
                      <button type="button" onClick={() => void saveAccessProfile()} disabled={savingAccess} style={actionBtnStyle}>
                        {savingAccess ? "Saving…" : "Save access"}
                      </button>
                      <button type="button" onClick={() => void testAccessProfile()} disabled={testingAccess} style={secondaryBtnStyle}>
                        {testingAccess ? "Testing…" : "Test connection"}
                      </button>
                      <button
                        type="button"
                        onClick={() => void deleteAccessProfile()}
                        disabled={deletingAccess || !selectedInstance.access_profile}
                        style={dangerBtnStyle}
                      >
                        {deletingAccess ? "Removing…" : "Remove access"}
                      </button>
                    </div>
                  </div>
                </section>
              )}

              <section style={sectionStyle}>
                <div style={sectionHeaderStyle}>
                  <div style={sectionTitleStyle}><RefreshCw size={14} strokeWidth={1.8} /> Activity</div>
                </div>
                <div style={{ ...innerGlass, padding: 14, display: "grid", gap: 8 }}>
                  {activity.length === 0 ? (
                    <div style={copyStyle}>No Vultr actions yet in this session.</div>
                  ) : (
                    activity.map((item) => (
                      <div key={item.id} style={metaTextStyle}>{item.text}</div>
                    ))
                  )}
                </div>
              </section>
            </>
          )}
        </div>
      </aside>
    </div>
  );
}

function formatDate(value?: number | null) {
  if (!value) return "Never";
  return new Intl.DateTimeFormat(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  }).format(new Date(value * 1000));
}

function sshColor(state: string) {
  if (state === "ssh_ready") return "rgba(120,220,140,0.92)";
  if (state === "ssh_failed") return "rgba(255,140,140,0.92)";
  return "rgba(255,210,120,0.92)";
}

function InfoRow({ label, value }: { label: string; value: string }) {
  return (
    <div style={{ display: "flex", justifyContent: "space-between", gap: 16 }}>
      <span style={labelStyle}>{label}</span>
      <span style={{ color: "rgba(255,255,255,0.88)", fontSize: 13.5, textAlign: "right" }}>{value}</span>
    </div>
  );
}

function Input({
  label,
  value,
  onChange,
  secret,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  secret?: boolean;
}) {
  return (
    <label style={{ display: "grid", gap: 6 }}>
      <span style={labelStyle}>{label}</span>
      <input
        type={secret ? "password" : "text"}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        style={inputStyle}
      />
    </label>
  );
}

function Textarea({
  label,
  value,
  onChange,
  rows,
  placeholder,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  rows: number;
  placeholder?: string;
}) {
  return (
    <label style={{ display: "grid", gap: 6 }}>
      <span style={labelStyle}>{label}</span>
      <textarea
        rows={rows}
        value={value}
        placeholder={placeholder}
        onChange={(event) => onChange(event.target.value)}
        style={textareaStyle}
      />
    </label>
  );
}

function Select({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string;
  options: CatalogEntry[];
  onChange: (value: string) => void;
}) {
  return (
    <label style={{ display: "grid", gap: 6 }}>
      <span style={labelStyle}>{label}</span>
      <select value={value} onChange={(event) => onChange(event.target.value)} style={inputStyle}>
        <option value="">Select…</option>
        {options.map((option) => (
          <option key={option.id} value={option.id}>
            {option.label}
          </option>
        ))}
      </select>
    </label>
  );
}

const chromeBtnStyle: CSSProperties = {
  ...smallPillGlass,
  width: 34,
  height: 34,
  borderRadius: 12,
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
  border: "none",
  color: "rgba(255,255,255,0.82)",
  cursor: "pointer",
};

const sectionStyle: CSSProperties = {
  display: "grid",
  gap: 10,
};

const sectionHeaderStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: 10,
};

const sectionTitleStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 8,
  color: "rgba(255,255,255,0.9)",
  fontSize: 13.5,
  fontWeight: 600,
};

const copyStyle: CSSProperties = {
  color: "rgba(255,255,255,0.68)",
  fontSize: 13,
  lineHeight: 1.6,
};

const linkStyle: CSSProperties = {
  color: "rgba(180,220,255,0.96)",
};

const labelStyle: CSSProperties = {
  color: "rgba(255,255,255,0.56)",
  fontSize: 12,
  fontWeight: 500,
};

const inputStyle: CSSProperties = {
  ...innerGlass,
  width: "100%",
  border: "1px solid rgba(255,255,255,0.08)",
  color: "rgba(255,255,255,0.88)",
  fontFamily: FONT,
  fontSize: 13.5,
  outline: "none",
  padding: "10px 12px",
  borderRadius: 14,
  background: "rgba(0,0,0,0.18)",
};

const textareaStyle: CSSProperties = {
  ...inputStyle,
  resize: "vertical",
  minHeight: 92,
};

const errorStyle: CSSProperties = {
  ...innerGlass,
  padding: 12,
  color: "rgba(255,150,150,0.96)",
  fontSize: 12.5,
  border: "1px solid rgba(255,120,120,0.20)",
};

const metaTextStyle: CSSProperties = {
  color: "rgba(255,255,255,0.60)",
  fontSize: 12.5,
  lineHeight: 1.5,
};

const actionBtnStyle: CSSProperties = {
  ...pillGlass,
  border: "none",
  borderRadius: 999,
  padding: "10px 14px",
  color: "rgba(255,255,255,0.9)",
  fontFamily: FONT,
  fontSize: 13,
  cursor: "pointer",
};

const secondaryBtnStyle: CSSProperties = {
  ...smallPillGlass,
  border: "1px solid rgba(255,255,255,0.08)",
  borderRadius: 999,
  padding: "10px 14px",
  color: "rgba(255,255,255,0.82)",
  fontFamily: FONT,
  fontSize: 13,
  cursor: "pointer",
};

const dangerBtnStyle: CSSProperties = {
  ...secondaryBtnStyle,
  color: "rgba(255,170,170,0.92)",
  border: "1px solid rgba(255,110,110,0.18)",
};

const miniBtnStyle: CSSProperties = {
  ...smallPillGlass,
  border: "1px solid rgba(255,255,255,0.08)",
  borderRadius: 999,
  padding: "7px 11px",
  color: "rgba(255,255,255,0.82)",
  fontFamily: FONT,
  fontSize: 12,
  cursor: "pointer",
  display: "inline-flex",
  alignItems: "center",
  gap: 6,
};

const checkboxRowStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  color: "rgba(255,255,255,0.8)",
  fontSize: 12.5,
};

const mutedWarningStyle: CSSProperties = {
  color: "rgba(255,205,120,0.9)",
  fontSize: 12.5,
  lineHeight: 1.5,
};
