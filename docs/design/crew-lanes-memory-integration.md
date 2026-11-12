# Architektur- und Implementierungsplan: Integrierte Lanes, Crew-Memory und Context-Orchestrierung

**Ziel-Repository:** `eyshoit-commits/bkg-crew-cli`  
**Basis:** `main` / aktuell geprüfter Stand `bf5c525`  
**Status:** Implementierungsstand 2026-11 (`bkgcrewcli v0.1.8`): Die integrierte Lane-/Gateway-Funktionalität wurde nativ in `bkgcrewcli` umgesetzt (Gateway-Module inkl. Scoped-Routen, Admin-API, Auth/Tokens, Streaming, Usage; statische Neon-WebUI in `webui/dist/`). Dieses Dokument beschreibt den Zielplan; Memory-/Context-Module sind in den offenen Phasen verankert.  
**Architekturvorgabe:** Die Lane-, Rate-Limit-, Queue- und Memory-Funktionen laufen innerhalb von `bkgcrewcli`. Kein separater `nim-proxy`-Prozess, kein externer Memory-Dienst und kein verpflichtender Sidecar.

## 1. Ziel und wichtige Begriffe

bkgcrewcli ist derzeit primär ein Rust-CLI, das Coding-Agenten als **Subprozesse** startet, deren native Ausgabe in einen einheitlichen Event-Stream übersetzt und optional direkte Provider-API-Aufrufe über `providers::ApiClient` ausführt. Das geplante System ergänzt es um:

1. einen integrierten Provider-Gateway-/Lane-Layer für mehrere Keys desselben Provider-/Modell-Pools;
2. ein lokal persistentes, projekt- und crew-spezifisches Multi-Agent-Memory;
3. einen Context Builder, der relevante Projektinformationen mit hartem Budget in Agenten-Tasks einfügt;
4. eine Ablaufsteuerung, die vorhandene Agentenadapter, Prozess-Cancellation, Keystore, Events und Session-Logs nutzt.

**Lane** ist ein API-Key plus dessen eigener Rate-Limit-Fensterzustand und Cooldown. **Pool** ist die Gruppe von Keys, die demselben Provider-/Endpoint-/Modell-Limit angehören. **Crew/Room** ist ein autorisierter Memory-Scope für mehrere Agenten, die an einem Projekt arbeiten.

## 2. Bestehende Basis im Ziel-Repository

Zum geprüften `main`-Stand gehören bereits:

- `src/runner.rs`, `src/process.rs`, `src/agents/*`: Agentenprozesse, Adapter, Event-Streams und Prozess-Cancellation.
- `src/config.rs`, `src/settings.rs`: `TaskConfig`, Projekt-/globale Settings und Agenten-Overrides.
- `src/keystore/mod.rs`: verschlüsselte API-Key-Verwaltung und Providerkonfiguration. Ein Provider verweist heute auf genau ein optionales `api_key_id`.
- `src/providers/mod.rs`: normalisierte Chat-Request-/Response-Typen und direkter `ApiClient` für Provideraufrufe.
- `src/event.rs`: normalisierte Events sowie `UsageData`, einschließlich Input-/Output- und Cache-Token-Feldern.
- `src/logger.rs`: lokale NDJSON-Sessionlogs und Metadaten.
- `Cargo.toml`: Rust 2021, MSRV 1.80, SQLite/SQLx und HTTP-Abhängigkeiten sind bereits vorhanden.

Der geprüfte Zielbranch enthält **noch nicht** die Crew-/Workflow-/Vault-/Cognition-/Mender-Module des früher untersuchten `bkg-zig/bkg-crew-cli`-Feature-Branches. Diese Funktionen sind daher Teil des hier geplanten neuen Zielsystems, nicht als bereits vorhandene Funktion vorauszusetzen. Vor dem Coding ist der tatsächliche neue Worktree-Stand erneut zu prüfen.

## 3. Architekturentscheidung: eingebaut statt daneben

Alle Funktionalität wird als Rust-Module in `bkgcrewcli` implementiert:

```text
CLI / Public Rust API
        │
        ├── Crew workflow + durable task state
        ├── Memory store + Context Builder
        ├── Provider admission / model governor
        ├── Global FIFO dispatcher
        ├── Provider pool: per-key lanes
        │
        ├── Direct API path: bkgcrewcli HTTP client -> selected provider/key
        └── Agent CLI path: in-process loopback gateway -> selected provider/key
                                               │
                                      supported coding-agent subprocess
```

