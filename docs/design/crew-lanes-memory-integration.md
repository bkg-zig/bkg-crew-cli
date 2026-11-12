Zielarchitektur: bkgcrewcli als lokale Agenten- und Provider-Plattform

Basis: Branch `feature/crew-sandbox-integration`, geprüfter HEAD `4b4a70f` (v0.1.9); vor jeder Implementierung den echten HEAD, Module und Tests neu verifizieren.
Status: Architektur- und Implementierungsplan; keine Runtime-Implementierung in diesem Dokument.
Eigenentwicklung: Die genannten Repositories sind ausschließlich Ideengeber. Es wird kein Quellcode kopiert, kein Referenzprojekt als verpflichtende Runtime-Abhängigkeit eingebunden und keine Zusammenarbeit behauptet. Wir schreiben und testen die Implementierung selbst.

1. Vision und Prioritäten
bkgcrewcli soll Agenten, Providerzugriff, Crew-Workflows und Projektwissen in einer überprüfbaren lokalen Plattform vereinen. Es soll nicht einfach möglichst viele Features anhäufen: Jede Funktion braucht eine klare Zuständigkeit, Berechtigungsgrenze, Messbarkeit und Ausfallstrategie.

Priorität und Abhängigkeiten:

Nim-proxy-inspirierte Provider-Lanes und Admission-Control als zuverlässige Unterlage für alle verwalteten Provider-Sends.
Copilot-API-inspirierte, multi-protokollfähige Gateway-Verträge: mehrere Client-Protokolle, Provider-/Modellrouting, Capabilities, Streaming und Diagnose – innerhalb der bereits vorhandenen Rust-Gateway-Basis.
Crew-/Task-Orchestrierung auf den vorhandenen Agenten-, Prozess- und Queue-Funktionen aufbauen.
Shodh-/Uteke-inspiriertes, lokales und belegbares Projekt-/Crew-Memory plus deterministischer Context Builder.
srag-inspirierte Code-Suche und TinyJuice-inspirierte, wiederherstellbare Tool-Output-Reduktion, nachdem Scope, Storage und Recovery definiert sind.
Prmt-inspirierte Terminal-UX nur, falls sie messbar nützlich ist; kein Kernsystem und keine notwendige Abhängigkeit.
Alles ist selbst zu implementieren. „Feature übernehmen“ bedeutet hier, gewünschtes Verhalten zu spezifizieren, nicht Code, interne Endpunkte oder nicht dokumentierte Authentifizierungsverfahren nachzubauen.

2. Tatsächlicher Ziel-Repository-Kontext
Das Ziel ist bkg-zig/bkg-crew-cli (dieses Repository). Das README beschreibt ein Rust-CLI mit Agent-Subprozessen (Claude Code, Codex, OpenCode, Cursor, Pi, Copilot), einheitlichem NDJSON-Eventstream, Model Registry und Projektkonfiguration. Der integrierte serve-Gateway umfasst Chat Completions, Responses und Anthropic Messages, Provider-scoped Routen, Tokens, Streaming/Usage sowie eine vorhandene WebUI.

Geprüfter Stand (HEAD `4b4a70f`): Die Module src/crew, src/cognition, src/vault, src/mender und src/task_queue existieren in diesem Repo. Davon sind vault/cognition/mender als SQLite-Stores vorhanden, aber nicht in Runner/CLI/Server verdrahtet; crew ist ein verwaistes, nicht in lib.rs eingebundenes Modul und muss erst repariert werden; Lanes/Queue/Governor, Code-Index und Artefakt-Recovery existieren noch nicht. Vor Implementierung ist der aktuelle HEAD erneut mit den tatsächlichen Dateien zu vergleichen.

3. Architekturübersicht
CLI / Public Rust API / vorhandene WebUI
        │
        ├── Task- und Crew-Orchestrierung
        │     └── DAG, durable state, retry policy, deadline, cancellation
        ├── Context Orchestrator (Budget + provenance)
        │     ├── Memory store: project / room / task scopes
        │     ├── Code index: symbols / FTS / optional local vectors
        │     └── Artifact store: tool output + explicit recovery handles
        ├── Agent adapters + verified capability matrix
        ├── Provider Gateway: Chat / Responses / Messages adapters
        └── Shared Provider Admission API
              ├── bounded FIFO dispatcher
              ├── per-key sliding-window lanes and cooldowns
              ├── model/account concurrency governor
              └── provider transports (HTTP / supported WebSocket)
