import { t } from "./language";
import { LanguageControl } from "./language-control";
import { listen } from "@tauri-apps/api/event";
import { Download, FolderOpen, Languages, RotateCw, Shield, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { bytes, call, describeError, native } from "./api";

type Update = { version: string; notes: string | null };
type Progress = { downloaded: number; total: number | null; installing: boolean };
type Folder = { name: string; status: "readable" | "denied" | "missing" | "unknown" };
type ReleaseInfo = { version: string; updatesConfigured: boolean };
const accessLabels: Record<Folder["status"], string> = {
  readable: "Lesbar", denied: "Zugriff fehlt", missing: "Ordner fehlt", unknown: "Nicht prüfbar",
};

export function Settings({ close }: { close: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [release, setRelease] = useState<ReleaseInfo>();
  const [available, setAvailable] = useState<Update>();
  const [progress, setProgress] = useState<Progress>();
  const [phase, setPhase] = useState<"idle" | "checking" | "current" | "installing" | "installed">("idle");
  const [updateError, setUpdateError] = useState("");
  const [folders, setFolders] = useState<Folder[]>([]);
  const [checkingFolders, setCheckingFolders] = useState(false);
  const [accessError, setAccessError] = useState("");
  const working = phase === "checking" || phase === "installing";
  const percent = progress?.total ? Math.min(100, Math.round(progress.downloaded / progress.total * 100)) : null;

  useEffect(() => {
    dialog.current?.showModal();
    if (!native) return;
    void call<ReleaseInfo>("release_info").then(setRelease).catch(error => setUpdateError(describeError(error)));
    const listener = listen<Progress>("update-progress", ({ payload }) => setProgress(payload));
    return () => { void listener.then(unlisten => unlisten()).catch(() => {}); };
  }, []);

  async function checkUpdate() {
    setUpdateError("");
    setAvailable(undefined);
    setPhase("checking");
    try {
      const next = await call<Update | null>("check_for_update");
      setAvailable(next ?? undefined);
      setPhase(next ? "idle" : "current");
    } catch (error) {
      setUpdateError(describeError(error));
      setPhase("idle");
    }
  }

  async function install() {
    if (!available) return;
    setUpdateError("");
    setProgress(undefined);
    setPhase("installing");
    try {
      await call("install_update", { version: available.version });
      setPhase("installed");
    } catch (error) {
      setUpdateError(describeError(error));
      setPhase("idle");
    }
  }

  async function checkAccess() {
    setAccessError("");
    setCheckingFolders(true);
    try {
      setFolders(await call<Folder[]>("check_folder_access"));
    } catch (error) {
      setAccessError(describeError(error));
    } finally {
      setCheckingFolders(false);
    }
  }

  return <dialog className="dialog settings-dialog" ref={dialog} aria-labelledby="settings-title"
    onCancel={event => { event.preventDefault(); if (!working && phase !== "installed") close(); }}>
    <header className="settings-heading">
      <h2 id="settings-title">{t("Einstellungen")}</h2>
      <button className="icon-button" type="button" aria-label={t("Einstellungen schließen")} autoFocus
        disabled={working || phase === "installed"} onClick={close}><X size={18} aria-hidden="true" /></button>
    </header>
    <section className="settings-section" aria-labelledby="language-title">
      <span className="settings-section-icon"><Languages size={18} aria-hidden="true" /></span>
      <div>
        <h3 id="language-title">{t("Sprache")}</h3>
        <LanguageControl />
      </div>
    </section>
    <section className="settings-section" aria-labelledby="update-title">
      <span className="settings-section-icon"><Download size={18} aria-hidden="true" /></span>
      <div>
        <h3 id="update-title">{t("Softwareupdate")}</h3>
        <p>{release ? `Ilum ${release.version}` : native ? t("Version wird geladen …") : t("In der macOS-App verfügbar")}</p>
        {release && !release.updatesConfigured && <p>{t("Dieser lokale Build hat keinen Update-Kanal.")}</p>}
        {phase === "current" && <p role="status">{t("Ilum ist auf dem neuesten Stand.")}</p>}
        {available && phase !== "installed" && <p role="status">{t("Version {0} ist verfügbar.", available.version)}</p>}
        {available?.notes && phase === "idle" && <p className="update-notes">{available.notes}</p>}
        {phase === "installing" && <div className="update-progress" role="status">
          <span>{progress?.installing ? t("Signatur wird geprüft und Update installiert …")
            : progress ? t("{0}{1} geladen", bytes(progress.downloaded), progress.total ? ` / ${bytes(progress.total)}` : "") : t("Download wird vorbereitet …")}</span>
          <progress max={100} value={progress?.installing ? undefined : percent ?? undefined} aria-label={t("Update-Fortschritt")} />
        </div>}
        {phase === "installed" && <p role="status">{t("Version {0} ist installiert und nach dem Neustart aktiv.", available?.version ?? "—")}</p>}
        {updateError && <p className="settings-error" role="status">{updateError}</p>}
        <div className="settings-actions">
          {phase === "installed"
            ? <button className="button primary compact" type="button" onClick={() => void call("restart_after_update").catch(error => setUpdateError(describeError(error)))}>{t("Ilum neu starten")}</button>
            : available
              ? <button className="button primary compact" type="button" disabled={working} onClick={() => void install()}>{t("Update installieren")}</button>
              : <button className="button secondary compact" type="button" disabled={working || !release?.updatesConfigured} onClick={() => void checkUpdate()}>
                <RotateCw size={14} className={phase === "checking" ? "spin" : undefined} aria-hidden="true" />
                {phase === "checking" ? t("Wird geprüft …") : t("Nach Updates suchen")}
              </button>}
        </div>
      </div>
    </section>
    <section className="settings-section" aria-labelledby="access-title">
      <span className="settings-section-icon"><Shield size={18} aria-hidden="true" /></span>
      <div>
        <h3 id="access-title">{t("Dateizugriff")}</h3>
        <p>{t("macOS kann beim ersten Zugriff auf geschützte Ordner nachfragen. Ilum überspringt Ordner, für die der Zugriff fehlt.")}</p>
        {folders.length > 0 && <dl className="folder-access">{folders.map(folder => <div key={folder.name}>
          <dt>{t(folder.name)}</dt><dd data-status={folder.status}>{t(accessLabels[folder.status])}</dd>
        </div>)}</dl>}
        {accessError && <p className="settings-error" role="status">{accessError}</p>}
        <p>{t("Der Check prüft Schreibtisch, Dokumente und Downloads. Scans nutzen auch andere lesbare Ordner.")}</p>
        <div className="settings-actions">
          <button className="button secondary compact" type="button" disabled={!native || checkingFolders} onClick={() => void checkAccess()}>
            <FolderOpen size={15} aria-hidden="true" /> {checkingFolders ? t("Wird geprüft …") : t("Ordnerzugriff prüfen")}
          </button>
          <button className="text-button" type="button" disabled={!native}
            onClick={() => void call("open_privacy_settings").catch(error => setAccessError(describeError(error)))}>{t("Systemeinstellungen öffnen")}</button>
        </div>
      </div>
    </section>
  </dialog>;
}
