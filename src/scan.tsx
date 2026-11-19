import { t } from "./language";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { ArrowLeft, ArrowUpRight, Check, ChevronRight, FolderOpen, LoaderCircle, RotateCw, Search, Trash2 } from "lucide-react";
import { bytes, call, dateTime, describeError, isRecommended, native, number, type Finding, type FileRow, type Group, type Mode, type Report } from "./api";
import { Confirmation } from "./confirmation";
import { FileThumbnail } from "./file-thumbnail";
import { type CacheScope, type FileScan } from "./file-scan";
import { SortHeader, sortRows, useSort, type SortState } from "./sort";

const copy = {
  cache: {
    title: "Bereinigung", subtitle: "Alte temporäre Dateien, Caches und mögliche Reste entfernter Apps finden.",
    working: "Installierte Apps und mögliche Bereinigungskandidaten werden geprüft …", result: "Gefundene Dateien",
  },
  storage: {
    title: "Festplatte", subtitle: "Sieh, welche persönlichen Ordner den meisten Platz belegen.",
    working: "Ordner werden vermessen …", result: "Dateien nach Größe",
  },
  files: {
    title: "Dateien & Duplikate", subtitle: "Identische Dateien, große Dateien und alte Downloads aufspüren.",
    working: "Dateien werden geprüft und verglichen …", result: "Dateifunde",
  },
};

const palette = ["#0f172a", "#be123c", "#475569", "#a61a40", "#52647b", "#8f2342"];

type Tile = { group: Group; x: number; y: number; width: number; height: number };
function tileMap(groups: Group[], x = 0, y = 0, width = 100, height = 100): Tile[] {
  if (groups.length === 0) return [];
  if (groups.length === 1) return [{ group: groups[0], x, y, width, height }];
  const total = groups.reduce((sum, group) => sum + group.bytes, 0);
  let split = 1;
  let partial = groups[0].bytes;
  while (split < groups.length - 1 && partial + groups[split].bytes / 2 < total / 2) {
    partial += groups[split].bytes;
    split++;
  }
  const share = partial / total;
  if (width * 2.75 >= height) {
    return [...tileMap(groups.slice(0, split), x, y, width * share, height),
      ...tileMap(groups.slice(split), x + width * share, y, width * (1 - share), height)];
  }
  return [...tileMap(groups.slice(0, split), x, y, width, height * share),
    ...tileMap(groups.slice(split), x, y + height * share, width, height * (1 - share))];
}

function StorageMap({ report, filter, setFilter }: { report: Report; filter: string; setFilter: (key: string) => void }) {
  if (report.total === 0) return null;
  return <section className="panel map-panel">
    <div className="panel-heading"><div><h2>{t("Persönliche Ordner")}</h2><p>{bytes(report.total)} {t("erfasste Dateigrößen")}</p></div><span className="muted">{t("Alte Downloads · ")}{bytes(report.oldDownloads)}</span></div>
    <div className="treemap">
      {tileMap(report.groups).map(({ group, x, y, width, height }) => {
        const color = palette[report.groups.indexOf(group) % palette.length];
        return <button key={group.name} className={`tile ${filter !== "all" && filter !== group.name ? "tile-muted" : ""}`}
          style={{ left: `${x}%`, top: `${y}%`, width: `${width}%`, height: `${height}%`, background: color }}
          aria-label={`${t(group.name)}, ${bytes(group.bytes)}`} aria-pressed={filter === group.name}
          title={`${t(group.name)} · ${bytes(group.bytes)}`}
          onClick={() => setFilter(filter === group.name ? "all" : group.name)}>
          {width > 14 && height > 19 && <><strong>{t(group.name)}</strong><span>{bytes(group.bytes)}</span></>}
        </button>;
      })}
    </div>
    <div className="map-legend">
      {report.groups.map((group, index) => <button key={group.name} className="filter-chip" aria-pressed={filter === group.name} onClick={() => setFilter(filter === group.name ? "all" : group.name)}><i style={{ backgroundColor: palette[index % palette.length] }} />{t(group.name)}</button>)}
    </div>
  </section>;
}

const findingText: Record<Finding, string> = {
  standard: "Manuell prüfen",
  oldTemp: "Ältere temporäre Datei",
  oldLog: "Älteres Protokoll",
  oldCache: "Älterer App-Cache",
  residualCache: "Cache oder Protokoll einer nicht zugeordneten App",
  residualData: "App-Daten oder Einstellungen · nur prüfen",
};