Provider routing, Agent-CLI compatibility, task scheduling, memory, code indexing und Artefaktkompression sind getrennte Komponenten, aber werden mit stabilen IDs und Events integriert. Alle Persistenzen sind lokal und haben unterschiedliche Retention-, Verschlüsselungs-, Backup- und Löschregeln.

4. Fundament: nim-proxy-inspirierte Lane- und Admission-Schicht
Die zentrale Lane-Logik kommt zuerst: weder Agenten noch Gateway-Routen dürfen Rate-Limits, Concurrency-Regeln oder Warteschlangen umgehen. Der Zielprozess kann vorhandene Gateway-/Provider-Module wiederverwenden, falls sie im aktuellen HEAD existieren; der Lane-Mechanismus wird als eigene Rust-Implementierung spezifiziert.

4.1 Provider-Pool und Lane
Pool-Schlüssel: provider_id + normalized_endpoint + model_or_shared_scope + rate_limit_scope. Jede Lane verweist nur über eine geheime Keystore-ID auf Credentials und hält rpm_limit, aktiv/inaktiv, Sliding-Window-Sendezustand, Cooldown und nicht geheime Health-/Fehlerklasse.

Bestehende einzelne Provider-Keys bleiben als Pool mit einer Lane kompatibel.
Keys werden nur dann über einen Pool addiert, wenn ihre realen Upstream-Limits dies erlauben. Keine Durchsatz- oder ToS-Umgehung versprechen.
API-Key-Text darf nicht in Logs, Metrikenlabels, Debug, Dry-Run oder Config-Ausgaben erscheinen.
4.2 Limiter, Queue und Auswahl
Sliding Window passend zum echten Providerlimit; kein burstfähiger Token-Bucket, wenn der Upstream Rolling RPM durchsetzt.
Reservationen sind atomar. Vor-Send-Abbruch erstattet ungesendete Slots; gesendete Requests, Retries und Hedges zählen.
Begrenzte zentrale FIFO-Warteschlange mit Queue-Full, Taskdeadline, Cancellation und konfiguriertem Max-Wait.
Lane-Auswahl: sofort nutzbare Conversation-Affinity, sonst normalisierte Auslastung (in_window / rpm_limit) mit round-robin-Tie-Break, dann Spillover. Affinity darf nicht unbegrenzt blockieren.
429/Retry-After kühlt die betroffene Lane ab. Modellweite Worker-Exhaustion regelt nur den Modell-Governor, nicht pauschal alle Lanes.
Request braucht RPM-Reservation und Modell-Concurrency-Permit; RAII-/Drop-Guards geben Permits auf jedem Ende frei.
Konversations-affine Lane kann Provider-Prefix-Caching begünstigen, garantiert aber weder Cache-Hit noch Tokenrabatt.
Retries sind begrenzt, klassifiziert und deadline-aware. Hedging bleibt standardmäßig deaktiviert und muss dasselbe Modell, Semantik und dieselben Budgetregeln nutzen.
4.3 Neustart und Mehrprozessbetrieb
In-memory Ratezustand allein ist nach Restart keine sichere Grundlage. Für aktivierte Pools ist persistierter Zustand oder ein konservativer Drain-/Window-Wait erforderlich. Mehrere aktive Prozesse dürfen dieselben Keys nicht ohne gemeinsamen atomaren Limiter und Fencing ausgeben. Der erste sichere Betrieb sollte genau einen Pool-Eigentümer je Key-Set erzwingen oder klar dokumentieren.

4.4 Streaming-Verhalten
SSE-Parser und -Writer müssen fragmentierte Events, Keepalive, EOF, Downstream-Disconnect, Cancellation, Usage-Endevents und Deadline behandeln. Ein Streaming-Request darf dem Client nur dann Erfolg signalisieren, wenn das gewählte Protokoll spätere Fehler zuverlässig als terminales Event ausdrücken kann. Heartbeats sind keine Verlängerung der absoluten Deadline.

