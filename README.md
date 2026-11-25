# DataPilot

A desktop data-analysis agent for macOS and Windows (Tauri UI + Rust core), in the spirit of Codex / Claude Code but focused on data work.

- Ask questions about Excel / CSV / Parquet / Word / PDF / PPT in natural language; DuckDB Text-to-SQL, pandas, charts and interactive BI dashboards.
- Works with any OpenAI-compatible endpoint (New API, vLLM, Ollama…). New API gateways support **log in & pick a key** (password, SSO / OIDC …).
- Agent loop with tools, skills (docx / xlsx / pptx / pdf), parallel sub-agents, short/long-term memory, keyword + grep retrieval (no embeddings).
- Approval modes (Plan / Ask / Auto / Full access) with `Tool(specifier)` allow/ask/deny rules; a single **network switch** gates every outbound request; optional **local-only mode**.
- Web search via the local Chrome over the DevTools Protocol (Google, Bing fallback) — no search API key. Chrome for Testing is downloaded on demand if no Chrome is installed.
- Code runs in a sandbox (macOS Seatbelt; Windows AppContainer + Job Object) with a persistent per-session Python kernel.

## Build
Prebuilt installers (macOS DMG, Windows EXE, unsigned) may be published on the GitHub Releases page; you can always build from source.
Requirements: macOS 14+ (Apple Silicon) with Xcode command-line tools, Rust (stable), Node 20+. The Windows installer is cross-built on the Mac (cargo-xwin + NSIS).

```bash
bash scripts/fetch_vendor.sh && bash scripts/build_venv.sh   # bundled Python 3.12 + packages
bash apps/desktop/scripts/build_mac.sh   # → apps/desktop/src-tauri/target/release/bundle/macos/DataPilot.app
bash apps/desktop/scripts/build_win.sh   # Windows installer, cross-built on the Mac (cargo-xwin + NSIS)
cd datapilot-core && cargo test --release                    # core unit + golden tests
cd apps/desktop && npx vitest run && node e2e/run.mjs        # UI unit + end-to-end tests
```

API keys are stored only in the system credential store (macOS Keychain / Windows Credential Manager, service `io.github.datapilot.apikeys`), never in config files.

## Docs
Design docs (Chinese) are in [`docs/`](docs/00-overview.md); the user guide is [`docs/user-guide.md`](docs/user-guide.md); architecture decisions are in [`docs/adr/`](docs/adr/).

## Cross-platform
macOS and Windows share one Tauri UI (`apps/desktop`) over the Rust core (`datapilot-core`); the Windows build is in testing ([checklist](docs/21-windows-test-checklist.md)). See [ADR-0012](docs/adr/0012-cross-platform-architecture.md) and the [dev guide](docs/18-cross-platform-dev-guide.md). Shared contracts live in [`protocol/`](protocol/README.md); behaviour is pinned by [golden samples](tests/golden/README.md).

## 致谢与许可 / Acknowledgements & license
DataPilot is licensed under the [Apache License 2.0](LICENSE). Third-party components are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) and in the app's **About DataPilot** window.

Contact: wanghualoong@126.com
