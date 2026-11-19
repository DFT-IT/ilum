import { currentLocale, t } from "./language";
import { useCallback, useEffect, useRef, useState } from "react";
import { Box, ExternalLink, FolderOpen, LoaderCircle, RotateCw, TerminalSquare, Trash2 } from "lucide-react";
import { bytes, call, dateTime, describeError, native, number, sizeValue } from "./api";
import { DevImpactDialog } from "./dev-impact";
import { InventoryButton, ToolchainView, type InventoryControl } from "./inventory";
import { SortHeader, sortRows, useSort } from "./sort";
import { DevCachesView } from "./dev-caches";

const kindLabels = {
  nodeModules: "Node.js-Abhängigkeiten",
  venv: "Python-Umgebung",
  rustTarget: "Rust-Build",
  nextBuild: "Next.js-Build",
  swiftBuild: "Swift-Build",
  xcodeDerivedData: "Xcode DerivedData",
};
type Project = {
  id: number; name: string; path: string; kind: keyof typeof kindLabels;
  bytes: number; modifiedAt: number | null; rebuild: string | null; canTrash: boolean;
};
type ProjectReport = { scan: number; roots: string[]; rows: Project[]; total: number; skipped: number; truncated: boolean };
type DockerReport = {
  status: "missing" | "offline" | "ready";
  usage: { label: string; size: string; reclaimable: string }[];
  images: { id: string; repository: string; tag: string; size: string }[];
  truncated: boolean;
};

const dockerLabel: Record<string, string> = {
  Images: "Images", Containers: "Container", "Local Volumes": "Volumes", "Build Cache": "Build-Cache",
};

function DockerPanel({ report, scanning, started, scan, open }: {
  report?: DockerReport; scanning: boolean; started: boolean; scan: () => void; open: () => void;
}) {
  const sort = useSort<"repository" | "tag" | "size">("size", "descending");
  const images = sortRows(report?.images ?? [], sort, (row, key) => key === "size" ? sizeValue(row.size) : row[key]);
  return <section className="panel dev-panel">
    <div className="panel-heading">
      <div className="dev-heading"><span className="panel-icon"><Box size={19} /></span><div>
        <h2>Docker Desktop</h2><p>{t("Images und Speicherbelegung des lokalen Docker-Dienstes")}</p>
      </div></div>
      <div className="dev-actions">
        {started && <button className="button secondary compact" disabled={!native || scanning} onClick={scan}><RotateCw size={15} className={scanning ? "spin" : undefined} /> {t("Aktualisieren")}</button>}
        {report && report.status !== "missing" && <button className="button secondary compact" onClick={open}>{t("Docker öffnen")}<ExternalLink size={14} /></button>}
      </div>
    </div>
    {scanning && <p className="dev-note" role="status">{t("Lokale Docker-Belegung wird gelesen …")}</p>}
    {report?.status === "missing" && <p className="dev-note">{t("Docker Desktop wurde nicht gefunden.")}</p>}
    {report?.status === "offline" && <p className="dev-note">{t("Der lokale Docker-Dienst antwortet nicht. Starte Docker Desktop und prüfe erneut.")}</p>}
    {report?.status === "ready" && <>
      {report.usage.length > 0 && <dl className="docker-usage">{report.usage.map(item => <div key={item.label}>
        <dt>{t(dockerLabel[item.label] ?? item.label)}</dt><dd>{item.size}</dd><small>{item.reclaimable} {t("laut Docker freigebbar")}</small>
      </div>)}</dl>}
      <p className="dev-note">{t("Prüfe ungenutzte Images in Docker Desktop. Images können Ebenen teilen, daher entsprechen ihre Einzelgrößen nicht dem frei werdenden Platz.")}</p>
      {report.images.length > 0 ? <div className="table-scroll"><table className="dev-table docker-table">
        <thead><tr><SortHeader column="repository" sort={sort} label="Image" /><SortHeader column="tag" sort={sort} label="Tag" /><SortHeader column="size" sort={sort} label={t("Größe")} first="descending" /></tr></thead>
        <tbody>{images.map(image => <tr key={`${image.id}/${image.repository}/${image.tag}`}>
          <td><div className="file-name"><div><strong title={image.repository}>{image.repository === "<none>" ? t("Image ohne Repository") : image.repository}</strong><span>{image.id.replace(/^sha256:/, "").slice(0, 12)}</span></div></div></td>
          <td>{image.tag === "<none>" ? t("Ohne Tag") : image.tag}</td><td>{image.size}</td>
        </tr>)}</tbody>
      </table></div> : <p className="dev-note">{t("Keine lokalen Images in der Docker-Liste.")}</p>}
      {report.truncated && <p className="dev-note">{t("Die Liste zeigt bis zu 150 Images.")}</p>}
    </>}
  </section>;
}