5. Copilot API: Feature-Abdeckungsprüfung und sinnvolle Kombination
Die öffentliche caozhiyuan/copilot-api-README beschreibt einen lokalen Universal-Gateway für OpenAI Chat Completions, Responses und Anthropic Messages, mehrere eingebaute und Drittanbieter-Provider, Agenten-Setups, SSE/WebSocket, Tool-/Modellkompatibilität, Model Discovery/Mappings, Auth-/Konfiguration, Usage Monitoring und eine Electron-Desktop-App. Die Liste unten macht transparent, was der Zielplan übernimmt, was durch vorhandene Ziel-Funktionalität bereits adressiert ist und was bewusst nicht in den Kern wandert. Das ist eine Architekturabdeckung, kein Claim gleicher Implementierung oder vollständiger Feature-Parität.

Copilot-API-Idee	Plan im bkgcrewcli-Ziel	Entscheidung / zusätzlicher Nachweis
Ein Gateway für Chat Completions, Responses und Messages	Beibehalten und härten; bestehende Gateway-Routen des Zielrepos prüfen, Protokolle als typisierte Adapter	Contract- und Round-trip-Tests je Endpoint, Client und Upstream; keine pauschale Übersetzung aller Felder behaupten
Mehrere Provider und Providerwahl	Erweitern über Registry, explizite Provider-Profile und Modelle; jeden Send über gemeinsame Lane-/Admission-API routen	Feature-Support matrix je Provider, Endpoint und Modell; unbekannte Kombination klar ablehnen
Agent-CLI-fertige Konfiguration	Beibehalten/erweitern über AgentCapabilities und geprüfte Config-Templates	Pro Agent testen: Base URL, Auth, Modell, tool calls, stream und kein unerwarteter Credential-Fallback
Chat/Responses/Messages Übersetzung	Selektiv implementieren als explizite Adapter, keine lossy magic translation	Schema-Kompatibilität versionieren; nicht unterstützte Tools/Modalitäten in Capability-Antworten ausweisen
SSE über mehrere Protokolle	Übernehmen mit Parser-Fixtures, Heartbeats und Cancellation	Chunk-Fragmentierung, Disconnect, Last-Event/Usage und Fehlerfälle testen
WebSocket-Upstream für Responses	Optionaler späterer Transport hinter demselben Admission-/Usage-Vertrag	Nur wenn Zielprovider es dokumentiert unterstützen und HTTP/SSE Tests/Benchmarks belegen Nutzen
Modellkatalog, Discovery, Aliases, Mapping	Erweitern aus bestehender Model Registry/scoped routes	Cache, TTL, single-flight Refresh, stabile IDs; Discovery darf keine Rate-Limit-Sends verbrauchen, wenn Provider dies ermöglicht
Tool-Use, tool_choice, Tool-Search/MCP bridge	Teilweise, explizit: standardisierte Tool-Calls und optionale deferred-tool-Schnittstelle	JSON-Schema, tool_choice, Round-trip IDs und Tool-Ergebnisgrenzen testen; nichts stillschweigend entfernen
Multimodalität (Bilder/Dateien)	Capability-gesteuert	Payload-Limits, MIME-/Format-Validierung, Redaction/Logs; keine Inhalte in Diagnose kopieren
OAuth-/Provider-Credentials	Nur autorisierte, dokumentierte Wege über bestehende Keystore/Auth-Adapter	Keine nicht dokumentierte Credential-Extraktion, privaten Endpunkte oder Zugriffskontrollumgehung; Providerzugang ist je Anbieter zu prüfen
Usage, Token/Kosten-/Quota-Ansichten	Übernehmen, an bestehende Usage/WebUI anschließen	exakte Providerusage von Schätzwerten trennen; Modellpreise/Quoten mit Quelle und Zeitstempel versionieren
Logs, Diagnose, Konfiguration	Übernehmen mit redigierten Request-Metadaten, Doctor und Konfigurationsvalidierung	Keine Requestinhalte in standardmäßigen Logs; Secrets immer redigieren
Desktop Electron App	Nicht in den Rust-Kern übernehmen	Vorhandene WebUI/CLI ist bereits die passende Oberfläche; zusätzliche Desktop-Runtime wäre Overhead und doppelte Konfigurationsquelle
Node/Bun-Paket-/Installationsmodell	Nicht übernehmen	Ziel ist native Rust-CLI/Server, keine parallele Runtime
Provider-spezifische Subscription-/OAuth-Tricks	Nicht als allgemeine Gateway-Funktion	Nur öffentlich unterstützte und autorisierte Providerintegration. Keine Reverse-Engineering- oder ToS-Umgehungsziele
Ergebnis der Abdeckung: Die aktuell geprüfte Architektur-Referenz des Ziels deckt bereits Chat Completions, Responses, Messages, scoped routing, Tokens, Streaming, Usage und WebUI ab. Sie belegt aber nicht allein die volle Copilot-API-Abdeckung von Multi-Provider-Capabilities, durchgängigen Tool-/Multimodal-Roundtrips, modellbezogener Transportwahl, robustem Discovery/Mapping, getrenntem Quota-/Usage-Modell oder Desktop-App-Funktionen. Die obige Tabelle markiert diese als neue/zu verifizierende Arbeit oder bewusste Nicht-Ziele. Wir sollten nicht behaupten, Copilot-API sei vollständig absorbiert, bis die Feature-Matrix pro Protokoll/Provider mit Tests ausgefüllt ist.

