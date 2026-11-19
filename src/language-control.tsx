import { setLanguage, t, useLanguage } from "./language";

const options = [
  { value: "de", label: "Deutsch" },
  { value: "en", label: "English" },
] as const;

export function LanguageControl({ compact = false }: { compact?: boolean }) {
  const language = useLanguage();

  return <div className={compact ? "toolbar-language" : "section-tabs settings-language"}
    role="group" aria-label={t("Sprache")} data-tauri-drag-region="false">
    {options.map(({ value, label }) => <button type="button" key={value} lang={value}
      aria-label={label} aria-pressed={language === value} title={compact ? label : undefined}
      onClick={() => setLanguage(value)}>
      {compact ? value.toUpperCase() : label}
    </button>)}
  </div>;
}
