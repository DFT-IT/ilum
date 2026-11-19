import { t } from "./language";
import { useCallback, useRef, useState, useEffect, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  Activity, AppWindow, ArrowRight, ArrowUpRight, ChartNoAxesCombined, Cpu,
  Files, HardDrive, LayoutGrid, MemoryStick, PanelLeft, ScanLine, Settings2, ShieldCheck, TerminalSquare, UserRound, Workflow
} from "lucide-react";
import { bytes, call, describeError, native, number, percent, ratio, useTelemetry, type Process, type Screen } from "./api";
import { AppearanceControl } from "./appearance";
import { LanguageControl } from "./language-control";
import { Confirmation } from "./confirmation";
import { ScanView } from "./scan";
import { useFileScan, type CacheScope } from "./file-scan";
import { QuickClean } from "./quick-clean";
import { SortHeader, sortRows, useSort } from "./sort";
import { DevView } from "./dev";
import { GrowthShortcut, GrowthView, useGrowth } from "./growth";
import { AppsView, InventoryButton, SystemScanSummary, useSystemInventory } from "./inventory";
import { Settings } from "./settings";
import { elevatedPressure, freeDisk, lowDisk, Recommendations, sustainedCpu } from "./recommendations";

const nav = [
  { key: "overview", text: "Übersicht", icon: LayoutGrid },
  { key: "cache", text: "Bereinigung", icon: ScanLine },
  { key: "apps", text: "Apps & Caches", icon: AppWindow },
  { key: "storage", text: "Festplatte", icon: HardDrive },
  { key: "growth", text: "Was ist gewachsen?", icon: ChartNoAxesCombined },
  { key: "files", text: "Dateien & Duplikate", icon: Files },
  { key: "processes", text: "Prozesse", icon: Activity },
  { key: "dev", text: "Dev-Modus", icon: TerminalSquare },
] as const;

const sidebarGroups = [
    { title: "System", entries: nav.filter(item => item.key === "overview" || item.key === "processes") },
    { title: "Speicher", entries: nav.filter(item => !["overview", "processes", "dev"].includes(item.key)) },
    { title: "Entwicklung", entries: nav.filter(item => item.key === "dev") },
  ];

export function Brand() {
  return <div className="brand">
    <img src="/mark.svg" width="37" height="37" alt="" />
    <div><strong>Ilum</strong></div>
  </div>;
}

function Trace({ history }: { history: number[] }) {
  const points = history.map((value, i) => `${132 - (history.length - 1 - i) * (132 / 35)},${39 - Math.max(0, Math.min(100, value)) * 0.34}`).join(" ");
  return <svg className="trace" viewBox="0 0 133 44" aria-hidden="true">
    <path d="M1 11H132M1 26H132M1 41H132" className="trace-grid" />
    {history.length > 1 && <>
      <polygon points={`${132 - (history.length - 1) * (132 / 35)},41 ${points} 132,41`} className="trace-fill" />
      <polyline points={points} className="trace-stroke" />
    </>}
    {history.length > 0 && <circle cx="131" cy={39 - history.at(-1)! * .34} r="2" className="trace-dot" />}
  </svg>;
}

export function Progress({ value, kind = "" }: { value: number | null; kind?: string }) {
  return <div className={`progress ${kind}`} role="meter" aria-label={t("Auslastung")} aria-valuemin={0} aria-valuemax={100} aria-valuenow={value ?? undefined}>
    <span style={{ transform: `scaleX(${(value ?? 0) / 100})` }} />
  </div>;
}

function Metric({ icon, label, value, sub, children }: {
  icon: ReactNode; label: string; value: string; sub: string; children?: ReactNode;
}) {
  const [amount, unit] = value.split(" ");
  return <section className="metric">
    <div className="metric-label"><span className="metric-icon">{icon}</span>{label}</div>
    <div className="metric-reading"><div className="metric-value">{amount}{unit && <small>{unit}</small>}</div>{children}</div>
    <div className="metric-sub">{sub}</div>
  </section>;
}

