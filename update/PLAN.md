# bkgcrewcli Update Plan – Sandbox/Task/Memory/Cognition Integration

Datum: 2026-11-11  
Scope: Nahtlose, **neu geschriebene** Integration der Konzepte aus `TSK_INTEGRATION_DESIGN.md` sowie den referenzierten Projekten (Remote-MicroVM-Sandbox, lokale Memory-Engines, Repair-Learning) in das bestehende `bkgcrewcli`-System. **Kein Code-Kopieren**; nur Ideen/Features eigenständig umgesetzt und an die Architektur angepasst.

## 0. Namenskonvention (verbindlich)
- **Keine Fremd-/Alt-Namen** aus anderen Projekten/Designs in unserer API, Modulen, Typen, CLI-Flags oder Config-Keys übernehmen. Alles passend zu **bkgcrewcli** benennen.
- **Sandbox/Isolation:** Modi/Provider z. B. `local` (direkt), `container` (docker/podman), `capsule` (remote MicroVM/Sandbox-Backend). Module `sandbox::{runner,container,capsule,template,policy}`.
- **Tasks/Queue:** bestehende Begriffe `task`/`queue` weiterführen (bkgcrewcli-nativ), keine „tsk“-Präfixe/Bezeichner.
- **Memory/Wissen:** Modul `vault` (persistenter Speicher, Recall, Kontext) + ggf. `memory`-API-Begriffe im CLI (`vault`/`memory` konsistent wählen).
- **Cognition/Repair (Neura-inspiriert):** Module `cognition` (Signale, Kontext-Aufbereitung) und `mender`/`repair` (Failure-Klassen, Repair-Motive, Confidence, Replay-Gates) – bkgcrewcli-eigene Namen.
- **Referenzen im Code:** keine Marken-/Repo-Namen als Identifier; Herkunft nur ggf. in `update/`-Dokumentation als Inspirationsquelle.

## 1. Ziele

1. **Sandbox/Isolation & Tasks** funktionsfähig in `bkgcrewcli` verankern: Sandbox-Ausführung (`local`/`container`/`capsule`), persistente Tasks, Chaining/Status, Git-freundlicher Workflow, CLI/Config-Integration.
2. **Remote-Sandbox („Capsule“)-Backend** als Provider-Option integrieren: Lifecycle (create/connect/info/pause/resume/kill/timeout), Ausführung/Host-Routing, Template/Volume/Env/Metadaten – sauber als Client/Runner-Modul.
3. **Wissensschicht „Vault“**: lokaler, persistenter Speicher (SQLite), hybride/mehrsignalige Suche, strukturierte Facts/Episoden/Preferences/Beliefs, Decay/Consolidation/Maintenance, Scopes/Spaces (Namespace/Room-artig), Attribution/Privacy-Hinweise.
4. **Adaptive Cognition & Repair („Mender“, Neura-inspiriert, Schwerpunkt):** kompakte Ausführungssignale, Failure-Klassifikation, wiederkehrende Repair-Motive, Confidence/Replay-Gates, Token-sparende Prompt-Memory/Kompression (optional lokaler Sidecar), Context-Vault/Referenzen statt Voll-Transcript.
5. **Nahtlose Einpassung:** bestehende Traits/Module (`AgentRunner`, `TaskConfig`, `sandbox`, `task_queue`, `crew`, `settings`, `event`, CLI) erweitern statt ersetzen; optional per Flag/Config aktivierbar, rückwärtskompatibel.

## 2. Ist-Analyse (bkgcrewcli)

- **CLI/Run:** `src/main.rs` (run/list/check/config/models/provider/keystore/crew/server), `TaskConfig`, Agent-Runners (claude/opencode/codex/cursor/pi/copilot).
- **Sandbox (Stubs/Grundlage):** `src/sandbox/{mod,runner,docker,cubesandbox,shim,template}.rs`, `SandboxConfig` in `config.rs` (modes docker/podman/cubesandbox, network/resources/devices/auto_pull).
- **Tasks:** `src/task_queue/{mod,task,storage}.rs`, SQLite-Storage vorhanden (Grundstruktur).
- **Crew:** `src/crew/{mod,workflow,scheduler,config}.rs` (DAG/Orchestrierung).
- **Server/TUI:** `src/server/mod.rs`, TUI vorhanden/ausbaufähig.
- **Memory/Cognition:** derzeit keine dedizierte Memory-/Repair-/Cognition-Schicht (neue Module nötig).

## 3. Architektur-Entwurf (neu, eigenständig, bkgcrewcli-Namen)

### 3.1 Sandbox & Isolation (`src/sandbox/`)
- **Modi:** `local` (kein Container), `container` (docker/podman), `capsule` (remote MicroVM-Backend).
- **`sandbox::container`:** echtes `docker`/`podman run`-Wrapping (Image/Stack, Mount cwd/workdir, Env/API-Keys, Network-Mode, Resource-Limits, Devices, Pull-Policy), Stream/Exit-Status → `StreamHandle`/Events.
- **`sandbox::capsule` (Client/Runner):** HTTP-Client für Kern-Lifecycle: health, create/connect(resume), get/info, is_running, set_timeout, pause, kill, optional exec/templates/list. Konfigurierbar (endpoint, api-key, template, timeout, metadata, volumes, envs).
- **`SandboxRunner`** als `AgentRunner`-Wrapper: Mode-Dispatch, Env-Injection, Mapping Logs/Result/Errors.
- **`sandbox::template`/`policy`:** Abbildung `stack/project → image|template`, Security/Network/Resource-Defaults.

