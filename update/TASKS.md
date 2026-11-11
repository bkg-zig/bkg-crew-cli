# bkgcrewcli Update Tasks

Stand: 2026-11-11 (initial)  
Referenz: `update/PLAN.md`, `TSK_INTEGRATION_DESIGN.md` (nur Konzeptquelle), Inspirationsfeatures ohne Fremdnamen (remote Sandbox/Capsule, Vault, Cognition/Mender). **Keine alten/fremden Namen in Code/CLI/Config.**

## Namens-Mapping (bkgcrewcli)
- Sandbox-Provider: `local` | `container` (docker/podman) | `capsule` (remote MicroVM). Module `sandbox::{container,capsule,...}`.
- Wissensschicht: `vault` (`src/vault/`).
- Adaptive Signale/Reparatur: `cognition` + `mender` (`src/cognition/`, `src/mender/`).
- Tasks/Queue: bestehend `task_queue`/`task` CLI beibehalten.

## Legende
- [ ] offen  [~] in Arbeit  [x] erledigt  [!] blockiert

## Phase A – Grundlagen & Gerüst
- [x] A1. `update/PLAN.md` erstellen (Architektur, Mapping, Phasen, Risiken, Namenskonvention).
- [x] A2. `update/TASKS.md` erstellen (dieses Dokument).
- [x] A3. Ist-Stand prüfen: sandbox/task/crew/config/CLI-Struktur, vorhandene Stubs & Dependencies (Cargo.toml).
- [x] A4. Config-Modelle erweitert (bkgcrewcli-Namen): Sandbox (mode/provider/container-runtime/capsule-endpoint/template/env/volumes/timeout), TaskQueue, Vault, Cognition/Mender, Sidecar/Git (serde/defaults, rückwärtskompatibel). (Status 2026-11-11: erweitert via `src/settings.rs` — Sandbox+Memory+TaskQueue; Admin-UI + `/admin/app-config` in Betrieb.)

## Phase B – Sandbox & Capsule (remote Backend)
- [ ] B1. `sandbox::container` (docker/podman): reales Container-Run-Wrapper (args/env/mounts/network/resources/devices, pull-policy, cwd, exit/log streaming) implementieren.
- [ ] B2. `sandbox::capsule`: HTTP-Client (health, create, connect/resume, info, running, timeout, pause, kill; optional exec/list/templates) + Request/Response-Typen.
- [ ] B3. `SandboxRunner`: Mode-Dispatch (`local|container|capsule`), Env-Injection, Fehlerabbildung auf `Error/Event/StreamHandle`.
- [ ] B4. Template/Stack-Mapping: `stack/project → image|template` Defaults + Config-Override.
- [ ] B5. CLI `run`: Flags `--sandbox`, `--sandbox-mode`/`--sandbox-provider`, `--stack`, `--project`, Netzwerk/Devices, `--capsule-endpoint/--capsule-template` (opt-in, bkgcrewcli-Namen).
- [ ] B6. Smoke-/Unit-Tests für Arg-Building, Config-Parsing, Client-Serialisierung (ohne externe Services).

## Phase C – Tasks, Queue, Chaining, Git (TSK)
- [ ] C1. Task-Datenmodell & SQLite-Storage vervollständigen (Status, Zeiten, agent/cwd/prompt-ref, sandbox, branch, parent_ids, logs/metrics/error, retries).
- [ ] C2. Task-Lifecycle-API: create/run/list/show/cancel/retry/wait, Status-Transitionen, Timeout/Retry-Policy.
- [ ] C3. Dependency/Chaining: `parent_ids` auswerten, Scheduling-Guard (blocked/ready), einfache DAG-Regeln.
- [ ] C4. Git-Helpers (optional, konfigurierbar/off): Branch-Naming, auto-commit Message-Template, Workdir/Copy-Hinweise (keine destruktiven Defaults).
- [ ] C5. CLI `task` Subcommands: `create|run|list|show|cancel|retry|wait|logs` + `run --task/--task-name/--parent`.
- [ ] C6. Events/Logs: Task-Events in Event-Stream/Logger integrieren.