function ProcessTable({ rows, full, refresh, initialSort = "memory" }: {
  rows: Process[] | undefined; full?: boolean; refresh: () => void; initialSort?: "memory" | "cpu";
}) {
  const sort = useSort<"name" | "pid" | "memory" | "cpu">(initialSort, "descending");
  const [target, setTarget] = useState<Process>();
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ content: string; error?: boolean }>();
  const processes = sortRows(rows ?? [], sort, (row, key) => row[key]).slice(0, full ? 30 : 4);

  async function terminate() {
    if (!target) return;
    setBusy(true);
    setMessage(undefined);
    try {
      await call("terminate_process", { pid: target.pid, started: target.started });
      setMessage({ content: t("Beenden-Signal an „{0}“ gesendet.", target.name) });
      refresh();
    } catch (error) {
      setMessage({ content: describeError(error), error: true });
    } finally {
      setBusy(false);
      setTarget(undefined);
    }
  }

  return <>
    {message && <div role="status" className={message.error ? "notice error" : "notice"}>{message.content}</div>}
    <div className="table-scroll">
      <table className="process-table">
        <thead><tr>
          <SortHeader column="name" sort={sort} label={t("Prozess")} />
          <SortHeader column="pid" sort={sort} label="PID" />
          <SortHeader column="memory" sort={sort} label="RAM" first="descending" />
          <SortHeader column="cpu" sort={sort} label="CPU" first="descending" />
          {full && <th><span className="sr-only">{t("Aktion")}</span></th>}
        </tr></thead>
        <tbody>
          {processes.map(process => <tr key={process.pid}>
            <td><span className="process-name"><span className="process-icon"><Workflow size={14} /></span><span title={process.name}>{process.name}</span></span></td>
            <td>{process.pid}</td><td>{bytes(process.memory)}</td><td>{percent(process.cpu)}</td>
            {full && <td>{process.canTerminate && <button className="text-button" onClick={() => setTarget(process)}>{t("Beenden")}</button>}</td>}
          </tr>)}
          {processes.length === 0 && <tr><td colSpan={full ? 5 : 4} className="empty-row">{rows ? t("Keine Prozesse gefunden.") : t("Prozessdaten werden geladen.")}</td></tr>}
        </tbody>
      </table>
    </div>
    {target && <Confirmation title={t("Prozess beenden?")} detail={t("„{0}“ (PID {1}) soll sich beenden. Ungesicherte Arbeit kann verloren gehen.", target.name, target.pid)} action={t("Prozess beenden")} busy={busy} onClose={() => setTarget(undefined)} onConfirm={terminate} />}
  </>;
}