export function DevView({ active, inventory }: { active: boolean; inventory: InventoryControl }) {
  const [tab, setTab] = useState<"tools" | "caches" | "projects" | "docker">("tools");
  const [folder, setFolder] = useState<string>();
  const [withHistory, setWithHistory] = useState(false);
  const projectsStarted = useRef(false);
  const dockerStarted = useRef(false);
  const projectsRunning = useRef(false);
  const dockerRunning = useRef(false);
  const sort = useSort<"name" | "kind" | "bytes" | "modifiedAt">("bytes", "descending");
  const [report, setReport] = useState<ProjectReport>();
  const [docker, setDocker] = useState<DockerReport>();
  const [scanning, setScanning] = useState(false);
  const [dockerScanning, setDockerScanning] = useState(false);
  const [target, setTarget] = useState<Project>();
  const [moving, setMoving] = useState(false);
  const [message, setMessage] = useState<{ text: string; problem?: boolean }>();
  const [filter, setFilter] = useState<"all" | "rebuildable">("all");

  const runDocker = useCallback(async () => {
    if (!native || dockerRunning.current) return;
    dockerStarted.current = true;
    dockerRunning.current = true;
    setDockerScanning(true);
    setMessage(undefined);
    try {
      setDocker(await call<DockerReport>("scan_docker"));
    } catch (error) {
      setMessage({ text: describeError(error), problem: true });
    } finally {
      dockerRunning.current = false;
      setDockerScanning(false);
    }
  }, []);

  const scan = useCallback(async (action: "default" | "choose" | "refresh") => {
    if (!native || projectsRunning.current) return;
    projectsStarted.current = true;
    projectsRunning.current = true;
    setScanning(true);
    setMessage(undefined);
    try {
      let custom = action === "refresh" && folder != null;
      if (action === "choose") {
        const picked = await call<string | null>("pick_dev_folder", { english: currentLocale() === "en-GB" });
        if (picked === null) return;
        setFolder(picked);
        custom = true;
      } else if (action === "default") {
        setFolder(undefined);
      }
      setReport(undefined);
      const result = await call<ProjectReport>("scan_dev", { custom });
      setReport(result);
    } catch (error) {
      setMessage({ text: describeError(error), problem: true });
    } finally {
      projectsRunning.current = false;
      setScanning(false);
    }
  }, [folder]);

  useEffect(() => {
    if (active && tab === "projects" && !projectsStarted.current) void scan("default");
    if (active && tab === "docker" && !dockerStarted.current) void runDocker();
  }, [active, tab, runDocker, scan]);

  async function actOnProject(project: Project, moveToTrash: boolean) {
    if (!report) return;
    if (moveToTrash) setMoving(true);
    setMessage(undefined);
    try {
      await call("act_on_dev_entry", { scan: report.scan, id: project.id, moveToTrash });
      if (moveToTrash) {
        setReport(current => current ? { ...current, total: current.total - project.bytes, rows: current.rows.filter(row => row.id !== project.id) } : undefined);
        setMessage({ text: t("{0} wurde in den Papierkorb verschoben.", project.name) });
      }
    } catch (error) {
      setMessage({ text: describeError(error), problem: true });
    } finally {
      setMoving(false);
      setTarget(undefined);
    }
  }

  const rows = sortRows(report?.rows.filter(row => filter === "all" || row.canTrash) ?? [], sort, (row, key) => key === "kind" ? t(kindLabels[row.kind]) : row[key]);
  const potential = report?.rows.filter(row => row.canTrash).reduce((sum, row) => sum + row.bytes, 0) ?? 0;
  const busy = scanning || moving;

  if (!active) return null;
  return <div className="screen">
    <div className="page-heading">
      <div><h1>{t("Dev-Modus")}</h1><p>{t("Werkzeuge, Paket-Caches, Projektdateien und Docker-Speicher.")}</p></div>
      {(tab === "tools" || tab === "caches") && inventory.started && <InventoryButton scanning={inventory.scanning} onClick={() => inventory.scan(withHistory)} />}
    </div>
    <nav className="section-tabs" aria-label={t("Bereiche im Dev-Modus")}>
      {([["tools", t("Toolchain & Pakete")], ["caches", t("Paket-Caches")], ["projects", t("Projektdateien")], ["docker", "Docker"]] as const).map(([key, title]) =>
        <button key={key} aria-current={tab === key ? "page" : undefined} onClick={() => setTab(key)}>{title}</button>)}
    </nav>
    {tab === "tools" && <ToolchainView control={inventory} activity={withHistory} setActivity={setWithHistory} openCaches={() => setTab("caches")} />}
    {tab === "caches" && <DevCachesView control={inventory} activity={withHistory} />}
    {message && <div role="status" className={message.problem ? "notice error" : "notice"}>{message.text}</div>}
    {tab === "projects" && <section className="panel dev-panel">
      <div className="panel-heading">
        <div className="dev-heading"><span className="panel-icon accent-icon"><TerminalSquare size={19} /></span><div>
          <h2>{t("Entwicklungsordner")}</h2>
          <p>{report ? t("{0} gefunden · {1} mit erkanntem Wiederaufbau", bytes(report.total), bytes(potential)) : t("node_modules, Python-Umgebungen sowie Build-Dateien von Rust, Next.js, Swift und Xcode")}</p>
        </div></div>
        <div className="dev-actions">
          <button className="button secondary compact" disabled={!native || busy} onClick={() => void scan("choose")}><FolderOpen size={16} /> {t("Ordner wählen")}</button>
          {projectsStarted.current && <button className="button secondary compact" disabled={!native || busy} onClick={() => void scan("refresh")}><RotateCw size={15} className={scanning ? "spin" : undefined} /> {t("Aktualisieren")}</button>}
        </div>
      </div>
      {scanning && <p className="dev-note" role="status"><LoaderCircle className="inline-block spin" size={15} /> {t("Projektordner werden geprüft …")}</p>}
      {!report && !scanning && <p className="dev-note">{t("Der Standardsuchlauf prüft übliche Projektordner im Benutzerordner und den Schreibtisch. Für ein anderes Verzeichnis wähle einen Ordner aus.")}</p>}
      {report && <>
        <div className="dev-scope">
          <p title={report.roots.join(" · ")}>{folder ? t("Geprüft: {0}", folder) : t("Geprüft: {0}", report.roots.join(" · ") || t("Keine Standardordner gefunden"))}</p>
          {folder && <button className="text-button" disabled={busy} onClick={() => void scan("default")}>{t("Standardordner verwenden")}</button>}
          {report.skipped > 0 && <p>{number(report.skipped)} {t("Pfade konnten nicht vollständig geprüft werden.")}</p>}
          {report.truncated && <p>{t("Die Liste zeigt die 250 größten Entwicklungsordner.")}</p>}
        </div>
        {report.rows.length > 0 && <div className="dev-filters filters">
          <button className="filter-chip" aria-pressed={filter === "all"} onClick={() => setFilter("all")}>{t("Alle Ordner")}</button>
          <button className="filter-chip" aria-pressed={filter === "rebuildable"} onClick={() => setFilter("rebuildable")}>{t("Wiederaufbau erkannt")}</button>
        </div>}
        {rows.length > 0 ? <div className="table-scroll"><table className="dev-table project-table">
          <thead><tr>
            <SortHeader column="name" sort={sort} label={t("Ordner")} />
            <SortHeader column="kind" sort={sort} label={t("Wiederaufbau")} />
            <SortHeader column="bytes" sort={sort} label={t("Größe")} first="descending" />
            <SortHeader column="modifiedAt" sort={sort} label={t("Geändert am")} first="descending" />
            <th scope="col"><span className="sr-only">{t("Aktionen")}</span></th></tr></thead>
          <tbody>{rows.map(row => <tr key={row.id}>
            <td><div className="file-name"><div><strong title={row.name}>{row.name}</strong><span title={row.path}>{row.path}</span></div></div></td>
            <td><div className="dev-rebuild"><strong>{t(kindLabels[row.kind])}</strong><span>{t(row.rebuild ?? "Projektdateien zuerst prüfen")}</span></div></td>
            <td>{bytes(row.bytes)}</td><td className="file-date">{dateTime(row.modifiedAt)}</td>
            <td><div className="file-actions">
              <button disabled={busy} className="icon-button" aria-label={t("{0} im Finder anzeigen", row.path)} title={t("Im Finder anzeigen")} onClick={() => void actOnProject(row, false)}><FolderOpen size={16} /></button>
              {row.canTrash && <button disabled={busy} className="icon-button danger-icon" aria-label={t("{0} in den Papierkorb verschieben", row.path)} title={t("In den Papierkorb")} onClick={() => setTarget(row)}><Trash2 size={16} /></button>}
            </div></td>
          </tr>)}</tbody>
        </table></div> : <p className="dev-note">{filter === "rebuildable" ? t("Für diese Treffer fehlt ein erkannter Wiederaufbau.") : t("Keine passenden Entwicklungsordner ab 1 MiB gefunden.")}</p>}
      </>}
    </section>}
    {tab === "docker" && <DockerPanel report={docker} started={dockerStarted.current} scanning={dockerScanning} scan={() => void runDocker()}
      open={() => void call("open_docker").catch(error => setMessage({ text: describeError(error), problem: true }))} />}
    {target && report && <DevImpactDialog scan={report.scan} kind="project" id={target.id} title={t("„{0}“ in den Papierkorb?", target.name)}
      detail={<>{target.path} · {bytes(target.bytes)}{t(". Beende laufende Builds und Dev-Server vorher. Wiederaufbau: ")}{t(target.rebuild ?? "—")}{t(". Du kannst den Ordner aus dem Papierkorb wiederherstellen.")}</>}
      action={t("In den Papierkorb")} busy={moving} onClose={() => setTarget(undefined)} onConfirm={() => void actOnProject(target, true)} />}
  </div>;
}
