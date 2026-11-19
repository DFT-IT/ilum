import { t } from "./language";
import { useEffect, useRef, useState } from "react";
import { FileImage, FileText, Film } from "lucide-react";
import { call, native } from "./api";

type PreviewJob = {
  key: string; mode: string; id: number; cancelled: boolean;
  display: (image: string | null) => void;
};
const jobs: PreviewJob[] = [];
const cache = new Map<string, string | null>();
let active = 0;
const extensions = /\.(jpe?g|png|gif|webp|heic|heif|avif|tiff?|pdf|mov|mp4|m4v|docx|pptx|key|pages|numbers)$/i;
const pictures = /\.(jpe?g|png|gif|webp|heic|heif|avif|tiff?)$/i;
const movies = /\.(mov|mp4|m4v)$/i;

function runQueue() {
  while (active < 3 && jobs.length > 0) {
    const job = jobs.shift()!;
    if (job.cancelled) continue;
    active++;
    void call<string | null>("file_thumbnail", { mode: job.mode, id: job.id }).then(preview => {
      const image = preview ? `data:image/png;base64,${preview}` : null;
      cache.set(job.key, image);
      if (cache.size > 100) cache.delete(cache.keys().next().value!);
      if (!job.cancelled) job.display(image);
    }).catch(() => {}).finally(() => {
      active--;
      runQueue();
    });
  }
}

export function FileThumbnail({ id, mode, name, open }: { id: number; mode: string; name: string; open: () => void }) {
  const canPreview = native && (mode === "files" || mode === "storage") && extensions.test(name);
  const key = `${mode}/${id}`;
  const [preview, setPreview] = useState<string | null>(() => cache.get(key) ?? null);
  const marker = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!canPreview || !marker.current) return;
    if (cache.has(key)) {
      setPreview(cache.get(key) ?? null);
      return;
    }
    const job = { key, mode, id, cancelled: false, display: setPreview };
    const observer = new IntersectionObserver(entries => {
      if (entries.some(entry => entry.isIntersecting)) {
        observer.disconnect();
        jobs.push(job);
        runQueue();
      }
    }, { rootMargin: "36px" });
    observer.observe(marker.current);
    return () => {
      job.cancelled = true;
      observer.disconnect();
    };
  }, [key, mode, id, canPreview]);

  const Icon = pictures.test(name) ? FileImage : movies.test(name) ? Film : FileText;
  return <button ref={marker} type="button" className={`file-thumbnail ${canPreview ? "previewable" : ""}`}
    title={t("Vorschau für {0}", name)} aria-label={t("Vorschau für {0}", name)} onClick={open} disabled={!native}>
    {preview ? <img src={preview} alt="" draggable={false} /> : <Icon size={canPreview ? 19 : 15} aria-hidden="true" />}
  </button>;
}
