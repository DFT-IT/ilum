import { t } from "./language";
import { Fragment, memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ArrowRight, Check, ChevronRight, FolderOpen, HardDrive, LoaderCircle, RotateCw, Search, TerminalSquare, Trash2 } from "lucide-react";
import { bytes, call, dateTime, describeError, native, number } from "./api";
import type { CacheScope } from "./file-scan";
import { SortHeader, sortRows, useSort } from "./sort";
import { DevImpactDialog } from "./dev-impact";

export type ToolKind = "environment" | "runtime" | "cli" | "package";
type ToolRow = {
  id: number; name: string; kind: ToolKind; source: string; version: string | null;
  path: string; lastUsed: number | null; indirect: boolean | null;
  removal: string | null; removalTarget: string | null;
};
export type DevCacheRow = {
  id: number; name: string; path: string; bytes: number; updatedAt: number | null;
  incomplete: boolean; removal: "Papierkorb" | "pnpm" | null;
};
export type ToolInventory = {
  rows: ToolRow[];
  caches: DevCacheRow[];
  history: "off" | "unavailable" | "undated" | "checked";
};
type AppRow = {
  id: string; name: string; version: string | null; installed: boolean; systemApp: boolean;
  lastUsed: number | null; cacheBytes: number; cacheUpdated: number | null; incomplete: boolean; appTarget: number | null;
  locations: { target: number; path: string; bytes: number; canReview: boolean }[];
};
export type SystemInventory = {
  scan: number; scannedAt: number; apps: AppRow[]; cacheBytes: number; skipped: number; tools: ToolInventory;
};

export type InventoryControl = {
  inventory?: SystemInventory; scanning: boolean; started: boolean; error: string; scan: (withHistory?: boolean) => void;
};

const days = (reference: number, stamp: number) => Math.max(0, Math.floor((reference - stamp) / 86400));
const appOlder = (row: AppRow, at: number) => row.installed && !row.systemApp && row.lastUsed != null && days(at, row.lastUsed) >= 90;
const kindLabels: Record<ToolKind, string> = { environment: "Entwicklungsumgebung", runtime: "Laufzeit & Compiler", cli: "CLI", package: "Paket" };
const PAGE_SIZE = 35;

export function useSystemInventory(automatic: boolean): InventoryControl {
  const [inventory, setInventory] = useState<SystemInventory>();
  const [scanning, setScanning] = useState(false);
  const [started, setStarted] = useState(false);
  const [error, setError] = useState("");

  const running = useRef(false);
  const scan = useCallback(async (withHistory = false) => {
    if (!native || running.current) return;
    running.current = true;
    setStarted(true);
    setScanning(true);
    setError("");
    try {
      setInventory(await call<SystemInventory>("scan_inventory", { commandHistory: withHistory }));
    } catch (error) {
      setError(describeError(error));
    } finally {
      running.current = false;
      setScanning(false);
    }
  }, []);

  useEffect(() => {
    if (automatic && !started) void scan();
  }, [automatic, scan, started]);

  return useMemo(() => ({ inventory, scanning, started, error, scan }), [inventory, scanning, started, error, scan]);
}

export function InventoryButton({ scanning, onClick }: { scanning: boolean; onClick: () => void }) {
  return <button className="button secondary" disabled={!native || scanning} onClick={onClick}>
    <RotateCw size={16} className={scanning ? "spin" : undefined} /> {t("Aktualisieren")}</button>;
}

export function InventoryStatus({ control }: { control: InventoryControl }) {
  return <>
    {control.error && <div className="notice error" role="status">{control.error}</div>}
    {control.scanning && <div className="scan-message" role="status"><LoaderCircle className="spin" size={16} /> {t("Apps, Cache-Ordner und Entwicklungswerkzeuge werden erfasst …")}</div>}
  </>;
}