type FileSort = "name" | "bytes" | "modifiedAt";
function FileTable({ rows, selected, toggle, inspect, ask, busy, sort, previewMode, showFindings = false, keepOne = false }: {
  rows: FileRow[]; selected: Set<number>; toggle: (ids: number[], checked: boolean) => void;
  inspect: (id: number, reveal: boolean) => void; ask: (ids: number[]) => void;
  busy: boolean; previewMode: string; sort: SortState<FileSort>; showFindings?: boolean; keepOne?: boolean;
}) {
  const eligible = rows.filter(row => row.finding !== "residualData");
  const bulk = keepOne ? eligible.slice(1) : eligible;
  const allChecked = bulk.length > 0 && bulk.every(row => selected.has(row.id));
  return <div className="table-scroll">
    <table className="file-table sorted-file-table">
      <thead><tr><th scope="col"><input type="checkbox" aria-label={keepOne ? t("Kopien auswählen und eine behalten") : t("Sichtbare Dateien auswählen")} checked={allChecked} disabled={busy || bulk.length === 0} onChange={event => {
        if (keepOne && eligible[0] && event.target.checked) toggle([eligible[0].id], false);
        toggle(bulk.map(row => row.id), event.target.checked);
      }} /></th>
        <SortHeader column="name" label={t("Datei")} sort={sort} />
        <SortHeader column="bytes" label={t("Größe")} sort={sort} first="descending" />
        <SortHeader column="modifiedAt" label={t("Geändert am")} sort={sort} first="descending" />
        <th scope="col"><span className="sr-only">{t("Aktionen")}</span></th></tr></thead>
      <tbody>
        {rows.map(row => <tr key={row.id} className={selected.has(row.id) ? "selected" : ""}>
          <td>{row.finding !== "residualData" && <input type="checkbox" aria-label={t("{0} auswählen", row.name)} checked={selected.has(row.id)} disabled={busy} onChange={event => toggle([row.id], event.target.checked)} />}</td>
          <td><div className="file-name"><FileThumbnail mode={previewMode} id={row.id} name={row.name} open={() => inspect(row.id, false)} /><div>
            <strong title={row.name}>{row.name}</strong><span title={row.path}>{row.path}</span>
            {showFindings && <span className={isRecommended(row.finding) ? "file-guidance suggested" : "file-guidance"}>{t(findingText[row.finding])}</span>}
          </div></div></td>
          <td>{bytes(row.bytes)}{row.hardLinks > 1 && <span className="inventory-sub" title={t("Andere Hardlinks bleiben nutzbar und halten die Dateiblöcke belegt.")}>{row.hardLinks} Hardlinks</span>}</td><td className="file-date">{dateTime(row.modifiedAt)}</td>
          <td><div className="file-actions">
            <button className="icon-button" title={t("Im Finder anzeigen")} aria-label={t("{0} im Finder anzeigen", row.name)} onClick={() => inspect(row.id, true)}><FolderOpen size={16} /></button>
            {row.finding !== "residualData" && <button className="icon-button danger-icon" disabled={busy} title={t("In den Papierkorb")} aria-label={t("{0} in den Papierkorb", row.name)} onClick={() => ask([row.id])}><Trash2 size={16} /></button>}
          </div></td>
        </tr>)}
        {rows.length === 0 && <tr><td colSpan={5} className="empty-row">{t("Keine passenden Dateien gefunden.")}</td></tr>}
      </tbody>
    </table>
  </div>;
}

