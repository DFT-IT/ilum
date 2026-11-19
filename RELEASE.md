# Ilum veröffentlichen

Ilum wird als `Ilum.dmg` für Apple Silicon und Intel über **GitHub Releases** angeboten. [dft-it.com](https://dft-it.com/) kann auf den [aktuellen Download](https://github.com/DFT-IT/ilum/releases/latest/download/Ilum.dmg) verlinken. Die App ist ad hoc signiert. macOS kann sie beim ersten Start blockieren, weil sie keine Apple-Notarisierung hat. In-App-Updates werden separat signiert. [Apple zum Öffnen heruntergeladener Apps](https://support.apple.com/de-de/102445)

## Einmal einrichten

1. Öffentliches GitHub-Repository einrichten und die [MIT-Lizenz für den Quellcode](LICENSE) mitführen. Rechte an Name und Branding klären.
2. GitHub-Umgebung `release` anlegen. Mit `npm run tauri signer generate` das Schlüsselpaar für Updates erzeugen und dauerhaft sichern. Bei späteren Builds dasselbe Schlüsselpaar verwenden.

| GitHub | Werte |
| --- | --- |
| Variables | `TAURI_UPDATE_PUBLIC_KEY` (Inhalt der `.pub`-Datei) |
| Secrets für Updates | `TAURI_SIGNING_PRIVATE_KEY` (Inhalt der privaten Schlüsseldatei), `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (falls gesetzt) |

## Pro Version

1. Version in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` und `package.json` erhöhen, Lockfiles aktualisieren und den passenden Tag `vX.Y.Z` pushen.
2. Den vom [Workflow](.github/workflows/release.yml) erzeugten Release-Entwurf prüfen. Unter **What's new** kurz beschreiben, was sich in dieser Version geändert hat. Danach als stabiles GitHub-Release veröffentlichen. Der Entwurf enthält DMG, Prüfsumme und signierte Update-Dateien.
3. Mit der vorherigen Version aus demselben Update-Kanal **Einstellungen → Softwareupdate** testen. Lokale Builds ohne Update-Kanal brauchen einmalig die öffentliche DMG.
