import { t } from "./language";
import { Activity, ArrowRight, HardDrive, RotateCcw, TerminalSquare } from "lucide-react";
import { bytes, uptime, type Snapshot } from "./api";

export function freeDisk(sample: Snapshot | undefined) {
  return sample?.diskTotal != null && sample.diskUsed != null ? sample.diskTotal - sample.diskUsed : null;
}

export function lowDisk(sample: Snapshot | undefined) {
  const free = freeDisk(sample);
  return free != null && (free < 10e9 || (free < 30e9 && free / sample!.diskTotal! < .1));
}

export function elevatedPressure(sample: Snapshot | undefined) {
  return sample?.memoryPressure === "warning" || sample?.memoryPressure === "critical";
}

export function sustainedCpu(history: number[]) {
  const recent = history.slice(-8);
  return recent.length === 8 && recent.every(value => value >= 80);
}

export function Recommendations({ sample, history, checkStorage, checkProcesses, checkDev }: {
  sample?: Snapshot;
  history: number[];
  checkStorage: () => void;
  checkProcesses: (sort: "memory" | "cpu") => void;
  checkDev: () => void;
}) {
  const memory = elevatedPressure(sample);
  const cpu = sustainedCpu(history);
  const disk = lowDisk(sample);
  return <section className="panel next-steps" aria-labelledby="next-steps-title">
    <div className="panel-heading"><div><h2 id="next-steps-title">{t("Empfehlungen")}</h2></div></div>
    {disk && <div className="recommendation-row">
      <span className="panel-icon accent-icon"><HardDrive size={19} /></span>
      <div><h3>{t("Nur {0} frei", bytes(freeDisk(sample)))}</h3><p>{t("Wenig freier Platz kann Updates und die Arbeit mit großen Dateien erschweren.")}</p></div>
      <button className="text-button" onClick={checkStorage}>{t("Festplatte ansehen")}<ArrowRight size={15} /></button>
    </div>}
    {(memory || cpu) && <div className="recommendation-row">
      <span className="panel-icon"><Activity size={19} /></span>
      <div><h3>{memory ? t("macOS meldet Speicherdruck") : t("CPU bleibt stark ausgelastet")}</h3>
        <p>{memory ? t("Prüfe, welche Apps den meisten RAM belegen. Sichere deine Arbeit, bevor du eine App beendest.")
          : t("Die CPU lag bei acht aufeinanderfolgenden Messungen bei mindestens 80 %. Prüfe, welcher Prozess die Last verursacht.")}</p></div>
      <button className="text-button" onClick={() => checkProcesses(memory ? "memory" : "cpu")}>{t("Prozesse prüfen")}<ArrowRight size={15} /></button>
    </div>}
    <div className="recommendation-row">
      <span className="panel-icon"><TerminalSquare size={18} /></span>
      <div><h3>{t("Speicher in Dev-Projekten prüfen")}</h3><p>{t("Oft lassen sich Abhängigkeiten und Build-Ordner aus dem Projekt wiederherstellen.")}</p></div>
      <button className="text-button" onClick={checkDev}>{t("Dev-Modus")}<ArrowRight size={15} /></button>
    </div>
    {sample && <details className="restart-help">
      <summary><RotateCcw size={15} />{memory || cpu ? t("Wann hilft ein Neustart?") : t("Neustart nur bei anhaltenden Problemen")}</summary>
      <p>{t("Seit {0} aktiv.", uptime(sample.uptime))} {t("Die Laufzeit allein ist kein Grund für einen Neustart. Wenn der Mac trotz geschlossener Problem-Apps langsam bleibt oder sich Systemfunktionen aufhängen, kann ein Neustart festsitzende Prozesse beenden. Sichere offene Arbeit und wähle im Apple-Menü ")}<strong>{t("Neustart …")}</strong>{t(". Für mehr freien Festplattenspeicher prüfe die Bereinigung.")}</p>
    </details>}
  </section>;
}