export function SystemScanSummary({ control, openApps, openTools }: {
  control: InventoryControl; openApps: () => void; openTools: () => void;
}) {
  const { inventory, scanning } = control;
  if (!inventory) return <InventoryStatus control={control} />;
  return <>
    <InventoryStatus control={control} />
    <section className="panel inventory-overview" aria-label={t("Ergebnis des Systemscans")} aria-busy={scanning}>
      <div><h2>{t("Letzter Systemscan")}</h2><p>{new Date(inventory.scannedAt * 1000).toLocaleString("de-DE", { dateStyle: "medium", timeStyle: "short" })}</p></div>
      <button className="inventory-link" onClick={openApps}><HardDrive size={18} /><span><strong>{bytes(inventory.cacheBytes)} {t("Cache-Belegung")}</strong><small>{number(inventory.apps.filter(row => row.installed).length)} {t("erkannte Apps")}</small></span><ArrowRight size={16} /></button>
      <button className="inventory-link" onClick={openTools}><TerminalSquare size={18} /><span><strong>{number(inventory.tools.rows.length)} {t("Dev-Funde")}</strong><small>{t("Umgebungen, Laufzeiten, CLIs und Pakete")}</small></span><ArrowRight size={16} /></button>
    </section>
  </>;
}

function SearchField({ query, setQuery, label }: { query: string; setQuery: (value: string) => void; label: string }) {
  return <label className="search inventory-search"><Search size={16} /><span className="sr-only">{label}</span>
    <input value={query} placeholder={label} onChange={event => setQuery(event.target.value)} />
  </label>;
}

function InventoryEmpty({ tools = false }: { tools?: boolean }) {
  return <section className="panel inventory-empty">
    <span className="panel-icon accent-icon">{tools ? <TerminalSquare size={20} /> : <HardDrive size={20} />}</span>
    <div><h2>{tools ? t("Entwicklungswerkzeuge erfassen") : t("Apps und ihre Caches erfassen")}</h2>
      <p>{tools ? t("Der Systemscan findet Installationen aus gängigen Werkzeug- und Paketordnern, auch wenn ein Tool gerade nicht im PATH liegt.")
        : t("Der Systemscan ordnet Standard- und Container-Caches Apps zu und zeigt, wann macOS eine App zuletzt geöffnet hat.")}</p></div>
  </section>;
}

type AppFilter = "all" | "cache" | "old" | "unassigned";