export function ScanView({ mode, fileScan, scope, onBack, selectRecommended, children }: {
  mode: Mode; fileScan: FileScan; scope?: CacheScope; onBack?: () => void; selectRecommended?: boolean; children?: ReactNode;
}) {
  const info = copy[mode];
  const { report, scanning, moving, error, started, refresh } = fileScan;
  const [selected, setSelected] = useState(() => new Set(selectRecommended
    ? report?.rows.filter(row => isRecommended(row.finding)).map(row => row.id)
    : []));
  const [notice, setNotice] = useState<{ text: string; error?: boolean }>();
  const [filter, setFilter] = useState(mode === "files" ? "heavy" : mode === "cache" && !scope ? "recommended" : "all");
  const [query, setQuery] = useState("");
  const [pendingIds, setPendingIds] = useState<number[]>();
  const sort = useSort<FileSort>("bytes", "descending");

  useEffect(() => {
    setSelected(previous => new Set([...previous].filter(id => report?.rows.some(row => row.id === id))));
  }, [report]);

  const needle = query.toLocaleLowerCase("de-DE").trim();
  const rows = useMemo(() => sortRows(report?.rows.filter(row =>
    (filter === "all" || (mode === "files" ? (filter === "heavy" ? row.bytes >= 104857600 : filter === "duplicate" ? !!row.duplicate : row.oldDownload) : filter === "recommended" ? isRecommended(row.finding) : row.category === filter))
    && (!needle || row.path.toLocaleLowerCase("de-DE").includes(needle))) ?? [], sort, (row, key) => row[key]), [report, filter, needle, mode, sort.key, sort.direction]);
  const grouped = new Map<string, FileRow[]>();
  const duplicateView = mode === "files" && filter === "duplicate";
  const residueView = mode === "cache" && filter === "Mögliche App-Reste";
  if (duplicateView || residueView) {
    for (const row of rows) {
      const key = duplicateView ? row.duplicate : row.appId;
      if (!key) continue;
      const group = grouped.get(key) ?? [];
      group.push(row);
      grouped.set(key, group);
    }
  }

  const selectedRows = report?.rows.filter(row => selected.has(row.id)) ?? [];
  const confirmationRows = report?.rows.filter(row => pendingIds?.includes(row.id)) ?? [];
  const busy = scanning || moving || !started;
  const size = (items: FileRow[]) => items.reduce((sum, row) => sum + row.bytes, 0);

  function selectFilter(next: string) {
    setSelected(new Set());
    setFilter(next);
  }

  function toggle(ids: number[], checked: boolean) {
    setSelected(previous => {
      const next = new Set(previous);
      for (const id of ids) {
        if (checked) next.add(id);
        else next.delete(id);
      }
      return next;
    });
  }

  async function move() {
    try {
      const result = await fileScan.trash(confirmationRows);
      setNotice({ text: [
        result.moved.length ? t("{0} Dateien in den Papierkorb verschoben.", number(result.moved.length)) : "",
        result.failed.length ? t("{0} fehlgeschlagen. {1}", number(result.failed.length), describeError(result.failed[0])) : "",
      ].filter(Boolean).join(" "), error: result.failed.length > 0 });
      toggle(result.moved, false);
    } catch (error) {
      setNotice({ text: describeError(error), error: true });
    } finally {
      setPendingIds(undefined);
    }
  }

  function inspect(id: number, reveal: boolean) {
    void call("inspect_file", { mode: fileScan.mode, id, reveal }).catch(error => setNotice({ text: describeError(error), error: true }));
  }

  const filterOptions = mode === "cache" && !scope ? [
    { key: "recommended", label: t("Empfohlen") }, { key: "Mögliche App-Reste", label: t("Mögliche App-Reste") }, { key: "all", label: t("Alle Treffer") },
    ...(report?.groups.filter(group => group.name !== "Mögliche App-Reste").map(group => ({ key: group.name, label: t(group.name) })) ?? []),
  ] : mode === "files" ? [
    { key: "heavy", label: t("Große Dateien") }, { key: "duplicate", label: t("Duplikate") }, { key: "old", label: t("Alte Downloads") },
  ] : [{ key: "all", label: t("Alle Dateien") }, ...(scope ? [] : report?.groups.map(group => ({ key: group.name, label: t(group.name) })) ?? [])];

  const tableProps = { selected, toggle, inspect, ask: setPendingIds, busy, sort, previewMode: fileScan.mode, showFindings: mode === "cache" };

  return <div className="screen">
    {scope && <button className="text-button back-link" onClick={onBack}><ArrowLeft size={15} /> Apps &amp; Caches</button>}
    <div className="page-heading">
      <div><h1>{scope ? t("Cache von {0}", scope.name) : t(info.title)}</h1><p>{scope?.path ?? t(info.subtitle)}</p></div>
      {started && <button className="button secondary" disabled={busy || !native} onClick={() => {
        setNotice(undefined); void refresh();
      }}><RotateCw size={16} className={scanning ? "spin" : undefined} /> {t("Aktualisieren")}</button>}
    </div>
    {children}
    {notice && <div className={notice.error ? "notice error" : "notice"} role="status">{!notice.error && <Check size={17} />}{notice.text}</div>}
    {error && <div className="notice error" role="status">{error}</div>}
    {(scanning || (native && !started)) && <div className="scan-message" role="status"><LoaderCircle className="spin" size={17} />{scope ? t("Cache-Dateien werden gelesen …") : t(info.working)}</div>}
    {report && <>
      {mode === "cache" && !scope && <section className="panel cleanup-summary">
        <div>
          <h2>{report.recommendedCount ? t("{0} zum Prüfen", bytes(report.recommendedBytes)) : t("Keine vorrangigen Treffer")}</h2>
          <p>{t("Empfehlungen umfassen ältere App-Caches, Protokolle und temporäre Dateien.")}</p>
          {report.recommendedCount > 0 && selectedRows.length !== report.recommendedCount && <button className="button secondary compact" disabled={busy} onClick={() => {
            setFilter("recommended"); setQuery(""); setSelected(new Set(report.rows.filter(row => isRecommended(row.finding)).map(row => row.id)));
          }}>{t("Empfohlene auswählen")}</button>}
        </div>
        <div>
          <h2>{t("Mögliche App-Reste")}</h2>
          <p>{!report.inventoryAvailable ? t("Das App-Inventar war unvollständig.")
            : report.residueCount ? t("{0} Dateien mit Kennungen, die in den üblichen App-Ordnern fehlen.", number(report.residueCount))
              : t("Keine auffälligen App-Kennungen in den geprüften Ordnern.")}</p>
          {report.residueCount > 0 && <button className="text-button" onClick={() => { selectFilter("Mögliche App-Reste"); setQuery(""); }}>{t("App-Reste prüfen")}<ArrowUpRight size={15} /></button>}
        </div>
      </section>}
      {(report.skipped > 0 || report.truncated) && <p className="inventory-footnote scan-footnote">
        {report.skipped > 0 && t("{0} Pfade konnten nicht vollständig geprüft werden. ", number(report.skipped))}
        {report.truncated && t("Der Scan zeigt die größten Treffer je Bereich. Nach der Bereinigung kannst du die Liste aktualisieren.")}
      </p>}
      {mode === "storage" && <StorageMap report={report} filter={filter} setFilter={selectFilter} />}
      <section className="panel scan-results" aria-busy={scanning || moving}>
        <div className="panel-heading"><div><h2>{scope ? t("Cache-Dateien") : t(info.result)}</h2><p>{number(rows.length)} {t("sichtbar · ")}{bytes(size(rows))}</p></div></div>
        <div className="scan-toolbar">
          <div className="filters" aria-label={t("Dateifilter")}>{filterOptions.length > 1 && filterOptions.map(option =>
            <button className="filter-chip" key={option.key} aria-pressed={filter === option.key} onClick={() => selectFilter(option.key)}>{option.label}</button>)}</div>
          <label className="search"><Search size={15} /><span className="sr-only">{t("Dateien suchen")}</span>
            <input value={query} onChange={event => setQuery(event.target.value)} placeholder={t("Dateien suchen")} /></label>
        </div>
        {scope && <p className="filter-guidance">{t("Schließe die App vor dem Verschieben ihrer Cache-Dateien.")}</p>}
        {residueView && <p className="filter-guidance">{t("App-Daten und Einstellungen können persönliche Inhalte enthalten. Sie lassen sich im Finder prüfen.")}</p>}
        {duplicateView && <p className="filter-guidance">{t("Vergleiche die Speicherorte und behalte mindestens eine Kopie je Gruppe.")}</p>}
        {selectedRows.length > 0 && <div className="selection-bar">
          <strong>{number(selectedRows.length)} {t("ausgewählt · ")}{bytes(size(selectedRows))}</strong>
          <button className="text-button" disabled={moving} onClick={() => setSelected(new Set())}>{t("Auswahl aufheben")}</button>
          <button className="button primary compact" disabled={busy} onClick={() => setPendingIds(selectedRows.map(row => row.id))}><Trash2 size={16} /> {t("In den Papierkorb")}</button>
        </div>}
        {grouped.size > 0 ? [...grouped].map(([key, group]) => <div className="duplicate-group" key={key}>
          <h3><ChevronRight size={15} />{duplicateView ? t("{0} sichtbare Kopien · {1} je Datei", group.length, bytes(group[0].bytes)) : `${key} · ${bytes(size(group))}`}</h3>
          <FileTable rows={group} keepOne={duplicateView} {...tableProps} />
        </div>) : <FileTable rows={rows} {...tableProps} />}
      </section>
    </>}
    {pendingIds && confirmationRows.length > 0 && <Confirmation
      title={confirmationRows.length === 1 ? t("„{0}“ in den Papierkorb?", confirmationRows[0].name) : t("Auswahl in den Papierkorb?")}
      detail={t("{0} Datei(en) · {1}. Du kannst sie im macOS-Papierkorb wiederherstellen.{2}{3}", number(confirmationRows.length), bytes(size(confirmationRows)), confirmationRows.some(row => row.hardLinks > 1) ? t(" Weitere Hardlinks halten die zugehörigen Dateiblöcke belegt.") : "", mode === "cache" ? t(" Schließe die betroffenen Apps vorher.") : "")}
      action={moving ? t("Wird verschoben …") : t("In den Papierkorb")} busy={moving} progress={fileScan.progress} onClose={() => setPendingIds(undefined)} onConfirm={() => void move()} />}
  </div>;
}
