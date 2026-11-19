# Ilum veröffentlichen

Ilum wird als `Ilum.dmg` für Apple Silicon und Intel über **GitHub Releases** angeboten. [dft-it.com](https://dft-it.com/) kann auf `https://github.com/OWNER/REPO/releases/latest/download/Ilum.dmg` verlinken. Eine Einreichung im Mac App Store ist dafür nicht nötig.

## Einmal einrichten

1. Öffentliches GitHub-Repository einrichten und die [MIT-Lizenz für den Quellcode](LICENSE) mitführen. Rechte an Name und Branding klären.
2. GitHub-Umgebung `release` anlegen. Für Signierung und Notarisierung ein **Developer ID Application**-Zertifikat und einen Apple-API-Schlüssel einrichten. Der Schlüssel aus App Store Connect dient der Notarisierung. [Anleitung zur macOS-Signierung](https://v2.tauri.app/distribute/sign/macos/)
3. Mit `npm run tauri signer generate` ein separates Schlüsselpaar für Updates erzeugen. Private Schlüssel gehören in GitHub Secrets.

| GitHub | Werte |
| --- | --- |
| Variables | `APPLE_SIGNING_IDENTITY` (Zertifikatsname), `TAURI_UPDATE_PUBLIC_KEY` (Inhalt der `.pub`-Datei) |
| Secrets für Apple | `APPLE_CERTIFICATE` (Base64-kodierte `.p12`), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_API_KEY` (Key-ID), `APPLE_API_ISSUER` (Issuer-ID), `APPLE_API_KEY_CONTENT` (Inhalt der `.p8`) |
| Secrets für Updates | `TAURI_SIGNING_PRIVATE_KEY` (Inhalt der privaten Schlüsseldatei), `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (falls gesetzt) |

## Pro Version

1. Version in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` und `package.json` erhöhen, Lockfiles aktualisieren und den passenden Tag `vX.Y.Z` pushen.
2. Den vom [Workflow](.github/workflows/release.yml) erzeugten Release-Entwurf prüfen und als stabiles GitHub-Release veröffentlichen. Er enthält DMG, Prüfsumme und signierte Update-Dateien.
3. Mit der vorherigen Version aus demselben Update-Kanal **Einstellungen → Softwareupdate** testen. Lokale Builds ohne Update-Kanal brauchen einmalig die öffentliche DMG.
