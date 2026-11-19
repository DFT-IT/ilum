# Development

## Architecture

- **React and TypeScript** render the dashboard and menu bar popover. Screens own their view state and use `src/api.ts` for native commands. `src/language.ts` synchronizes language and locale across windows. `src/messages.en.json` maps German source messages to English. Numbered placeholders preserve filenames, paths, and other inserted values.
- **Rust and Tauri** provide telemetry, inventories, file operations, permissions, and the updater. Filesystem scans run away from the UI thread. The app uses native Quick Look, Finder, and the Trash where appropriate.
- **Scan results** hold opaque IDs for permitted actions. Before moving an item, the backend resolves its ID against the current scan and rechecks its path. Developer removal uses the package manager or the Trash where appropriate.
- **Updates** are checked on request. Release builds embed a public verification key and an HTTPS GitHub Releases endpoint. The user chooses when to install and restart.

## Scan coverage

| View | Locations |
| --- | --- |
| Disk and file finder | Standard personal folders, including Desktop, Documents, Downloads, Movies, Music, and Pictures |
| Cleanup | Selected caches, logs, temporary files, and potential app remnants. Personal app data is review-only. |
| App and cache inventory | Common app install locations, Spotlight-indexed apps, and readable standard, app-container, and shared-container caches |
| Developer tools | Common project and package-manager folders, a manually selected project folder, and local Docker Desktop data when available |
| Storage history | Selected personal folders, app caches, and developer data, plus free space on the startup disk |

The File Access check tests only Desktop, Documents, and Downloads. macOS can restrict other locations even when those checks pass. Ilum skips inaccessible paths and can report incomplete coverage. Granting Full Disk Access can make more paths readable. It does not expand Ilum's scan locations or override its cleanup rules.

The storage history uses allocated file blocks and runs about every six hours while Ilum is running. Shared APFS blocks and hard links can make measured sizes differ from space recovered. CLI history is optional, stays local, and is off during normal scans.

## Code conventions

Use descriptive English identifiers and language-appropriate casing. Apply the [Google TypeScript guide](https://google.github.io/styleguide/tsguide.html) for naming and JSDoc, and the [Google Rust guide](https://google.github.io/styleguide/rustguide.html) for Rustdoc and implementation comments. Document contracts, side effects, non-obvious invariants, and safety decisions. Use a clear name when it can replace a comment.

```sh
npm run build
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo test --locked --manifest-path src-tauri/Cargo.toml
```