6. Agent-Adapter und Kompatibilitätsmatrix
Jeder Agent und Provider bekommt explizite Capabilities, z. B.:

protocols: chat | responses | messages
streaming: sse | websocket | none
features: tool_calls, tool_choice, json_mode, images, files, usage
routing: configurable_base_url, auth_scheme, model_mapping
limits: request_bytes, context_tokens, max_output_tokens
fallback: none | explicitly_unmanaged
Diese Angaben sind nicht bloß Konfiguration; CI muss sie durch contract tests verifizieren. Ein Adapter darf niemals als „durch Lanes gepoolt“ ausgewiesen werden, wenn er tatsächlich intern eigene direkte Provider-Credentials verwendet. Nicht unterstützte Felder werden strukturiert zurückgemeldet statt still verworfen.

7. Crew-Task-Orchestrierung
Auf den tatsächlich existierenden Task-/Crew-Modulen aufbauen. Workflow ist ein dauerhafter DAG aus typed tasks mit workflow_id, task_id, Abhängigkeiten, Agent/Provider-Zuweisung, Deadline, Retry-Policy, Cancellation-Token, Workspace-Sandbox und Result-Ref.

Ergebnisse zwischen Aufgaben als begrenzte Dependency-Summaries und Artefakt-IDs übergeben, nicht gesamte Transkripte.
Taskzustände und Event-IDs crash-konsistent persistieren; nach Restart eindeutige Fortsetzung oder explizit interrupted melden.
Tasks können unterschiedliche Agents/Modelle nutzen, aber Modellwahl und Fallback sind explizite Workflow-Policy, nicht stille Substitution.
Side effects (Dateiänderungen, Commands, Commits) bleiben über Sandbox-/Permission-Policy und menschliche Freigaben steuerbar.
Idempotency keys für wiederholte Workflow-Submission und Tool-/Memory-Capture verwenden.
8. Memory-Kern: Shodh + Uteke
8.1 Datenmodell und Scope
project_id, room_id, workflow_id, task_id, session_id
agent_id, author_kind (human|agent|system), source_ref
memory_id, type, tier, content_hash, version, status
created_at, valid_from, valid_until, confidence, importance
privacy_class, tags/entities, accessed_at, access_count
relations: supports | contradicts | supersedes | references | co_retrieved
Projekt- und Room-Scope sind echte Berechtigungsgrenzen; erlaubte IDs müssen vor Recall gefiltert werden. Projektwissen wird nicht automatisch in globale User-Memory hochgestuft. Writes sind versioniert/optimistisch (expected_version). Autoren-/Quellen-Provenienz bleibt erhalten. Dedupe ist idempotent nach Scope, normalisiertem Inhalt und Quelle.

