import { t } from "./language";
import { listen } from "@tauri-apps/api/event";
import { ArrowDown, ArrowRight, ArrowUp, ChartNoAxesCombined, FolderOpen, HardDrive, RotateCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { bytes, call, dateTime, describeError, native, number } from "./api";

const areas: Record<string, string> = {
  downloads: "Downloads", desktop: "Schreibtisch", documents: "Dokumente",
  appCaches: "App-Caches", xcode: "Xcode DerivedData", docker: "Docker Desktop",
  npm: "npm-Cache", cargo: "Cargo-Registry",
};
const periods = { previous: "Letzte Messung", day: "Ca. 24 Stunden", week: "Ca. 7 Tage" };
type Change = { area: string; name: string; before: number; after: number };
type Comparison = {
  period: keyof typeof periods; from: number; freeDelta: number | null;
  growing: Change[]; totalChanges: number; partialAreas: number;
};
type Report = { measuredAt: number | null; freeBytes: number | null; comparisons: Comparison[] };

export function useGrowth() {
  const [report, setReport] = useState<Report>();
  const [busy, setBusy] = useState(native);
  const [error, setError] = useState("");

  const measure = useCallback(async (refresh = false) => {
    if (!native) return;
    if (refresh) setBusy(true);
    try {
      const next = await call<Report>("storage_growth", { refresh });
      setReport(current => (next.measuredAt ?? 0) >= (current?.measuredAt ?? 0) ? next : current);
      setError("");
    } catch (error) {
      setError(describeError(error));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    if (!native) return;
    const listeners = ["surface-shown", "storage-growth-updated"].map(event => listen(event, () => void measure()));
    void measure();
    return () => { for (const listener of listeners) void listener.then(unlisten => unlisten()).catch(() => {}); };
  }, [measure]);

  return { report, busy, error, measure };
}
type Control = ReturnType<typeof useGrowth>;

function freeChange(delta: number | null) {
  if (delta == null) return t("Änderung nicht verfügbar");
  if (delta === 0) return t("Freier Speicher unverändert");
  return t("{0} {1} frei", bytes(Math.abs(delta)), delta < 0 ? t("weniger") : t("mehr"));
}

export function GrowthShortcut({ control, open }: { control: Control; open: () => void }) {
  const comparison = control.report?.comparisons[0];
  const leader = comparison?.growing[0];
  const hint = leader
    ? t("{0} · +{1} seit der letzten Messung", t(areas[leader.area] ?? leader.area), bytes(leader.after - leader.before))
    : comparison ? freeChange(comparison.freeDelta)
      : control.report?.measuredAt ? t("Erste Messung gespeichert")
        : control.error ? t("Speicherverlauf öffnen") : t("Erste Messung läuft …");
  return <button type="button" className="growth-shortcut" onClick={open}>
    <span className="growth-shortcut-icon"><ChartNoAxesCombined size={18} aria-hidden="true" /></span>
    <span><strong>{t("Was ist gewachsen?")}</strong><small>{hint}</small></span>
    <ArrowRight size={16} aria-hidden="true" />
  </button>;
}

export function GrowthView({ control }: { control: Control }) {
  const { report, busy, error, measure } = control;
  const [period, setPeriod] = useState<Comparison["period"]>("previous");
  const [revealError, setRevealError] = useState("");
  const comparison = report?.comparisons.find(item => item.period === period) ?? report?.comparisons[0];
  const delta = comparison?.freeDelta;
  const max = comparison?.growing[0];
  const maxGrowth = max ? max.after - max.before : 1;

  async function reveal(change: Change) {
    setRevealError("");
    try {
      await call("reveal_storage_growth", { area: change.area, name: change.name });
    } catch (error) {
      setRevealError(describeError(error));
    }
  }

  return <div className="screen growth-screen">
    <div className="page-heading">
      <div><h1>{t("Was ist gewachsen?")}</h1><p>{t("Sieh, wo seit der letzten Messung mehr Speicher belegt ist.")}</p></div>
      <button type="button" className="button secondary compact" disabled={busy || !native} onClick={() => void measure(true)}>
        <RotateCw size={15} aria-hidden="true" className={busy ? "spin" : undefined} />
        {busy ? t("Wird gemessen …") : t("Jetzt messen")}
      </button>
    </div>
    {(error || revealError) && <div className="notice error" role="status">{error || revealError}</div>}
    {!comparison ? <section className="panel growth-empty" role="status">
      <span className="growth-shortcut-icon"><ChartNoAxesCombined size={22} aria-hidden="true" /></span>
      <div>
        <h2>{report?.measuredAt ? t("Erste Messung gespeichert") : busy ? t("Erste Messung läuft") : t("Noch keine Messung verfügbar")}</h2>
        <p>{report?.measuredAt
          ? t("Seit {0}. Sobald eine zweite Messung vorliegt, siehst du die Veränderungen hier.", dateTime(report.measuredAt))
          : busy ? t("Ilum erfasst das Startvolume, persönliche Ordner und ausgewählte Entwickler-Caches.")
            : t("Starte eine Messung in der macOS-App.")}</p>
      </div>
    </section> : <>
      <div className="growth-controls">
        <div className="section-tabs" role="group" aria-label={t("Vergleichszeitraum")}>
          {Object.entries(periods).map(([key, label]) => {
            const available = report?.comparisons.some(item => item.period === key);
            return <button type="button" key={key} disabled={!available}
              title={available ? undefined : t("Für diesen Zeitraum fehlt noch eine Messung")}
              aria-current={comparison.period === key ? "true" : undefined}
              onClick={() => setPeriod(key as Comparison["period"])}>{t(label)}</button>;
          })}
        </div>
        <span className="muted growth-dates">{dateTime(comparison.from)} – {dateTime(report?.measuredAt)}</span>
      </div>
      <section className="panel growth-volume" aria-label={t("Freier Speicher auf dem Startvolume")}>
        <span className="growth-shortcut-icon"><HardDrive size={19} aria-hidden="true" /></span>
        <div><h2>{t("Startvolume")}</h2><p>{report?.freeBytes == null ? t("Freier Speicher nicht verfügbar") : t("{0} aktuell frei", bytes(report.freeBytes))}</p></div>
        <strong data-direction={delta != null && delta < 0 ? "down" : "up"}>
          {delta != null && delta !== 0 && (delta < 0 ? <ArrowDown size={15} aria-hidden="true" /> : <ArrowUp size={15} aria-hidden="true" />)}
          {freeChange(delta ?? null)}
        </strong>
      </section>
      <section className="panel growth-list">
        <div className="panel-heading">
          <div><h2>{t("Größte Zuwächse")}</h2>
            <p>{comparison.totalChanges > comparison.growing.length
              ? t("Die {0} größten von {1} Funden", comparison.growing.length, number(comparison.totalChanges))
              : comparison.totalChanges ? t("{0} gewachsene Einträge · nach Zuwachs sortiert", number(comparison.totalChanges))
                : t("In den vergleichbaren Ordnern wurde kein Zuwachs gemessen.")}</p></div>
        </div>
        {comparison.growing.length > 0 && <ul className="growth-rows">
          {comparison.growing.map(change => <li key={`${change.area}/${change.name}`}>
            <div className="growth-row-name"><strong title={change.name}>{change.name}</strong><small>{t(areas[change.area] ?? change.area)}</small></div>
            <div className="growth-row-change">
              <strong>+{bytes(change.after - change.before)}</strong>
              <span className="growth-bar" aria-hidden="true"><i style={{ width: `${100 * (change.after - change.before) / maxGrowth}%` }} /></span>
              <small>{bytes(change.before)} → {bytes(change.after)}</small>
            </div>
            <button type="button" className="icon-button" title={t("Im Finder anzeigen")} aria-label={t("{0} im Finder anzeigen", change.name)} onClick={() => void reveal(change)}>
              <FolderOpen size={17} aria-hidden="true" />
            </button>
          </li>)}
        </ul>}
      </section>
      {comparison.partialAreas > 0 && <p className="growth-note">{comparison.partialAreas === 1
        ? t("Ein Bereich war nur teilweise messbar.") : t("{0} Bereiche waren nur teilweise messbar.", comparison.partialAreas)} {t("Unvollständige Einträge fließen nicht in den Vergleich ein.")}</p>}
    </>}
    <p className="growth-note">{t("Ilum speichert Messungen lokal und misst bei laufender App etwa alle sechs Stunden. Die Liste deckt ausgewählte Ordner ab. Andere Daten und gemeinsam genutzte Dateiblöcke können die Änderung am Startvolume beeinflussen.")}</p>
  </div>;
}