export const AppsView = memo(function AppsView({ active, control, openCleanup, reviewCache }: {
  active: boolean; control: InventoryControl; openCleanup: () => void; reviewCache: (scope: CacheScope) => void;
}) {
  const { inventory, scanning, scan } = control;
  const [filter, setFilter] = useState<AppFilter>("all");
  const sort = useSort<"cacheBytes" | "lastUsed" | "cacheUpdated" | "name">("cacheBytes", "descending");
  const [query, setQuery] = useState("");
  const [limit, setLimit] = useState(PAGE_SIZE);
  const [expanded, setExpanded] = useState<string>();
  const [error, setError] = useState("");
  const reference = inventory?.scannedAt ?? 0;
  const counts = useMemo(() => ({
    installed: inventory?.apps.filter(row => row.installed).length ?? 0,
    cache: inventory?.apps.filter(row => row.cacheBytes > 0).length ?? 0,
    old: inventory?.apps.filter(row => appOlder(row, reference)).length ?? 0,
    known: inventory?.apps.filter(row => row.installed && row.lastUsed != null).length ?? 0,
    unassigned: inventory?.apps.filter(row => !row.installed).length ?? 0,
  }), [inventory, reference]);
  const rows = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return sortRows((inventory?.apps ?? []).filter(row =>
      (!needle || `${row.name} ${row.id}`.toLowerCase().includes(needle))
      && (filter === "all" || (filter === "cache" && row.cacheBytes > 0)
        || (filter === "old" && appOlder(row, reference)) || (filter === "unassigned" && !row.installed))), sort, (row, key) => row[key]);
  }, [inventory, filter, sort.key, sort.direction, query, reference]);

  function reveal(target: number) {
    if (!inventory) return;
    setError("");
    void call("reveal_inventory_item", { scan: inventory.scan, target }).catch(error => setError(describeError(error)));
  }

  function inspectCache(row: AppRow, location: AppRow["locations"][number]) {
    if (inventory) reviewCache({ scan: inventory.scan, target: location.target, name: row.name, path: location.path });
  }

  if (!active) return null;

  return <div className="screen">
    <div className="page-heading"><div><h1>Apps &amp; Caches</h1><p>{t("Sieh, was Platz belegt und welche Apps du schon länger nicht geöffnet hast.")}</p></div>
      {control.started && <InventoryButton scanning={scanning} onClick={() => scan()} />}</div>
    <InventoryStatus control={control} />
    {error && <div role="status" className="notice error">{error}</div>}
    {!inventory && !scanning && <InventoryEmpty />}
    {inventory && <>
      <dl className="inventory-stats">
        <div><dt>{t("Erkannte Apps")}</dt><dd>{number(counts.installed)}</dd></div>
        <div><dt>{t("Erfasste Cache-Belegung")}</dt><dd>{bytes(inventory.cacheBytes)}</dd></div>
        <div><dt>{t("Letztes Öffnen 90+ Tage her")}</dt><dd>{number(counts.old)}</dd><span className="inventory-sub">{t("Öffnungsdatum für ")}{number(counts.known)} {t("von ")}{number(counts.installed)} Apps</span></div>
      </dl>
      <section className="panel inventory-panel" aria-busy={scanning}>
        <div className="panel-heading"><div><h2>{t("App-Inventar")}</h2><p>{inventory.skipped > 0 ? t("{0} Pfade konnten nicht vollständig gelesen werden.", number(inventory.skipped)) : t("Nutzungsdaten stammen aus macOS-Metadaten.")}</p></div>
          <button className="text-button" onClick={openCleanup}>{t("Bereinigung öffnen")}<ArrowRight size={15} /></button></div>
        <div className="inventory-toolbar">
          <div className="filters" role="group" aria-label={t("Apps filtern")}>
            {([["all", t("Alle"), inventory.apps.length], ["cache", t("Mit Cache"), counts.cache], ["old", t("90+ Tage"), counts.old], ["unassigned", t("Ohne App-Zuordnung"), counts.unassigned]] as const)
              .map(([key, title, count]) => <button className="filter-chip" key={key} aria-pressed={filter === key} onClick={() => { setFilter(key); setLimit(PAGE_SIZE); }}>{title} <span>{number(count)}</span></button>)}
          </div>
          <SearchField query={query} setQuery={value => { setQuery(value); setLimit(PAGE_SIZE); }} label={t("App suchen")} />
        </div>
        {filter === "old" && <p className="inventory-hint">{t("Nur Apps mit einem bekannten Datum erscheinen hier. Prüfe vor einer Deinstallation, ob du die App oder ihre Daten noch brauchst.")}</p>}
        {filter === "unassigned" && <p className="inventory-hint">{t("Gemeinsam genutzte oder anders benannte Caches können hier auftauchen. Prüfe den Ordner, um seine Herkunft einzuordnen.")}</p>}
        <div className="table-scroll">
          <table className="inventory-table apps-table">
            <thead><tr>
              <SortHeader column="name" sort={sort} label={t("App oder Cache")} />
              <SortHeader column="cacheBytes" sort={sort} label="Cache" first="descending" />
              <SortHeader column="lastUsed" sort={sort} label={t("Geöffnet am")} first="descending" />
              <SortHeader column="cacheUpdated" sort={sort} label={t("Cache geändert")} first="descending" />
              <th scope="col"><span className="sr-only">{t("Aktionen")}</span></th>
            </tr></thead>
            <tbody>{rows.slice(0, limit).map(row => {
              const primary = row.locations.find(location => location.canReview && location.bytes > 0);
              return <Fragment key={row.id}>
                <tr className={expanded === row.id ? "inventory-expanded" : undefined}>
                  <td><div className="inventory-name"><strong title={row.name}>{row.name}</strong><span>{row.installed ? [row.version, row.systemApp ? t("macOS-Systemapp") : row.id].filter(Boolean).join(" · ") : t("Ohne App-Zuordnung")}</span></div></td>
                  <td>{row.cacheBytes > 0 ? bytes(row.cacheBytes) : "—"}{row.incomplete && <span className="inventory-sub">{t("Unvollständig")}</span>}</td>
                  <td>{row.lastUsed != null ? dateTime(row.lastUsed) : t("Keine Angabe")}{appOlder(row, reference) && <span className="inventory-sub dormant">{days(reference, row.lastUsed!)} {t("Tage zurück")}</span>}</td>
                  <td>{dateTime(row.cacheUpdated)}</td>
                  <td><div className="app-row-actions">
                    {primary && <button className="text-button cache-row-action" onClick={() => inspectCache(row, primary)}>{t("Cache prüfen")}</button>}
                    <button className="icon-button expander" aria-expanded={expanded === row.id} aria-label={t("Details für {0}", row.name)} onClick={() => setExpanded(expanded === row.id ? undefined : row.id)}><ChevronRight size={17} /></button>
                  </div></td>
                </tr>
                {expanded === row.id && <tr className="inventory-detail"><td colSpan={5}>
                  <div>
                    <span>{row.locations.length ? t("Erfasste Cache-Pfade") : t("In den geprüften Ordnern wurde kein Cache zugeordnet.")}</span>
                    {row.appTarget !== null && <button className="text-button" onClick={() => reveal(row.appTarget!)}><FolderOpen size={15} /> {t("App im Finder")}</button>}
                  </div>
                  {row.locations.map(location => <div key={location.target}>
                    <span title={location.path}>{location.path} · {bytes(location.bytes)}</span>
                    <div className="app-row-actions">
                      {location.canReview && <button className="text-button" onClick={() => inspectCache(row, location)}>{t("Cache prüfen")}</button>}
                      <button className="text-button" onClick={() => reveal(location.target)}>{t("Im Finder")}<FolderOpen size={15} /></button>
                    </div>
                  </div>)}
                </td></tr>}
              </Fragment>;
            })}
              {rows.length === 0 && <tr><td colSpan={5} className="empty-row">{t("Keine Funde für diese Auswahl.")}</td></tr>}
            </tbody>
          </table>
        </div>
        {rows.length > limit && <div className="inventory-more"><span>{number(limit)} {t("von ")}{number(rows.length)} {t("Funden")}</span><button className="button secondary compact" onClick={() => setLimit(value => value + PAGE_SIZE)}>{t("Mehr anzeigen")}</button></div>}
      </section>
      <p className="inventory-footnote">{t("Cache-Größen basieren auf belegten Dateiblöcken der erfassten Ordner. Gemeinsame APFS-Blöcke können den tatsächlich frei werdenden Platz verändern. Ohne macOS-Nutzungsdatum bleibt die Nutzung unbekannt.")}</p>
    </>}
  </div>;
});