export function Dashboard() {
  const [screen, setScreen] = useState<Screen>("overview");
  const [sidebarCompact, setSidebarCompact] = useState(false);
  const [settings, setSettings] = useState(false);
  const [accountPicture, setAccountPicture] = useState<string>();
  const [scope, setScope] = useState<CacheScope>();
  const [quickReview, setQuickReview] = useState(false);
  const [processSort, setProcessSort] = useState<"cpu" | "memory">("memory");
  const { sample, history, error, refresh } = useTelemetry();
  const [opened, setOpened] = useState(false);
  useEffect(() => { if (sample) setOpened(true); }, [sample]);
  const inventory = useSystemInventory(opened);
  const growth = useGrowth();
  const cache = useFileScan("cache", opened || screen === "cache");
  const storage = useFileScan("storage", screen === "storage");
  const files = useFileScan("files", screen === "files");
  const appCache = useFileScan("appCache", screen === "appCache", scope);
  const activeScreen = screen === "appCache" ? "apps" : screen;
  const ram = ratio(sample?.ramUsed, sample?.ramTotal);
  const disk = ratio(sample?.diskUsed, sample?.diskTotal);
  const diskFree = freeDisk(sample);
  const content = useRef<HTMLElement>(null);

  const go = useCallback((next: Screen) => {
    setQuickReview(false);
    setScreen(next);
    content.current?.scrollTo({ top: 0 });
  }, []);

  useEffect(() => {
    if (!native) return;
    const listener = listen("show-storage-growth", () => go("growth"));
    return () => { void listener.then(unlisten => unlisten()).catch(() => {}); };
  }, [go]);

  useEffect(() => {
    if (native) void call<string | null>("account_picture").then(picture => {
      if (picture) setAccountPicture(`data:image/jpeg;base64,${picture}`);
    }).catch(() => {});
    const openSettings = (event: KeyboardEvent) => {
      if (event.metaKey && event.key === ",") {
        event.preventDefault();
        setSettings(true);
      }
    };
    window.addEventListener("keydown", openSettings);
    return () => window.removeEventListener("keydown", openSettings);
  }, []);

  function reviewQuickCleanup() {
    go("cache");
    setQuickReview(true);
  }

  function reviewAppCache(next: CacheScope) {
    setScope(next);
    go("appCache");
  }

  function showProcesses(sort: "cpu" | "memory") {
    setProcessSort(sort);
    go("processes");
  }

  return <div className={`app-frame ${sidebarCompact ? "sidebar-compact" : ""}`}>
    <aside className="sidebar" id="workspace-sidebar">
      <div className="sidebar-titlebar" data-tauri-drag-region />
      <nav aria-label={t("Arbeitsbereiche")}>
        {sidebarGroups.map(group => <section className="sidebar-group" key={group.title} aria-label={t(group.title)}>
          <p>{t(group.title)}</p>
          {group.entries.map(({ key, icon: Icon, text }) => <button className="nav-button"
            title={t(text)} aria-label={t(text)} aria-current={activeScreen === key ? "page" : undefined}
            onClick={() => go(key)} key={key}><Icon size={17} strokeWidth={1.8} aria-hidden="true" /><span>{t(text)}</span></button>)}
        </section>)}
      </nav>
      <div className="sidebar-status">
        <span className="sidebar-status-icon">
          {accountPicture ? <img alt="" src={accountPicture} /> : <UserRound size={16} aria-hidden="true" />}
        </span>
        <div><strong title={sample?.computerName ?? undefined}>{sample?.computerName ?? "Mac"}</strong></div>
      </div>
    </aside>
    <div className="workspace">
      <header className="topbar" data-tauri-drag-region="deep">
        <div className="toolbar-leading">
          <button className="icon-button sidebar-toggle" title={t("Seitenleiste umschalten")} aria-label={t("Seitenleiste umschalten")}
            aria-controls="workspace-sidebar" aria-pressed={sidebarCompact} onClick={() => setSidebarCompact(value => !value)}>
            <PanelLeft size={17} aria-hidden="true" />
          </button>
          <Brand />
        </div>
        <div className="toolbar-trailing">
          <LanguageControl compact />
          <AppearanceControl />
          <button className="icon-button settings-toggle" type="button" onClick={() => setSettings(true)}
            title={t("Einstellungen (⌘,)")} aria-label={t("Einstellungen")}><Settings2 size={17} aria-hidden="true" /></button>
        </div>
      </header>
      <main className="main-content" ref={content}>
        {!native && <div className="notice">{t("Öffne die macOS-App für Live-Systemdaten und Dateiaktionen.")}</div>}
        {error && <div role="status" className="notice error">{error}</div>}
        {screen === "overview" && <div className="screen" key="overview">
          <div className="page-heading">
            <div><h1>{t("Systemübersicht")}</h1><p>{t("Was Aufmerksamkeit braucht und was du jetzt tun kannst.")}</p></div>
            {inventory.started && <InventoryButton scanning={inventory.scanning} onClick={() => inventory.scan()} />}
          </div>
          <QuickClean scan={cache} openReview={reviewQuickCleanup} onMoved={refresh} />
          <SystemScanSummary control={inventory} openApps={() => go("apps")} openTools={() => go("dev")} />
          <GrowthShortcut control={growth} open={() => go("growth")} />
          <Recommendations sample={sample} history={history} checkStorage={() => go("storage")}
            checkProcesses={showProcesses} checkDev={() => go("dev")} />
          <h2 className="section-title">{t("Systemauslastung")}</h2>
          <div className="metrics">
            <Metric icon={<Cpu size={17} />} label={t("Prozessor")} value={percent(sample?.cpu)} sub={sample ? t("{0} logische Kerne", sample.cores) : "CPU-Auslastung"}>
              <Trace history={history} />
            </Metric>
            <Metric icon={<MemoryStick size={17} />} label={t("Arbeitsspeicher")} value={bytes(sample?.ramUsed)} sub={sample ? t("von {0} RAM belegt{1}", bytes(sample.ramTotal), elevatedPressure(sample) ? t(" · Speicherdruck") : "") : t("Verwendeter RAM")}>
              <Progress value={ram} kind="memory" />
            </Metric>
            <Metric icon={<HardDrive size={17} />} label={t("Festplatte")} value={percent(disk)} sub={diskFree == null ? t("Belegter Speicher") : t("{0} verfügbar", bytes(diskFree))}>
              <Progress value={disk} kind="disk" />
            </Metric>
          </div>
          <section className="panel processes-panel">
            <div className="panel-heading"><div><h2>{t("Aktive Prozesse")}</h2><p>{t("Höchster Arbeitsspeicherverbrauch")}</p></div><button className="button secondary compact" onClick={() => showProcesses("memory")}>{t("Prozesse öffnen")}<ArrowRight size={15} /></button></div>
            <ProcessTable rows={sample?.processes} refresh={refresh} />
          </section>
        </div>}
        {screen === "processes" && <div className="screen" key="processes"><div className="page-heading"><div><h1>{t("Prozesse")}</h1><p>{t("Bis zu 30 Prozesse nach RAM- oder CPU-Verbrauch. 100 % CPU entsprechen einem ausgelasteten Kern.")}</p></div></div><section className="panel processes-panel"><div className="panel-heading"><h2>{t("Aktive Prozesse")}</h2><span className="muted">{Math.min(sample?.processes.length ?? 0, 30)} {t("Einträge")}</span></div><ProcessTable rows={sample?.processes} refresh={refresh} initialSort={processSort} full /></section></div>}
        {screen === "growth" && <GrowthView control={growth} />}
        {screen === "cache" && <ScanView mode="cache" fileScan={cache} selectRecommended={quickReview} />}
        {screen === "storage" && <ScanView mode="storage" fileScan={storage}>
          <section className="panel storage-panel volume-panel">
            <div className="panel-heading"><div><h2>{t("Internes Startvolume")}</h2><p>{diskFree != null && t("{0} verfügbar", bytes(diskFree))}</p></div><div className="panel-icon"><HardDrive size={19} /></div></div>
            <div className="disk-summary"><strong>{bytes(sample?.diskUsed)}</strong><span> / {bytes(sample?.diskTotal)} {t("belegt")}</span></div>
            <Progress value={disk} kind="storage-volume" />
            <div className="storage-footer"><div className="legend"><span><i /> {t("Belegt")}</span><span><i /> {t("Verfügbar")}</span></div><button className="text-button" onClick={() => go("files")}>{t("Große Dateien prüfen")}<ArrowRight size={15} /></button></div>
          </section>
        </ScanView>}
        {screen === "files" && <ScanView mode="files" fileScan={files} />}
        <AppsView active={screen === "apps"} control={inventory} openCleanup={() => go("cache")} reviewCache={reviewAppCache} />
        {screen === "appCache" && scope && <ScanView key={scope.target} mode="cache" fileScan={appCache} scope={scope} onBack={() => go("apps")} />}
        <DevView active={screen === "dev"} inventory={inventory} />
      </main>
    </div>
    {settings && <Settings close={() => setSettings(false)} />}
  </div>;
}