8.2 Tiers und Lernregeln
Working: flüchtige Taskziele, aktueller Fortschritt, offene Fragen.
Session/Episodic: Taskergebnisse, Test-/Fehlerbeobachtungen, Übergaben und Quellen.
Long-term/Semantic/Procedural: geprüfte Fakten, Constraints, Entscheidungen, Muster und Prozeduren.
Shodh-inspirierte Zugriffsstärke, Vergessen/Decay und Konsolidierung dienen nur dem Ranking und der Wartung – niemals Wahrheitsprüfung oder Scope-Autorisierung. Deterministische Regeln zuerst. Kein LLM-Aufruf ist für grundlegendes Speichern oder Abrufen erforderlich. Promotion aus Episoden in geprüfte Fakten bleibt Review-/Policy-gesteuert. Originalbelege bleiben archiviert/verknüpft.

Uteke-inspirierte Rooms, Zeitabfragen, Quellenzitate, typed relations, Widerspruchs- und Supersession-Kanten sowie tombstones für explizit zurückgewiesene Werte machen Memory nachvollziehbar und konfliktfest.

8.3 Recall und Sicherheit
Erste Version: strukturierte SQLite-Filter + FTS, Tags/Entities, Zeit-/Roomfilter und relationale Links; deterministisches Ranking/RRF. Lokale Embeddings/ANN und Cross-Encoder bleiben optionale, austauschbare Traits, bis Qualität, Modellgröße, Offlineverhalten und Latenz gemessen sind.

Secrets vor Persistenz und Embedding redigieren; optional strikter Modus lehnt sensible Writes ab. Recalled Memory/Code ist untrusted data und wird im Prompt als Dateninhalt gekennzeichnet. „Forget“ entfernt auch abgeleitete Index-/Cache-Inhalte, mit nachvollziehbarem Tombstone, wenn nötig.

8.4 Capture statt blindem Auto-Save
Beim Taskabschluss kann ein Review-fähiger Episodenvorschlag entstehen: Ziel, Änderungen/Artefakte, tatsächlich ausgeführte Tests und deren Ergebnis, beobachtete Fehler, Lösung und offene Fragen. Jeder Claim braucht Event-/Datei-/Tool-Quelle. Keine vollständigen Prompts oder Session-Logs ungeprüft als Memory speichern.

9. Projektübergreifender Code-Index: srag-Ideen
Lokaler optionaler Index mit Allowlist pro Projekt, .gitignore-/Build-/generated-Ausschlüssen, Secret-Scrub vor Persistenz und inkrementeller Hash-Synchronisierung.

Exakte Suche und SQLite FTS als robuste Basis.
Symbol-/AST-aware Chunks mit Tree-sitter, Zeilenbereichen und markiertem Fallback.
Cross-project Pattern-/Convention-Suche mit expliziter Projekt-Allowlist.
Optional lokaler Vector Search und Reciprocal Rank Fusion.
Caller/Callee- und Call-Graph-Kontext nur für Sprachen/Parser mit validierter Genauigkeit.
Treffer liefern Projekt, Pfad, Zeilen, Symbol, Index-/Commit-Version und Relevanzgrund. Standardmäßig gibt es kleine Snippets; Inhalte können gezielt mit Datei-/Zeilenrange nachgeladen werden. Keine Projektübergreifende Suche außerhalb der autorisierten Allowlist.

10. Wiederherstellbare Tool-Output-Kompression: TinyJuice-Ideen
Content-aware Policies für Logs, Tests, Diffs, JSON, Search und HTML; pro Agent/Tool/Task off, light, full. Deterministische Reducer behalten Fehler, Warnungen, Stacktraces, geänderte Zeilen, Signaturen, relevante Treffer und Anomalien. Exakte Datei-Lesevorgänge bleiben standardmäßig unberührt.

Verlustbehaftete Ausgabe muss Auslassungen offen markieren. Original wird in task-/scopegebundenem Artefakt-Store mit TTL, Size Cap, Auth und Löschpfad hinterlegt und per Recovery-Tool selektiv abrufbar. Wenn Original nicht sicher gespeichert werden kann, verlustbehaftete Kompression ablehnen bzw. klaren Größenfehler melden; niemals still abschneiden. Keine automatische LLM-Zusammenfassung im ingest-Pfad der ersten Version.

TinyJuice- und srag-README kennzeichnen GPLv3/GPL-3.0. Der Zielrepo ist laut README MIT-lizenziert. Es wird weder Code kopiert noch eine direkte Runtime-Abhängigkeit eingezogen; wir implementieren unabhängige Spezifikationen und lassen Lizenz-/Dependency-Policy vor Release prüfen.