Für direkte `ApiClient`-Requests kann der Pool den Key unmittelbar vor dem Send auswählen. Für Agenten-CLI-Subprozesse reicht es **nicht**, Schlüssel in bkgcrewcli zu konfigurieren: Die CLI führt die Providerrequests selbst aus. Daher ist für dynamische Lane-Auswahl durch unterstützte CLI-Agenten ein im selben `bkgcrewcli`-Prozess gestarteter, nur lokal erreichbarer kompatibler Gateway-Endpunkt nötig. Dieser ist ein integriertes Modul und kein separater Prozess oder Sidecar.

Der Gateway-Endpunkt muss an den Task gebunden, nur auf Loopback geöffnet, mit einem kurzlebigen zufälligen Task-Token geschützt und nach Taskende zuverlässig geschlossen werden. API-Keys werden dem Agentenprozess nicht direkt übergeben, wenn er über den Gateway geleitet wird. Unterstützt ein Adapter keine konfigurierbare Base-URL/kompatible Authentifizierung, wird er als **nicht lane-gateway-kompatibel** ausgewiesen; bkgcrewcli darf für ihn keine Key-Rotation versprechen. Alternativ kann ein solcher Agent ohne Gateway unverändert und mit genau seinem bestehenden Credentialschema ausgeführt werden.

Kein Fallback darf unbemerkt an der Lane-Verwaltung vorbei direkt zum Provider senden. Bei Gateway-Fehler gilt fail-closed, sofern der Operator nicht explizit einen unlimitierten Direct-Modus ausgewählt hat.

## 4. Integrierte Lane- und Admission-Funktionen

### 4.1 Provider-Pool und Lane-Konfiguration

Ein Pool ist mindestens durch diese Identität bestimmt:

```text
provider_id + normalized_base_url + model_id + rate_limit_scope
```

Jede Lane enthält:

```text
key_id (Keystore-Referenz, niemals Klartext in Metriken)
rpm_limit
enabled
rolling_window_send_timestamps
cooldown_until
lane_health / last_error_class
```

Fünf Keys für dasselbe Modell werden genau einem Pool zugeordnet, sofern sie dieselben Upstreamgrenzen teilen. Verschiedene Modelle oder Provider werden nicht still in denselben Pool gelegt.

Konfigurationsentwurf:

```toml
[provider_pools.nim-main]
provider = "nvidia_nim"
base_url = "https://integrate.api.nvidia.com/v1"
models = ["<model-id>"]
rate_window_seconds = 60
safety_margin_seconds = 1
selection = "least_loaded_sticky"

[[provider_pools.nim-main.lanes]]
key_ref = "nim-key-1"
rpm = 40

[[provider_pools.nim-main.lanes]]
key_ref = "nim-key-2"
rpm = 40
# ... bis zu fünf oder mehr Lanes
```

Projektkonfiguration enthält nur nicht geheime Referenzen. API-Key-Inhalte bleiben im bestehenden verschlüsselten Keystore. Keine Keys in `Debug`, Dry-Run, Tracing, Fehlermeldungen oder Config-Dumps ausgeben.

### 4.2 Rate-Limiter

- Für einen Upstream mit rollierendem RPM-Limit ein exakt passendes Sliding Window verwenden, nicht einen Token-Bucket mit Burst-Semantik.
- Die Windowlänge und Sicherheitsmarge sind pro Provider konfigurierbar; beim NIM-Profil zunächst 60 Sekunden plus 1 Sekunde konservativer Versand-/Jitter-Marge vorsehen.
- Ein Sendeslot wird beim atomaren Admission-/Sendeschritt reserviert. Parallele Requests können denselben Slot nicht doppelt erhalten.
- Nicht gesendete Reservationen werden bei Abbruch vor dem Send zurückgegeben. Bereits gesendete Requests, Retries und Hedge-Kopien verbrauchen Ratebudget und werden nicht erstattet.
- 429 `Retry-After` markiert nur die Lane, die diesen Request gesendet hat. Connect-/Transportfehler und sonstige 5xx können eigene, begrenzte Lane-Cooldowns haben.
- Sliding-Window- und Cooldown-Zustand bleibt bei einer laufenden Pool-Neukonfiguration für unveränderte Key-IDs erhalten.