export const ToolchainView = memo(function ToolchainView({ control, activity, setActivity, openCaches }: {
  control: InventoryControl; activity: boolean; setActivity: (value: boolean) => void; openCaches: () => void;
}) {
  const { inventory, scanning } = control;
  const report = inventory?.tools;
  const [onlyRemovable, setOnlyRemovable] = useState(false);
  const [selection, setSelection] = useState<{ row: ToolRow; scan: number }>();
  const [removing, setRemoving] = useState(false);
  const [feedback, setFeedback] = useState<{ text: string; error?: boolean }>();
  const [filter, setFilter] = useState<"all" | ToolKind>("all");
  const [query, setQuery] = useState("");
  const [limit, setLimit] = useState(PAGE_SIZE);
  const sort = useSort<"name" | "source" | "version" | "lastUsed">("name", "ascending");
  const counts = useMemo(() => {
    const totals: Record<ToolKind, number> = { environment: 0, runtime: 0, cli: 0, package: 0 };
    for (const row of report?.rows ?? []) totals[row.kind]++;
    return totals;
  }, [report]);
  const rows = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return sortRows((report?.rows ?? []).filter(row =>
      (filter === "all" || row.kind === filter) && (!onlyRemovable || row.removal) && (!needle || `${row.name} ${row.version ?? ""} ${row.source} ${row.path}`.toLowerCase().includes(needle))), sort, (row, key) => row[key]);
  }, [report, filter, onlyRemovable, query, sort.key, sort.direction]);

  async function removeSelected() {
    if (!selection || removing) return;
    const { row, scan } = selection;
    setRemoving(true);
    setFeedback(undefined);
    try {
      await call("remove_inventory_tool", { scan, id: row.id });
      setFeedback({ text: t("„{0}“ wurde entfernt.", row.removalTarget ?? row.name) });
      control.scan(activity);
    } catch (error) {
      setFeedback({ text: describeError(error), error: true });
    } finally {
      setRemoving(false);
      setSelection(undefined);
    }
  }

  return <>
    <InventoryStatus control={control} />
    {feedback && <div className={feedback.error ? "notice error" : "notice tool-success"} role="status">
      {!feedback.error && <Check size={16} />} {feedback.text}
    </div>}
    {!report && !scanning && <InventoryEmpty tools />}
    <section className="panel inventory-options">
      <div><h2>{t("CLI-Nutzung aus zsh")}</h2><p>{t("Optional liest der Scan lokal Zeitstempel und Befehlsnamen aus ")}<code>~/.zsh_history</code>{t(". Das Inventar speichert für die Anzeige nur Befehlsnamen und Zeitstempel.")}</p></div>
      <label className="inline-checkbox"><input type="checkbox" checked={activity} onChange={event => setActivity(event.target.checked)} /> {t("Beim Aktualisieren einbeziehen")}</label>
    </section>
    {report && <>
      <dl className="inventory-stats tool-stats">
        <div><dt>{t("Umgebungen")}</dt><dd>{number(counts.environment)}</dd></div>
        <div><dt>{t("Laufzeiten & Compiler")}</dt><dd>{number(counts.runtime)}</dd></div>
        <div><dt>CLIs</dt><dd>{number(counts.cli)}</dd></div>
        <div><dt>{t("Pakete")}</dt><dd>{number(counts.package)}</dd></div>
      </dl>
      <section className="panel inventory-panel" aria-busy={scanning}>
        <div className="panel-heading"><div><h2>{t("Installierte Werkzeuge und Pakete")}</h2><p>{report.history === "checked" ? t("CLI-Daten zeigen den letzten direkten Aufruf unter diesem Befehlsnamen.") : report.history === "unavailable" ? t("Der Standardverlauf von zsh war nicht verfügbar.") : report.history === "undated" ? t("Der zsh-Verlauf enthält keine auswertbaren Zeitstempel.") : t("CLI-Nutzungsdaten sind optional.")}</p></div></div>
        <div className="inventory-toolbar">
          <div className="filters" role="group" aria-label={t("Werkzeugtyp filtern")}>{([["all", t("Alle")], ["environment", t("Umgebungen")], ["runtime", t("Laufzeiten")], ["cli", "CLIs"], ["package", t("Pakete")]] as const)
            .map(([key, title]) => <button className="filter-chip" key={key} aria-pressed={filter === key} onClick={() => { setFilter(key); setLimit(PAGE_SIZE); }}>{title}</button>)}</div>
          <div className="tool-inventory-controls">
            <button className="filter-chip" aria-pressed={onlyRemovable} onClick={() => { setOnlyRemovable(!onlyRemovable); setLimit(PAGE_SIZE); }}>{t("Entfernbar")}</button>
            <SearchField query={query} setQuery={value => { setQuery(value); setLimit(PAGE_SIZE); }} label={t("Werkzeug suchen")} />
          </div>
        </div>
        <div className="table-scroll"><table className="inventory-table tools-table">
          <thead><tr>
            <SortHeader column="name" sort={sort} label={t("Werkzeug")} />
            <SortHeader column="source" sort={sort} label={t("Quelle")} />
            <SortHeader column="version" sort={sort} label="Version" first="descending" />
            <SortHeader column="lastUsed" sort={sort} label={t("Letzte Nutzung")} first="descending" />
            <th scope="col"><span className="sr-only">{t("Aktionen")}</span></th>
          </tr></thead>
          <tbody>{rows.slice(0, limit).map(row => <tr key={row.id}>
            <td><div className="inventory-name"><strong>{row.name}</strong><span title={row.path}>{row.path}</span></div></td>
            <td>{t(row.source)}<span className="inventory-sub">{row.indirect ? t("Abhängigkeit") : t(kindLabels[row.kind])}</span></td><td>{row.version ?? "—"}</td>
            <td>{row.lastUsed ? <>{dateTime(row.lastUsed)}<span className="inventory-sub">{row.kind === "environment" ? t("App geöffnet") : t("CLI-Aufruf")}</span></> : "—"}</td>
            <td>{row.removal ? <button className="text-button tool-remove" aria-label={t("{0} entfernen", row.name)}
              title={t("Über {0} entfernen", t(row.removal))} disabled={scanning || removing}
              onClick={() => { if (inventory) setSelection({ row, scan: inventory.scan }); }}><Trash2 size={14} /> {t("Entfernen")}</button>
              : <span className="inventory-sub tool-unavailable" title={t("Für diese Installation ist kein eindeutiger Entfernen-Weg verfügbar.")}>—</span>}</td>
          </tr>)}
            {rows.length === 0 && <tr><td className="empty-row" colSpan={5}>{t("Keine Funde für diese Auswahl.")}</td></tr>}
          </tbody>
        </table></div>
        {rows.length > limit && <div className="inventory-more"><span>{number(limit)} {t("von ")}{number(rows.length)} {t("Funden")}</span><button className="button secondary compact" onClick={() => setLimit(value => value + PAGE_SIZE)}>{t("Mehr anzeigen")}</button></div>}
      </section>
      {report.caches.length > 0 && <button className="text-button tool-cache-shortcut" onClick={openCaches}>
        {t("Paket- und Build-Caches ansehen")}<ArrowRight size={15} />
      </button>}
      <p className="inventory-footnote">{t("Das Inventar liest bekannte Installationsorte und Manifeste. Eine fehlende Version oder Nutzungsangabe bleibt offen. macOS-Starter können beim ersten Aufruf weitere Werkzeuge benötigen.")}</p>
    </>}
    {selection && <DevImpactDialog scan={selection.scan} id={selection.row.id} kind="tool"
      title={t("„{0}“ entfernen?", selection.row.removalTarget ?? selection.row.name)}
      detail={selection.row.removal === "Papierkorb"
        ? t("Die Installation wird in den Papierkorb verschoben. Beende das Werkzeug und laufende Builds vorher.")
        : t("{0} deinstalliert die Installation. Laufende Projekte können das Werkzeug benötigen. Du kannst es über denselben Paketmanager erneut installieren.", selection.row.removal ?? "")}
      action={selection.row.removal === "Papierkorb" ? t("In den Papierkorb") : t("Deinstallieren")}
      busy={removing} onClose={() => setSelection(undefined)} onConfirm={() => void removeSelected()} />}
  </>;
});
