import { t } from "./language";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { Activity, AlertTriangle, Check, FolderGit2, Link2, LoaderCircle, Package, ScanSearch } from "lucide-react";
import { call, describeError, number } from "./api";
import { ActionProgress } from "./confirmation";

type ReferenceKind = "running" | "link" | "installedPackage" | "project";
type Impact = {
  references: { kind: ReferenceKind; name: string; path: string }[];
  checkedProjects: number;
  skipped: number;
  truncated: boolean;
  blocking: boolean;
};

const indicators = {
  running: { icon: Activity, label: "Laufender Prozess" },
  link: { icon: Link2, label: "Direkter Link" },
  installedPackage: { icon: Package, label: "Installiertes Paket" },
  project: { icon: FolderGit2, label: "Projekt-Hinweis" },
};

export function DevImpactDialog({ scan, kind, id, title, detail, action, busy, onClose, onConfirm }: {
  scan: number; kind: "cache" | "tool" | "project"; id: number; title: string; detail: ReactNode;
  action?: string; busy: boolean; onClose: () => void; onConfirm?: () => void;
}) {
  const [impact, setImpact] = useState<Impact>();
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const dialog = useRef<HTMLDialogElement>(null);
  const titleId = useId();

  useEffect(() => { dialog.current?.showModal(); }, []);
  useEffect(() => {
    let active = true;
    void call<Impact>("check_dev_impact", { scan, kind, id }).then(result => {
      if (active) setImpact(result);
    }).catch(error => {
      if (active) setError(describeError(error));
    });
    return () => { active = false; };
  }, [scan, kind, id, attempt]);

  return <dialog ref={dialog} className="dialog impact-dialog" aria-labelledby={titleId}
    onCancel={event => { event.preventDefault(); if (!busy) onClose(); }}>
    <header className="impact-dialog-heading">
      <div className="dialog-icon"><ScanSearch size={20} /></div>
      <div><h2 id={titleId}>{title}</h2><p>{detail}</p></div>
    </header>
    <section className="impact-results" aria-label={t("Abhängigkeits-Check")}>
      <h3>{t("Abhängigkeits-Check")}</h3>
      {!impact && !error && <div className="impact-status" role="status"><LoaderCircle className="spin" size={16} /> {t("Lokale Projekte und Prozesse werden geprüft …")}</div>}
      {error && <div className="impact-status problem" role="status"><AlertTriangle size={16} />{error}</div>}
      {impact && <>
        {impact.blocking ? <div className="impact-status problem" role="status"><AlertTriangle size={16} /> {t("Ein Projektlink oder laufender Prozess greift direkt auf das Ziel zu.")}</div>
          : impact.references.length ? <div className="impact-status"><Link2 size={16} /> {t("Hinweise gefunden. Prüfe sie vor dem Entfernen.")}</div>
            : <div className="impact-status clear"><Check size={16} /> {t("In den geprüften Quellen wurde kein Verweis gefunden.")}</div>}
        {impact.references.length > 0 && <ul className="impact-list">
          {impact.references.map(reference => {
            const { icon: Icon, label } = indicators[reference.kind];
            return <li key={`${reference.kind}/${reference.path}/${reference.name}`}>
              <Icon size={15} aria-hidden="true" /><div><strong>{reference.kind === "running" ? reference.name : t(reference.name)}</strong><span title={reference.path}>{reference.path}</span></div>
              <small>{t(label)}</small>
            </li>;
          })}
        </ul>}
        <p className="impact-scope">{number(impact.checkedProjects)} {t("Projektordner mit Manifesten geprüft.")}{impact.skipped === 1 ? t(" 1 Pfad wurde übersprungen.")
            : impact.skipped > 1 ? t(" {0} Pfade wurden übersprungen.", number(impact.skipped)) : ""}
          {impact.truncated && t(" Die Prüfung oder Trefferliste ist begrenzt.")}
        </p>
      </>}
    </section>
    {busy && <ActionProgress />}
    <div className="dialog-actions">
      <button className="button secondary" onClick={onClose} autoFocus disabled={busy}>{action ? t("Abbrechen") : t("Schließen")}</button>
      {(error || impact?.blocking) && <button className="button secondary" disabled={busy} onClick={() => {
        setError(""); setImpact(undefined); setAttempt(value => value + 1);
      }}>{t("Erneut prüfen")}</button>}
      {action && <button className="button primary" onClick={onConfirm} disabled={busy || !impact || impact.blocking}>
        {busy && <LoaderCircle className="spin" size={16} />}{busy ? t("Wird verarbeitet …") : action}
      </button>}
    </div>
  </dialog>;
}