## Phase D – Vault & Adaptive Cognition/Mender (Schwerpunkt)
- [ ] D1. Neues Modul `src/vault/`: SQLite-Store, Schema (scopes/spaces, episodes, facts, prefs, beliefs, artifacts/refs, tags, importance, timestamps, author/source/privacy).
- [ ] D2. Vault-API: remember/recall/list/forget, Scope/Space-Scoping, strukturierte Facts & Episoden, Importance/Pin, Attribution.
- [ ] D3. Retrieval & Kontext-Injektion: hybride/mehrsignalige Heuristik (Keyword/Text-Match + Recency/Salience/Importance + Typ-Filter), Token-budgetierte Prompt-Snippets.
- [ ] D4. Lifecycle/Maintenance: decay/aging, dedup/consolidation light, Maintenance-Run (`vault maintain`), opt-in/konfigurierbar.
- [ ] D5. Neues Modul `src/cognition/`: Execution-Signal-Erfassung (run/task/tool/validation), kompakte Metriken.
- [ ] D6. Neues Modul `src/mender/`: Failure-Klassifikation (build/test/runtime/provider/auth/network/tooling/context), Normalisierung, Repair-Motive, Recurrence, Confidence, Replay-Gates, Fix-Strategien.
- [ ] D7. Prompt-Memory & Token-Sparen: kompakte Recall/Repair-Hinweise injizieren; große Outputs als Artefakt-Referenzen (Rehydrierung on-demand).
- [ ] D8. Optionaler Sidecar-Hook: Config-Schnittstelle für lokalen Helper (Summary/Kompression/Routing/Critique) mit graceful Fallback.
- [ ] D9. CLI: `vault`/`mender` (oder integrierte) Subcommands `remember|recall|facts|maintain|repairs|signals` (minimal sinnvoller Satz).

## Phase E – Server/TUI, Doku, Stabilisierung
- [ ] E1. Scheduler/Server: Dependency-aware Queue-Verarbeitung, Status/Enqueue-Endpunkte erweitern (inkrementell, kompatibel).
- [ ] E2. TUI: Task/Run/Repair-Status-Sichten (falls schnell integrierbar).
- [ ] E3. Konfiguration & Beispiele: `bkgcrewcli.toml`/Settings Beispiele (sandbox, task, memory, cognition, cubesandbox).
- [ ] E4. Doku: `update/IMPLEMENTATION_NOTES.md` + README/Docs-Abschnitt zu neuen Features/Flags.
- [ ] E5. Build & Checks: `cargo fmt` (falls konfiguriert), `cargo check`, `cargo test` (relevante), Fixes.
- [ ] E6. Rückwärtskompatibilität & Sicherheit prüfen (Defaults opt-in, keine Secret-Leaks, robuste Fehlerpfade).

## Start (nächste konkrete Schritte)
1. [x] A1–A3 Plan/Tasks + Ist-Analyse + Namenskonvention (keine Alt-/Fremdnamen).
2. [~] A4/B5/C5/D9 CLI- & Config-Gerüst anlegen (bkgcrewcli-Namen: `container`/`capsule`, `vault`, `cognition`, `mender`).
3. [ ] B1–B3 Sandbox/Container/Capsule-Kernpfade implementieren.
4. [ ] D1–D7 Vault/Cognition/Mender-Kern implementieren (Fokus Repair-Learning & Token-Sparen).
5. [~] 2026-11-11 (Sitzung): Admin-Endpunkte live (`/admin/app-config`, `/admin/doctor`, `/admin/config`, `/admin/config/model-mappings`, `/api/tokens*`), Provider-Scoped-Router (`/:provider/v1/*`), Static-WebUI (Fallback- statt Nest), zentrale Settings-Pfad-Auflösung (`/srv/bkg/data/bkgcrewcli`), Version 0.1.8, Release-Deploy. Nächste: B1–B3, D1–D7, IDE-Opencode-Stabilisierung.
5. [ ] Build/Check und iterativ vervollständigen.