### 4.3 Auswahlpolicy für fünf Keys

1. Wenn eine Conversation-Affinity-Lane sofort bereit ist, diese verwenden.
2. Ist sie voll oder im Cooldown, sofortige verfügbare Lanes nach normalisierter Auslastung (`requests_in_window / rpm_limit`) sortieren.
3. Bei Gleichstand zyklischer Cursor/round-robin als fairer Tie-Breaker.
4. Keine Lane darf nur wegen Cache-Affinity unbegrenzt blockieren; sticky wait ist standardmäßig null oder kurz konfigurierbar, danach Spillover.
5. Wenn keine Lane bereit ist: globaler FIFO-Dispatcher wartet bis zur nächsten erwarteten Freigabe und beachtet Taskdeadline/Max-Wait.
6. Requests, die dasselbe Modell verwenden, teilen den Model-Governor – auch wenn sie verschiedene Keys verwenden.

Bei fünf Keys mit je 40 RPM ist 200 RPM nur die theoretische Summe per-Key. Der reale Durchsatz ist höchstens das Minimum aus per-Key-Limits, account-/providerweiten Limits, Modell-Concurrency und verfügbarer Nachfrage.

### 4.4 Globaler Dispatcher und Backpressure

- Alle Provider-Sends, sowohl aus `ApiClient` als auch aus dem eingebauten Agenten-Gateway, gehen durch dieselbe Admission-API.
- Eine zentrale Tokio-Queue vergibt Slots in FIFO-Reihenfolge, damit neu eingetroffene Tasks nicht systematisch vor älteren Wartenden gewinnen.
- Begrenzte Queue, Taskdeadline, Cancellation und expliziter Queue-Full-Fehler vorsehen; keine unbegrenzte Warteschlange.
- Kleine Grant-Pacing-Lücke nur als konfigurierbare Stampede-Bremse einsetzen und mit Messungen begründen.
- Streaming- und nicht streamende Aufrufer erhalten aussagekräftige Queue-/Timeout-Fehler; Streaming darf bei Wartezeit Keepalive-Ereignisse senden.

### 4.5 Modellweiter Concurrency-Governor

Per-Key RPM und modellweiter Worker-Concurrency-Engpass sind separate Limits:

- Jede Anfrage benötigt sowohl eine RPM-Reservation als auch einen Modell-Permit.
- Permit wird als RAII-Guard/Drop-Guard umgesetzt, sodass Erfolg, Fehler, Deadline, Disconnect und Cancellation den Slot freigeben.
- Bei einer klar klassifizierten modellweiten Worker-Exhaustion wird Modell-Concurrency zurückgeregelt; nicht alle Keys werden deshalb in Cooldown versetzt.
- Adaptive Limitsteigerung ist später möglich, aber die erste Version sollte ein konservatives konfiguriertes Maximum und nachvollziehbare Backoff-Regeln bevorzugen.

### 4.6 Same-Model Hedging (spätere optionale Funktion)

- Standardmäßig deaktiviert.
- Nur identische Modell-/Requestsemantik konkurrieren lassen; keine implizite Modellsubstitution.
- Attempts und Verzögerung begrenzen, z. B. 1–3 Kopien, und Hedge-Budget pro Task/Projekt anwenden.
- Zusätzliche Kopien können Key-Diversität bevorzugen, dürfen FIFO, Rate-Limit oder Modell-Governor nicht umgehen.
- Jede tatsächlich gesendete Kopie verbraucht eine Lane-Reservation und potenziell Tokens/Kosten. Antwort eines Gewinners beendet Pending-Kopien; bereits verbrauchte Requests bleiben gezählt.
- Streaming gewinnt erst bei einem definierten gültigen Inhaltssignal; Antworten dürfen nie aus verschiedenen Generierungen zusammengesetzt werden.

## 5. Eingebauter Agenten-Gateway

### 5.1 Zuständigkeit

Der Gateway ist ein Rust-Modul innerhalb des ausführenden bkgcrewcli-Prozesses. Er stellt nur die Provider-API-Oberflächen bereit, die ein Agentenadapter tatsächlich benötigt. Der Gateway:

- bindet ausschließlich an `127.0.0.1` oder Loopback-Äquivalent;
- verwendet einen einmaligen, kryptografisch zufälligen Token pro Task, der nicht in Prozesslisten/Logs landet, soweit der Adapter das zulässt;
- ordnet Endpoint + Tasktoken einem `ProviderPool`/Modell zu;
- streamt SSE byte-/event-sicher, behandelt Fragmentierung, Cancellation und Downstream-Disconnect;
- prüft `model`, Requestgröße, `stream`, Content-Type und unterstützte Endpoints;
- leitet Fehlerstatus/`Retry-After` weiter und aktualisiert Lane-/Governor-Zustand;
- wird beim Taskende, Signal, Timeout und Fehler geschlossen.

### 5.2 Adapter-Kompatibilität

Für jeden Agentenadapter müssen Tests belegen, ob folgende Dinge unterstützt werden:

- konfigurierbare API-Base-URL;
- kompatibles Authentifizierungsformat für den lokalen Gatewaytoken;
- gewählte Chat-/Messages-API und Modellnamen;
- Streaming-/SSE-Verhalten;
- kein stiller Rückfall auf eingebaute Provider-Credentials.

Capability-Metadaten des `AgentRunner` um eine optionale Gateway-Kompatibilität ergänzen oder einen separaten typisierten Adaptervertrag einführen. Nicht unterstützte Agenten bleiben im Legacy-Modus und werden in `check --capabilities`/Diagnose sichtbar.

### 5.3 Sicherheitsgrenze

- Loopback allein ist keine ausreichende Autorisierung: jede Route benötigt das Tasktoken.
- Gateway akzeptiert nur Pool-/Modell- und Requestgrenzen, die bkgcrewcli beim Taskstart festlegt.
- Keine beliebige URL, freie Modellumschaltung oder universelle Proxyfunktion anbieten.
- Token bei Debug-Ausgaben und Prozessdiagnose redigieren; Keys werden nur aus Keystore-Referenzen zur finalen Upstream-Verbindung geladen.
- Kein offenes Bind an `0.0.0.0` in der ersten Version.

## 6. Kombiniertes Memory-System

Die folgenden Repositories sind **Designvorlagen**, nicht automatisch zu linkende Runtime-Abhängigkeiten:

- Uteke: gemeinsame Rooms, Agentenautor/Provenienz, Hybrid aus Volltext- und semantischem Recall.
- Shodh-Memory: gestufte Erinnerung, Deduplication, Konsolidierung und nutzungsbasierte Relevanz.
- Cortex: Working-/Episodic-/Semantic-/Procedural-Tiers, zeit-/saliencebewusstes Recall, token-budgetierter Context und Widerspruchsbehandlung.

Ziel ist, diese Funktionen in bkgcrew-cli mit der eigenen Rust-/SQLite-/SQLx-Architektur zu integrieren. Quellcode wird nicht ungeprüft kopiert.

### 6.1 Scopes und Identität

```text
project_id       stabiler, nicht geheimer Projekt-Identifier
room_id          crew/workflow-übergreifender gemeinsamer Projektbereich
workflow_id      ein Crew-DAG-Lauf
task_id          Task und Quelle/Provenienz
agent_id         schreibender Agent
conversation_id  fortlaufende Agentenunterhaltung, falls vorhanden
```

Memory-Scope ist explizit und mandantenfähig. Projektwissen wird nicht automatisch in globale User-Memory hochgestuft. Projekt-/Room-Zugriff wird vor Recall server-/anwendungsseitig autorisiert, nicht nur durch ein SQL-`scope`-Argument behauptet.

### 6.2 Tiers und Typen

```text
Working:      kurze, flüchtige Taskzusammenfassung
Episodic:     Ereignisse, Agentenübergaben, Tests, Fehler und Ergebnisse
Semantic:     geprüfte Projektfakten, Entscheidungen, Constraints
Procedural:   geprüfte Workflows, Test-/Build-/Release-Prozeduren
```

Vorgeschlagene Memory-Typen: `decision`, `fact`, `constraint`, `learning`, `error`, `repair`, `task`, `artifact_ref`, `procedure`, `observation`.

### 6.3 Provenienz, Vertrauens- und Update-Policy

Jeder Eintrag enthält Autor, Scope, Quelle, Zeit, Privacy, Version/Status, Confidence/Importance und optional Supersession-/Contradiction-Referenzen. Schreiben erfolgt über strukturierte Vorschläge:

- Agenten-Ausgaben werden nicht automatisch als bestätigte Fakten gespeichert.
- Erfolgreiche Taskzusammenfassungen können als Episoden vorgeschlagen werden; Entscheidungen/Constraints brauchen eine überprüfbare Quelle oder Review-Policy.
- Deduplication ist idempotent nach Scope und normalisiertem Inhalt/Quelle.
- Parallele Updates verwenden `expected_version`/optimistisches Locking; Konflikte bleiben sichtbar.
- Widersprüche werden erhalten und markiert, nicht still per Last-Write-Wins beseitigt.
- Memory-Text ist untrusted data und darf keine System-/Tool-/Sandbox-Regeln überschreiben.
- Secrets vor Storage, Indexierung und Promptinjection redigieren/ablehnen.

### 6.4 Retrieval und Context Builder

Zuerst bestehende SQLite-Volltext-/strukturierte Felder nutzen; semantische Embeddings später als optionales lokales Modul ergänzen. Retrieval-Pipeline:

1. Scope- und Berechtigungsfilter vor Suche.
2. Volltext-/strukturierte Suche; später optional Vektor-Retrieval.
3. Kandidaten nach Taskrelevanz, Quelle/Vertrauen, Aktualität, Pin/Importance und Provenienz ordnen.
4. Deduplizieren, Widersprüche kenntlich machen.
5. Hartes Tokenbudget und Maximum an Einträgen einhalten.
6. Artefakt-/Dateireferenzen statt kompletter Logs oder Dateien einfügen.
7. Retrieval-ID/Memory-Version für Diagnose speichern, Inhalte nicht routinemäßig loggen.

Memory-Ausfall darf optionale Memory-Injection kontrolliert auslassen und den Task weiterlaufen lassen. Ein explizit erforderliches Memory-/Policy-Setup darf stattdessen fail-closed sein; der Modus muss konfiguriert und dokumentiert werden.

### 6.5 Tokenreduktion und Prefix-Stabilität

**Sichere Tokenreduktion:** Nicht jedes Mal ganze Vorgängertranskripte, alle Agentenlogs oder irrelevante Tool-Definitionen übertragen. Stattdessen kuratierte Projektfakten, relevante Entscheidungen und kleine Task-Deltas übergeben.

**Möglicher KV-/Prefix-Cache:** stabile Reihenfolge und unveränderte Prefix-Texte können Upstream-Caching begünstigen, aber garantieren keinen Cache. Ein KV-Cache bedeutet nicht zwingend weniger übertragene oder abgerechnete Input-Tokens. Metriken für `cache_read_tokens` und `cache_creation_tokens` nur als echte Daten aus der Providerantwort führen.

Vorgeschlagene Prompt-Reihenfolge:

1. fester System-/Sicherheitsrahmen;
2. stabile Agentenrolle und stabil sortierte Tools;
3. kurze versionierte Projektzusammenfassung;
4. relevante Entscheidungen/Constraints;
5. taskbezogene Artefakt-Referenzen;
6. aktuelle Taskanweisung und Variablen.

Memory-Änderungen dürfen nicht zurückgehalten werden, nur um einen vermeintlichen Cache-Hit zu erzwingen.

## 7. Event-, Usage- und Session-Integration

- `Event::UsageDelta` und `Event::Result` werden nicht blind addiert: Final-Usage kann einen Snapshot derselben zuvor gemeldeten Nutzung enthalten.
- Usage pro Requestversuch erfassen: Provider/Modell/Pool, Lane-ID als nicht geheime interne Referenz, Versuchszahl, Input-/Output-/Cache-Tokens, Kosten, Queue-Wait, TTFT und Gesamtzeit.
- Rohprompts, Memory-Texte, API-Key und vollständige Response-Inhalte nicht in Lane-Metriklabels aufnehmen.
- SessionLogger-Policy für Credentials/Prompts und Retention separat prüfen. Für Memory-Capture nicht automatisch ganze NDJSON-Logs übernehmen.
- CLI- und Event-Schema nur mit rückwärtskompatiblen, optionalen Feldern erweitern.

## 8. Persistenz und Restart-Sicherheit