### 3.2 Task Queue, Persistence, Chaining, Git (`src/task_queue/`, CLI)
- **Persistenz:** SQLite-Schema (Status, Zeiten, Agent, cwd, prompt/ref, sandbox, branch, parent_ids, logs/metrics, error). CRUD + Filter/List/Show.
- **Lifecycle:** create/run/retry/cancel/wait, Status-Übergänge, Retry/Timeout.
- **Chaining:** `parent_ids`/Dependencies; Scheduler respektiert Abhängigkeiten.
- **Git-Workflow (optional):** Branch `bkg/task-{id|name}`, Copy/Worktree-Option, auto-commit/push Hooks (konfigurierbar, off by default).
- **CLI:** `bkgcrewcli task {create,run,list,show,cancel,retry,wait,logs}` + `run --sandbox/--task/--stack/...` Flags.

### 3.3 Wissensschicht `vault` (`src/vault/` – neu)
- **Store:** SQLite (lokal, z. B. `~/.local/share/bkgcrewcli/vault.db`), Scopes/Spaces, Autor/Quelle, Timestamps.
- **Datentypen:** Einträge/Episoden, Facts (Subjekt/Prädikat/Objekt), Preferences, Beliefs/Confidence, Repair-Motive (s. 3.4), Links/Tags, Artefakt-Referenzen.
- **Retrieval:** hybride Signale (Keyword/Text + strukturierte Filter + Recency/Salience/Importance) und kompakte Kontext-Injektion (Token-Budget). (Basis deterministisch/SQLite-FTS-orientiert, Vektor optionaler Ausbau.)
- **Lifecycle:** decay/aging, pin/importance, consolidation/dedup light, Maintenance-Zyklus als Kommando/Hook.
- **Privacy/Isolation:** Scope-Isolation, opt-in Sharing, keine Telemetrie.

### 3.4 Adaptive Cognition & Repair `cognition` + `mender` (`src/cognition/`, `src/mender/`, Neura-inspiriert)
- **`cognition` (Signale):** pro Run/Task/Tool/Validation kompakte Events (Erfolg/Fehler, Dauer, Exit/Typ, Kontext) erfassen und für Prompt abrufen.
- **Failure-Klassifikation (`mender`):** build/test/runtime/provider/auth/network/tooling/context; Normalisierung von Fehlertexten.
- **Repair-Motive:** wiederkehrende Muster (Fehlerklasse+Kontext → Fix/Strategie), Recurrence-Tracking, Confidence-Kalibrierung, Replay-Gates (Vorschlag/Auto-Ausführung).
- **Prompt-Memory & Token-Sparen:** kompakte Recall/Repair-Hinweise injizieren; große Outputs als Artefakt-Referenzen speichern und gezielt rehydrieren (Context-Vault-Prinzip).
- **Sidecar-Hook (optional):** konfigurierbare Schnittstelle für lokalen Helper (Summary/Kompression/Routing/Critique), graceful fallback.

### 3.5 Server/Scheduler/TUI (inkrementell)
- Scheduler: Queue + Dependency-aware Ausführung, Warmup/Status-Hooks.
- Server: Status/Enqueue/Events (bestehendes `server`-Modul erweitern).
- TUI: Task/Status/Logs Sicht (bestehende Struktur ausbauen, falls vorhanden).

## 4. Arbeitsphasen

- **Phase A – Grundlagen & Planartefakte:** `update/PLAN.md`, `update/TASKS.md` (dies), Modul-Skelette/Config/CLI-Flags verdrahten.
- **Phase B – Sandbox & CubeSandbox:** Docker/Podman Exec-Pfad, CubeSandbox-Client + Runner, Config/CLI (`run --sandbox --stack --provider cubesandbox`), Tests.
- **Phase C – Tasks/Git/CLI:** Task-Storage/Lifecycle/Chaining, `task` Subcommands, `run --task`, optionale Git-Helfer.
- **Phase D – Memory/Cognition/Repair:** `memory` + `cognition` Module, Signal-Hooks in run/task, Recall/Injection & Repair-Vorschläge, Maintenance-Kommandos.
- **Phase E – Stabilisierung:** Doku, Config-Beispiele, Build/Tests, Kompatibilitätsprüfung.

## 5. Risiken / Leitplanken
- **Kein fremder Code:** nur saubere Neuimplementierung nach Konzepten.
- **Abhängigkeiten sparsam:** neue Crates nur wenn nötig (HTTP, SQLite bereits vorhanden? prüfen), Features optional.
- **Sicherheit:** keine Secrets in Logs/Store; Sandbox-Defaults restriktiv konfigurierbar.
- **Rückwärtskompatibilität:** Defaults erhalten bisheriges Verhalten (`--sandbox/--task` opt-in).

## 6. Definition of Done (inkrementell)
- `cargo check`/`cargo build` erfolgreich, neue CLI-Flags/Subcommands verfügbar, Sandbox/Task/Memory/Cognition Module kompiliert und grundlegend verdrahtet, Konfig dokumentiert in `update/` + README/Docs-Notizen.