11. Context Orchestrator
Ein einziger deterministischer Builder kombiniert Task, Memory, Code-Suche, Artefaktverweise und Adapterwissen unter hartem Tokenbudget. Es ist kein weiterer Speicher.

Prioritäten:

System-/Safety-Policy und unveränderte aktuelle Taskanweisung.
Agent-Rolle/Tools, soweit nicht native Agentenconfig.
Gepinnte, gültige Projekt-Constraints und geprüfte Entscheidungen.
Wenige Code-Symbole/Snippets mit zitierbarer Quelle.
Relevante Episoden, Fehler, Reparaturen und Prozeduren mit Provenienz/Alter.
Dependency-Summaries, Recovery- und Artefakt-IDs.
Je Block Tokenkosten schätzen, Duplikate reduzieren, Widersprüche markieren, Max-Einträge durchsetzen und verarbeitete Quellen referenzieren. Getrennte Context-Buckets (system, tools, memory, code, conversation) ermöglichen Diagnose. Provider-gemeldete Usage übersteuert lokale Schätzung. Kein Versprechen, dass Prefix-Stabilität Upstream-Cache oder Kostensenkung bewirkt.

12. WebUI, CLI und Betrieb
Vorhandene WebUI und Diagnoseflächen nutzen, statt einen zweiten Admin-Client einzuführen. Ergänzungen in sinnvoller Reihenfolge: Pool-/Lane-Status, Queue/Modell-Capacity, Provider-Protokollmatrix, Agent-Capabilities, Usage/Quota, Memory-/Index-Health, Retrieval-/Recovery-Metadaten und Doctor.

Dashboard zeigt Trends, Health, Kapazität, Queue-/TTFT-/Latenz-Aufschlüsselung, pro-Modell/per-Lane-Auslastung und Providerusage. Metriken nutzen counts/sizes/status/strategy statt Prompt-, Memory- oder Codeinhalt. „Measured“, „estimated“ und „unavailable“ werden unterschieden. Retention ist konfigurierbar.

Prmt ist nur Inspiration für schnellen Kontextstatus (Projekt, Branch, aktiver Workflow, Queue, letzte Tests), nicht als Dependency. Keine langsame Versionsabfrage/Netzwerkzugriffe pro Shell-Prompt; Plain-Text und no-color sind Pflicht.

13. Phasen und Abnahmekriterien
Phase 0 — HEAD-Audit und Feature-Matrix
Ziel bkg-zig/bkg-crew-cli HEAD, bestehende Gateway-/Provider-/Agent-/WebUI-/Task-Module, Crate-Lizenz und Tests auditieren.
Copilot-API-Tabelle für jeden Endpoint, Provider, Model discovery/mapping, tool use, streaming/transport, usage/auth ausfüllen: exists / partial / missing / intentionally excluded plus Testreferenz.
Pro Agent/Provider capability matrix; nur unterstützte und dokumentierte Auth- und Endpoint-Verträge.
Gate: kein Upstream-Modul irrtümlich als Zielrepo-Basis vorausgesetzt; jeder gewünschte Featurebereich hat Status und Beleg.

Phase 1 — Lanes + Dispatcher + Governor
ProviderPool/Lane/Reservation, exact sliding window, persistent/conservative restart state, FIFO Queue, deadlines, cancellation, per-model permits, cooldown taxonomy.
Alle verwalteten HTTP-/SSE-/ggf. WebSocket-Sends über dieselbe Admission API.
Fünf-Key enforcing mock mit 429/Retry-After, worker slots, disconnect und cancellations.
Gate: keinerlei Rate-Verstöße im Mock, kein Permit/Reservation-Leak, keine versteckten Sends außerhalb des Pools.

Phase 2 — Multi-protocol Gateway-Abdeckung (Copilot-API-Inspiration)
Bestehende Chat/Responses/Messages contracts härten und Endpoint-Matrix vervollständigen.
Multi-provider routing, model mappings/discovery, tool round-trip, streaming und Usage konsistent anbinden.
Provider-/Modellfähigkeiten und unsupported parameters explizit ausweisen; CLI config examples und Doctor.
WebSocket nur nach protokollspezifischem Nutzen-/Kompatibilitätsnachweis.
Gate: Contract tests je Feature-Matrix-Zelle; keine falsche Full-Parity-Aussage.