- Keystore-Erweiterung erhält Rückwärtskompatibilität für vorhandene `ProviderConfig.api_key_id`: ein bestehender Provider wird als Pool mit genau einer Lane behandelt.
- Poolmitgliedschaften und RPM-Parameter in SQLite/Config referenzieren Keystore-IDs, nie Secretwerte.
- In-memory Sliding-Window-Zustand schützt nur innerhalb eines Prozesses. Für Neustarts mit Wiederverwendung desselben Pools entweder persisted state oder konservatives Drain-/Window-Wait vor erneutem Start vorsehen.
- Kein Multi-Process-Shared-Key-Betrieb ohne atomaren gemeinsamen Limiter und Fencing/Lease. Für den ersten Release klar „ein aktiver bkgcrewcli Pool-Eigentümer je Key-Set“ erzwingen oder dokumentieren.
- SQLite-Memory-Store nutzt Transaktionen, Schema-Versionen, Indizes und Tests für Datenbank-Lock-/Recovery-Verhalten.

## 9. Implementierungsphasen

### Phase 0 — Baseline und Integrationsverträge

- Exakten Fork-HEAD/Worktree, Builds und Tests prüfen.
- Unterschiede zwischen Fork und Basisrepo prüfen, keine alten Annahmen aus einem anderen Branch übernehmen.
- Agentenadapter für Base-URL/Auth/SSE mit Capability-Matrix testen.
- Entscheiden: reiner direkter API-Pfad, integrierter Loopback-Gateway für Subprozesse oder beides.
- Fünf-Key enforcing Mock, kein echtes Providerkonto für die Grundfunktion.

**Abnahme:** Für jeden Agenten ist klar markiert, ob Gateway-/Lane-routing tatsächlich unterstützt wird.

### Phase 1 — Provider-Pool und Sliding-Window-Lanes

- `ProviderPoolConfig`, `LaneConfig`, `LaneState`, `Reservation` und `LaneLimiter` typisiert implementieren.
- Fünf Keys pro Provider-/Modellkonfiguration abbilden.
- Sliding Window, atomare Reservation, Cooldowns, Retry-After und Rebuild-Zustand testen.
- Bestehenden Einzelkey-Provider als Ein-Lane-Pool migrieren.
- Alle direkten Provider-Sends durch Dispatcher/Pool leiten.

**Abnahme:** Kein per-Key Window-Verstoß im enforcing Mock; alter Einzelkeybetrieb bleibt kompatibel.

### Phase 2 — Queue und Model Governor

- Begrenzte globale FIFO-Send-Queue und explizite Queue-/Deadline-Fehler ergänzen.
- Model-Concurrency-Permits unabhängig von RPM implementieren.
- Cancellation/Drop-Pfade in HTTP- und SSE-Send sicherstellen.
- 429 lane-scoped, modellweite Worker-Exhaustion model-scoped behandeln.

**Abnahme:** parallele Calls, Cancellation und Retries können keine Slot-/Permit-Lecks verursachen.

### Phase 3 — Lokaler Gateway für kompatible Agenten

- Loopback Listener im aktuellen Prozess starten und an Task-Lifecycle koppeln.
- Nur tatsächlich benötigte API-Oberflächen implementieren.
- Tasktoken, Host-/Model-Policy, SSE-/Streamtests und Shutdown integrieren.
- `AgentCapabilities` um verifizierte Gateway-Unterstützung erweitern.
- Direkten Fallback aus Gateway auf Provider verhindern, wenn nicht explizit erlaubt.

**Abnahme:** unterstützter Agent sendet nachweislich über eingebauten Gateway und denselben Pool; nicht unterstützter Agent wird nicht fälschlich als gepoolt angezeigt.

### Phase 4 — Projekt-Memory und Context Builder

- Lokalen versionierten Memory-Store und Scope-/Room-Policy einführen, oder bestehende Stores aus dem tatsächlich vorhandenen Fork passend weiterverwenden.
- Autor-/Task-/Quellen-Provenienz, Dedupe, Vorschläge, CAS und Konfliktmodell bauen.
- Recall und deterministischen, token-budgetierten Context Builder implementieren.
- Crew-/Workflow-Scheduler über dependency summaries und begrenzte Kontextinjektion anbinden.
- Secrets und untrusted Memory im Prompt absichern.

**Abnahme:** projektübergreifende Isolation; mehrere Agenten teilen erlaubte Fakten; parallel verfasste Memory-Updates gehen nicht verloren.

