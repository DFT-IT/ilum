import { t } from "./language";
import { useState } from "react";
import { Check, FolderOpen, Layers, Link2, ScanSearch } from "lucide-react";
import { bytes, call, dateTime, describeError, number } from "./api";
import { InventoryStatus, type DevCacheRow, type InventoryControl } from "./inventory";
import { DevImpactDialog } from "./dev-impact";
import { SortHeader, sortRows, useSort } from "./sort";

type Result = { processed: number; remaining: number; inTrash: boolean };

function description(item: DevCacheRow) {
  if (item.name === "Maven") return t("Enthält auch lokal veröffentlichte Artefakte. Diese können für Builds nötig sein.");
  if (item.incomplete) return t("Nur teilweise lesbar");
  if (item.removal === "pnpm") return t("pnpm bereinigt seinen eigenen Speicher");
  if (item.bytes === 0) return t("Keine belegten Dateiblöcke");
  if (!item.removal) return t("Cache kann im Finder geprüft werden");
  return t("Cache-Ordner, wiederherstellbar aus dem Papierkorb");
}

export function DevCachesView({ control, activity }: { control: InventoryControl; activity: boolean }) {
  const { inventory, scanning } = control;
  const [itemsAfterCleanup, setItemsAfterCleanup] = useState<{ scan: number; results: Record<number, Result> }>();
  const [selected, setSelected] = useState<{ scan: number; item: DevCacheRow; checkOnly: boolean }>();
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<{ text: string; problem?: boolean }>();
  const sort = useSort<"name" | "bytes" | "updatedAt">("bytes", "descending");
  const current = inventory?.scan === itemsAfterCleanup?.scan ? itemsAfterCleanup?.results : undefined;
  const rows = sortRows((inventory?.tools.caches ?? []).map(row => {
    const after = current?.[row.id];
    if (!after) return row;
    return { ...row, bytes: after.remaining, removal: null };
  }), sort, (row, key) => row[key]);
  const total = rows.reduce((count, row) => count + row.bytes, 0);
  const trashable = rows.filter(row => row.removal === "Papierkorb").reduce((count, row) => count + row.bytes, 0);

  async function clean() {
    if (!selected || !selected.item.removal || busy) return;
    const { scan, item } = selected;
    setFeedback(undefined);
    setBusy(true);
    try {
      const result = await call<Result>("clean_dev_cache", { scan, id: item.id });
      setItemsAfterCleanup(previous => ({
        scan, results: { ...(previous?.scan === scan ? previous.results : {}), [item.id]: result },
      }));
      setFeedback({ text: result.inTrash ? t("{0}: {1} im Papierkorb.", item.name, bytes(result.processed))
        : result.processed ? t("pnpm hat {0} aus dem Store entfernt. {1} bleiben belegt.", bytes(result.processed), bytes(result.remaining))
          : t("pnpm hat den Store geprüft. Die gemessene Belegung liegt bei {0}.", bytes(result.remaining)) });
      control.scan(activity);
    } catch (error) {
      setFeedback({ text: describeError(error), problem: true });
    } finally {
      setBusy(false);
      setSelected(undefined);
    }
  }

  function reveal(id: number) {
    if (!inventory) return;
    setFeedback(undefined);
    void call("reveal_dev_cache", { scan: inventory.scan, id }).catch(error => setFeedback({ text: describeError(error), problem: true }));
  }

  return <>
    <InventoryStatus control={control} />
    {feedback && <div className={feedback.problem ? "notice error" : "notice tool-success"} role="status">{!feedback.problem && <Check size={16} />}{feedback.text}</div>}
    {inventory && <section className="panel inventory-panel cache-management" aria-busy={scanning}>
      <header className="panel-heading">
        <div className="dev-heading"><span className="panel-icon accent-icon"><Layers size={19} /></span>
          <div><h2>{t("Paket- und Build-Caches")}</h2><p>{t("Größen und mögliche Auswirkungen vor dem Entfernen prüfen.")}</p></div>
        </div>
      </header>
      <dl className="cache-metrics">
        <div><dt>{t("Erfasste Belegung")}</dt><dd>{bytes(total)}</dd></div>
        <div><dt>{t("Ordner mit Papierkorb-Option")}</dt><dd>{bytes(trashable)}</dd></div>
      </dl>
      <details className="cache-reference-help">
        <summary><Link2 size={15} /> {t("Was passiert bei Verweisen?")}</summary>
        <p>{t("Ein symbolischer Link bricht, wenn sein Ziel fehlt. Ein weiterer Hardlink hält dieselbe Datei und ihre Speicherblöcke erreichbar. Nach dem Leeren eines Download-Caches können Builds länger dauern oder offline scheitern.")}</p>
        <p>{t("Ein Projektmanifest zeigt, welches Werkzeug oder Paket ein Projekt erwartet. Direkte Links und laufende Prozesse zeigt der Check separat an.")}</p>
      </details>
      <div className="table-scroll"><table className="inventory-table dev-cache-table">
        <thead><tr>
          <SortHeader column="name" sort={sort} label="Cache" />
          <SortHeader column="bytes" sort={sort} label={t("Belegt")} first="descending" />
          <SortHeader column="updatedAt" sort={sort} label={t("Geändert am")} first="descending" />
          <th scope="col"><span className="sr-only">{t("Aktionen")}</span></th>
        </tr></thead>
        <tbody>{rows.map(item => <tr key={item.id}>
          <td><div className="inventory-name"><strong>{item.name}</strong><span title={item.path}>{item.path}</span></div><span className="cache-row-detail">{description(item)}</span></td>
          <td>{bytes(item.bytes)}</td><td>{dateTime(item.updatedAt)}</td>
          <td><div className="file-actions">
            <button className="icon-button" title={t("Im Finder anzeigen")} aria-label={t("{0} im Finder anzeigen", item.name)} disabled={scanning || Boolean(current?.[item.id]?.inTrash)} onClick={() => reveal(item.id)}><FolderOpen size={16} /></button>
            <button className={item.removal ? "text-button cache-review-action" : "icon-button"} title={t("Abhängigkeits-Check")}
              aria-label={t("Abhängigkeiten von {0} prüfen", item.name)} disabled={scanning || Boolean(current?.[item.id]?.inTrash)}
              onClick={() => setSelected({ scan: inventory.scan, item, checkOnly: !item.removal })}>
              <ScanSearch size={16} />{item.removal && t("Prüfen & leeren")}
            </button>
          </div></td>
        </tr>)}
          {rows.length === 0 && <tr><td colSpan={4} className="empty-row">{t("In den üblichen Ordnern wurden keine Paket- oder Build-Caches gefunden.")}</td></tr>}
        </tbody>
      </table></div>
      {rows.length > 0 && <footer className="inventory-more">{number(rows.length)} {t("erkannte Cache-Ordner")}</footer>}
    </section>}
    {selected && <DevImpactDialog key={`${selected.scan}/${selected.item.id}`}
      scan={selected.scan} id={selected.item.id} kind="cache"
      title={t("{0}-Cache prüfen", selected.item.name)}
      detail={selected.item.name === "Maven" ? t("Das lokale Maven-Repository kann Artefakte enthalten, die sich nur aus deinem Projekt neu bauen lassen.")
        : selected.item.removal === "pnpm" ? t("pnpm räumt seinen Store nach den Regeln der installierten Version auf. Spätere Installationen können einige Pakete erneut laden.")
          : selected.checkOnly ? `${selected.item.path} · ${bytes(selected.item.bytes)}`
            : <>{bytes(selected.item.bytes)} {t("im Scan. Schließe laufende Builds vorher. Der Ordner lässt sich aus dem Papierkorb zurückholen.")}</>}
      action={selected.checkOnly ? undefined : selected.item.removal === "pnpm" ? t("Mit pnpm bereinigen") : t("In den Papierkorb")}
      busy={busy} onClose={() => setSelected(undefined)} onConfirm={() => void clean()} />}
  </>;
}