Phase 3 — Durable Crew-/Taskkoordination
Task DAG, IDs, persisted status/events, bounded dependency summaries, retry/deadline/cancel und Sandbox-Bindung.
Idempotente Submission und restart/recovery Tests.
Gate: parallele/abgebrochene/restarted Workflows behalten nachvollziehbaren Zustand.

Phase 4 — Memory + Context Builder
Scopes/Rooms/Provenienz/FTS, CAS, Dedupe, relations, tombstones, backup/restore und secret redaction.
Token-budgetierter Context und task evidence capture proposals.
Gate: kein cross-project leak; Memory kann ausfallen ohne unerwarteten Taskverlust; required-policy ist explizit fail-closed.

Phase 5 — Code Index + Recoverable artifacts
Inkrementeller FTS/AST-Index und berechtigte Suche; optionale Vektoren nach Quality Gate.
content-aware reducers, loss markers, Task Recovery API, TTL/capacity and fail-safe passthrough.
Gate: Treffer sind zitierbar; kein stiller Verlust; recovery round-trip und Löschung funktionieren.

Phase 6 — UX, adaptive policy and rollout
Aggregierte WebUI-/Metrics-/Doctor-Anzeigen, opt-in flags, Canary und klare Downgradepfade.
Adaptive lane/concurrency policies oder hedging nur nach reproducible load tests, default off.
Gate: Sicherheit, Lizenz, Usage/Kostenklarheit, Performance und Rollback getestet.

14. Tests und Qualitätsmessung
Gateway: Protocol contract tests, tool-call round-trip, model mapping/discovery, malformed/unsupported parameters, SSE fragmentation, WebSocket only if enabled, auth and redaction.
Lanes: strict rolling-window enforcement, FIFO fairness, sticky spill, cooldown, 429/Retry-After, model governor, restart, multiprocess fencing.
Crew: DAG edges, idempotency, process cancellation, retry policy, crash recovery, sandbox permissions.
Memory: scope isolation, CAS conflict, dedupe, time-travel, relations, tombstones, privacy filters, backup/restore.
Code search: expected top-k symbols, exact line ranges, generated/secret exclusions, incremental hash update and project allowlist.
Compression: no-loss/marked-loss behavior, original retrieval exactness, safety failures, fixture signal preservation, TTL and hard caps.
Evaluation: own reproducible datasets and receipts; report recall@k, source coverage, false scope hits, task success, tool/context tokens, recovery success, latency and storage. Do not copy reference benchmark claims as bkgcrewcli results.
CI: cargo fmt --all -- --check, cargo test --workspace, cargo clippy --workspace --all-targets -- -D warnings, migrations, security and license checks. No live keys or unlimited real-provider load tests.
15. Referenzen und Übernahmegrenzen
Alle sind Verhaltens-/Designreferenzen. Keine Quellcodeübernahme:

bkg-zig/nim-proxy — MIT laut README; Sliding-Window-Lanes, FIFO dispatcher, affinity/spillover, Retry-After, Modell-Concurrency, SSE heartbeats, deadlines, usage and operational dashboards. Primäre Vorlage für Provider Admission, unabhängig in bkgcrewcli implementieren.
caozhiyuan/copilot-api — MIT laut Repository-Metadaten; Universal Gateway, Chat/Responses/Messages, provider/model routing, agent configuration, SSE/WebSocket, discovery/mappings, tools, usage and desktop UX. Vorlage für die zu testende Multi-protocol Feature-Matrix, kein Claim vollständiger Parität.
varun29ankuS/shodh-memory — Apache-2.0 laut README; local-first memory tiers, offline/no-LLM recall, dedup, usage signals and consolidation.
codecoradev/uteke — Apache-2.0 laut README; Rooms, author/source, hybrid retrieval, relations, time-aware recall, privacy and reproducible evaluation.
wrxck/srag — GPL-3.0 laut README; incremental AST-aware cross-project indexing, hybrid semantic/full-text search, patterns, caller/callee and secret filtering. Concept-only; independently implement.
tinyhumansai/tinyjuice — GPLv3 laut README; profile-aware content reducers and recoverable lossy outputs. Concept-only; independently implement.
leeguooooo/agent-cli-to-api — protocol facade and multiple CLI backend adapter patterns; confirm exact license and provider-supported auth before any future dependency/copy (not planned).
3axap4eHko/prmt — MIT laut README; fast, context-aware terminal presentation only.
Vor jedem Release werden Ziel-Lizenz, alle direkten/transitiven Dependencies und SBOM erneut geprüft. Besonders GPL-3.0-Referenzen bleiben konzeptionell, weil der Ziel-README MIT nennt. Auch MIT/Apache-Repos werden nicht kopiert: dies hält Ownership, Architektur und Lizenzlage klar.