### Phase 5 — Optionale gleiche Modell-Hedges und adaptive Qualität

- Same-model-Racing optional und standardmäßig aus.
- Versuchsbudgets, Key-Diversität, erste gültige Antwort, loser cancellation und Kosten erfassen.
- Adaptive/concurrency-Policy erst nach Load-Tests, nicht als unbelegte automatische Heuristik.

**Abnahme:** jeder gesendete Hedge wird gezählt; keine Rate-/Model-Governor-Umgehung; keine Antworten aus verschiedenen Streams mischen.

### Phase 6 — Benchmark und Rollout

Vergleiche auf Testdaten, dann begrenzt und autorisiert in realer Umgebung:

- fünf Keys: per-Lane Requests, Gesamt-RPM, 429/5xx, Queue-Wait, Cooldowns;
- Modell-Concurrency: In-Flight, Worker-Exhaustion, Erholung;
- Crew-Memory: Input-Tokens pro abgeschlossenem Task, Context-Länge, Memory-Recall-Latenz, Taskqualität;
- Prefix-Cache: nur Provider-gemeldete cached tokens und TTFT, sofern Upstream entsprechende Daten liefert;
- Provider-/Modelllast und Testreihenfolge als Störgrößen berücksichtigen.

Memory-Tokenreduzierung getrennt von möglichem Provider-KV-Cache-Effekt berichten.

## 10. Konfiguration und CLI-UX

Mögliche CLI-Kommandos nach Prüfung bestehender Kommandonamen:

```text
bkgcrewcli provider pool list
bkgcrewcli provider pool validate <name>
bkgcrewcli provider pool status <name>
bkgcrewcli memory status
bkgcrewcli memory recall --project <id> --query <text>
bkgcrewcli memory inspect <entry-id>
bkgcrewcli crew run <workflow>
```

Sensible Values werden in `pool list/status` ausschließlich maskiert. `config show` und `--dry-run` zeigen Pool-/Gateway-Modus, aber keine API-Keys oder Gatewaytokens. Projekt-TOML kann Poolnamen und Memory-Scope referenzieren, nicht Secretmaterial.

## 11. Teststrategie und Release-Gates

### Unit-Tests

- Rolling-window Grenzen, Fenster-Marge, RPM 0/-ungültige Werte, gleiche Zeitstempel.
- Concurrent reservation: nie mehr als RPM Slots pro Lane.
- Sticky hit, spillover, normalized least-loaded und fairer Tie-Break.
- Lane-Cooldown/Retry-After und Fehlerklassifikation.
- Rebuild mit behaltenen Keys, deaktivierten Keys und RPM-Absenkung.
- Governor Permit Release bei Erfolg/Fehler/Cancel/Timeout.
- Memory Scope, CAS, Dedupe, Provenienz, Widerspruch, Tokenbudget und Prompt-Reihenfolge.

### Integrationstests

- enforcing Mock für fünf Keys und ein modellweites Concurrency-Limit.
- lokaler Gateway: Auth, Loopback-Bindung, SSE in fragmentierten Chunks, Abbruch, Task-Shutdown, ungültiges Modell.
- `ApiClient` direkte Route und Gatewayroute zeigen dieselben Poolzähler.
- unterstützte/unsupported Runner Capability-Matrix.
- Keystore-Migration von einem Key zu Ein-Lane-Pool.
- Project/Crew Workflow mit parallelen Tasks, Dependency-Zusammenfassungen und Memory-Sharing.
- Session logs, Debug, Dry-Run und Config-Dumps auf Secret-Redaction.

### Lokale CI

- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- Migration-/Config-Kompatibilitätstests
- keine echten API-Keys oder unbeschränkten Live-Requests in CI

## 12. Designvorlagen: Nutzung und Lizenz

Diese Referenzen liefern Architekturideen, keine behauptete gemeinsame Implementierung oder Kooperation:

- [bkg-zig/nim-proxy](https://github.com/bkg-zig/nim-proxy): per-Key Sliding Window, Cooldowns, global FIFO Dispatcher, Sticky-Affinity mit Spillover und separater Modell-Governor. Diese Funktionen werden in bkgcrewcli als interne Rust-Module nachgebaut, keine `nim-proxy`-Instanz daneben.
- [codecoradev/uteke](https://github.com/codecoradev/uteke): Rooms für geteiltes Agentenwissen, Autor-/Quellen-Provenienz, Hybrid-Retrieval und Privacy-Schutz. Lizenz im Referenzrepo: Apache-2.0.
- [varun29ankuS/shodh-memory](https://github.com/varun29ankuS/shodh-memory): lokale Memory-Tiers, Deduplizierung, Konsolidierung und nutzungsbewusste Relevanz. Lizenz im Referenzrepo: Apache-2.0.
- [gambletan/cortex](https://github.com/gambletan/cortex): token-budgetierter Context, vier Memory-Tiers, zeit-/saliencebewusstes Recall, Widerspruchs-/Confidence-Modelle und private-by-default. Lizenz im Referenzrepo: MIT.

Zunächst werden nur Konzepte/Verhalten implementiert. Falls Quellcode kopiert oder angepasst wird, müssen MIT-/Apache-2.0-Lizenzbedingungen, Copyright, NOTICE und Dependency-Policy vorab geprüft und korrekt übernommen werden. Die Zielcrate ist selbst MIT-lizenziert; Lizenzkompatibilität und Attribution sind trotzdem Teil der Review- und Release-Gates.

Als Referenz für die Lane-Logik wurde 2026 zunächst ein externer `nim-proxy`-Prozess eingesetzt (Lane-Pool, FIFO-Dispatcher, Modell-Governor, später Cloudflare-Deployment inkl. R2-State). Die Funktion ist inzwischen nativ in `bkgcrewcli` implementiert; der Referenzprozess wurde danach wieder entfernt.

## 13. Hauptrisiken und Gegenmaßnahmen

| Risiko | Gegenmaßnahme |
|---|---|
| Agenten-CLI umgeht den integrierten Pool | pro Adapter Gateway-Support nachweisen; fail-closed oder klarer ungepoolter Modus |
| fünf Keys teilen ein account-/modellweites Limit | per-key RPM vom Modell-Governor und Providerlimits trennen; messen statt 5x-Concurrency behaupten |
| mehrere bkgcrewcli-Prozesse geben denselben Key aus | exklusives Key-Ownership oder gemeinsamer atomarer Limiter mit Fencing vor Multi-Process-Betrieb |
| Memory enthält falsche Fakten oder Prompt-Injection | Provenienz, Vorschläge statt Auto-Promotion, untrusted-data framing, Widerspruchsstatus |
| Context-Kompression verwirft wichtige Regeln | gepinnte Constraints/Entscheidungen, Quellenlinks, Regressionen auf Projektaufgaben |
| KV-Cache wird als Kosten-/Tokenrabatt missverstanden | tatsächlich gesendete Usage, Provider-cache usage und Tokenreduktion getrennt messen |
| Streaming hängt bei fragmentiertem SSE oder Disconnect | Event-Parser mit Fragment-/EOF-/Cancellation-Tests und Deadline |
| Keys/Prompts in Logs oder CLI-Ausgabe | redacted Debug-Typen, Secret-Scrubbing, Security-Tests |
| Referenzcode-Lizenzen werden unbemerkt übernommen | zuerst konzeptionelle Umsetzung; bei Codeübernahme Lizenz-/NOTICE-Review |

## 14. Definition of Done

- bkgcrewcli enthält die Pool-, Rate-Limit-, Queue-, Model-Governor- und Memory-/Context-Module; es wird kein externer nim-proxy oder Memory-Dienst benötigt.
- Direkte API-Requests und als kompatibel geprüfte Agenten-CLI-Requests benutzen denselben Provider-Pool.
- Nicht kompatible Agenten zeigen sichtbar an, dass ihre API-Aufrufe nicht durch den integrierten Pool laufen.
- Fünf-Key Load- und Rate-Safety-Test besteht ohne Verletzung des simulierten Upstreamlimits.
- Projekt-Memory ist scoped, nachvollziehbar, redigiert, versioniert und mit hartem Tokenbudget injiziert.
- Tokenersparnis wird anhand tatsächlich gemeldeter Usage und derselben Task-Baseline gemessen; KV-Cache-Effekt wird nicht behauptet ohne upstreamseitigen Nachweis.
- bestehende CLI-, Config-, Event- und Einzelkey-Verträge bleiben rückwärtskompatibel oder erhalten eine explizite Migration.
- Format, Workspace-Tests, Clippy, Security- und Lizenzreview bestehen.

````