export function Popover() {
  const { sample, history, error } = useTelemetry();
  const [problem, setProblem] = useState("");
  const growth = useGrowth();
  const rows = [
    { label: t("Prozessor"), icon: Cpu, text: percent(sample?.cpu), value: sample?.cpu ?? null },
    { label: t("Arbeitsspeicher"), icon: MemoryStick, text: bytes(sample?.ramUsed), value: ratio(sample?.ramUsed, sample?.ramTotal) },
    { label: t("Festplatte"), icon: HardDrive, text: percent(ratio(sample?.diskUsed, sample?.diskTotal)), value: ratio(sample?.diskUsed, sample?.diskTotal) },
  ];
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => { if (event.key === "Escape") void call("close_popover").catch(() => {}); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const menuHint = lowDisk(sample) ? t("Wenig freier Platz · {0} verfügbar", bytes(freeDisk(sample)))
    : elevatedPressure(sample) ? t("Erhöhter Speicherdruck · Prozesse prüfen")
      : sustainedCpu(history) ? t("Anhaltende CPU-Last · Prozesse prüfen")
        : sample?.computerName ?? t("Systemdaten werden geladen");

  return <main className="popover">
    <header><Brand /><LanguageControl compact /></header>
    <section className="menu-metrics" aria-label={t("Systemauslastung")}>
      {rows.map(({ label, icon: Icon, text, value }) => <div className="menu-metric" key={label}><Icon size={17} /><div><div><span>{label}</span><strong>{text}</strong></div><Progress value={value} /></div></div>)}
    </section>
    <GrowthShortcut control={growth} open={() => void call("open_dashboard", { growth: true }).catch(error => setProblem(describeError(error)))} />
    <footer>
      {problem || error ? <span role="status" className="muted">{problem || error}</span> : <span className="muted"><ShieldCheck size={15} /> {menuHint}</span>}
      <button className="button primary" onClick={() => void call("open_dashboard").catch(error => setProblem(describeError(error)))}>{t("Dashboard öffnen")}<ArrowUpRight size={17} /></button>
    </footer>
  </main>;
}