16. Definition of Done
Provider-Lanes/Queue/Model-Governor decken sämtliche verwalteten Provider-Sends ab und bestehen enforcing Rate-/Concurrency-Tests.
Copilot-API-Feature-Matrix ist ehrlich ausgefüllt: jede Zelle getestet oder als partial/intentionally excluded begründet; keine unbelegte Aussage „alles absorbiert“.
Agenten-Kompatibilität und unverwaltete Fallbacks sind sichtbar und überprüfbar.
Crew-Tasks sind durable, bounded, cancellable und sandbox-/permission-aware.
Memory ist lokal-first, scoped, nachvollziehbar, redigiert, export-/backupfähig und sicher löschbar.
Codeindex liefert belegte Treffer nur für autorisierte Projekte.
Verluste durch Kompression sind sichtbar und wiederherstellbar oder Kompression findet nicht statt.
Context Builder hat ein hartes Budget; Metriken speichern keine Inhalte.
Keine Referenz-Quellcodes oder unfreigegebenen Runtime-Abhängigkeiten; Lizenz-, Security-, CI- und Rollback-Gates bestehen.

# Architektur- und Implementierungsplan: Integrierte Lanes, Crew-Memory und Context-Orchestrierung

 
**Basis:** `feature/crew-sandbox-integration` / aktuell geprüfter Stand `4b4a70f`  
**Status:** Implementierungsstand 2026-11 (`bkgcrewcli v0.1.9`): Der Gateway ist nativ in `bkgcrewcli` umgesetzt (Gateway-Module inkl. Scoped-Routen, Admin-API, Auth/Tokens, Streaming, Usage; statische Neon-WebUI in `webui/dist/`). Lane-/Admission-, Code-Index- und Artefakt-Funktionen sind noch nicht implementiert; Memory-/Context-Module (vault/cognition/mender) existieren als Stores, sind aber nicht verdrahtet. Dieses Dokument beschreibt den Zielplan; die offenen Phasen sind unten verankert.  
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

Der geprüfte Zielbranch enthält die Crew-/Workflow-/Vault-/Cognition-/Mender-Module bereits, jedoch in unterschiedlichem Reifegrad: vault/cognition/mender sind eigenständige SQLite-Stores ohne Verdrahtung in Runner/CLI/Server, crew ist nicht in `lib.rs` eingebunden und task_queue hat keine Retry-/Cancel-/Recovery-Logik. Diese Funktionen sind daher zu reparieren und zu verdrahten, nicht als fertig vorauszusetzen. Vor dem Coding ist der tatsächliche Worktree-Stand erneut zu prüfen.

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

- Exakten HEAD/Worktree (`feature/crew-sandbox-integration`, geprüft `4b4a70f`), Builds und Tests prüfen.
- Keine alten Annahmen aus einem anderen Branch oder Repo übernehmen; Modulstand im aktuellen HEAD verifizieren.
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

- Lokalen versionierten Memory-Store und Scope-/Room-Policy einführen, oder die bestehenden Stores (vault/cognition/mender) aus diesem Repo passend weiterverwenden und verdrahten.
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

Als Referenz für die Lane-Logik diente 2026 zunächst ein externer `nim-proxy`-Prozess (Lane-Pool, FIFO-Dispatcher, Modell-Governor). Die Lane-Funktion ist in `bkgcrewcli` noch zu implementieren (Phase 1); der Referenzprozess ist keine Runtime-Abhängigkeit.

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