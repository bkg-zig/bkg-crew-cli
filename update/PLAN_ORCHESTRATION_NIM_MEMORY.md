# Plan: Orchestrierung mit gemeinsamem Agenten-Memory und intelligentem Lane-Routing

**Status:** Architekturvorschlag zur Prüfung; keine Implementierung oder Integration der referenzierten Projekte ist erfolgt.  
**Ziel:** Für jedes Provider-/Modell-Paar gibt es fünf API-Key-Lanes. Mehrere Agenten arbeiten am selben Projekt. Eine lokale/selbst gehostete Memory-Schicht teilt freigegebene Projekterkenntnisse, baut einen kleinen stabilen Prompt-Kontext und reduziert damit redundante Eingabe-Tokens. Das Lane-Routing bevorzugt Cache-Lokalität, nutzt Spillover bei Engpässen und wahrt alle RPM- und Modell-Concurrency-Grenzen.

## 1. Zusammenfassung der Empfehlung

Die drei referenzierten Projekte ergänzen sich als **Designvorlagen**, nicht als blind zu übernehmende Pakete oder als Ersatz für den bestehenden Rate-Limiter:

- **Shodh-Memory:** Vorbild für lokale, LLM-freie Aufnahme und Pflege von Memory, gestufte Working-/Session-/Long-term-Speicher, Deduplizierung und nutzungsbasierte Konsolidierung.
- **Uteke:** Vorbild für explizite gemeinsame Räume (Rooms) mehrerer Agenten, Autor-/Quellen-Provenienz, Recall per Hybrid-Suche, zeitliche Abfragen und Datenschutzfilter.
- **Cortex:** Vorbild für getrennte Working-/Episodic-/Semantic-/Procedural-Memory, token-budgetierte Context-Erzeugung, zeit-/saliencebewusste Retrieval-Signale, Widerspruchsbehandlung und private-by-default Memory.
- **nim-proxy:** Bleibt die Autorität für Upstream-Request-Routing, per-Key-RPM, Cooldowns, FIFO-Admission und Modell-Concurrency. Memory-Retrieval sitzt vor dem Proxy im Orchestrator/Context Builder und darf niemals RPM-Slots umgehen.

**Priorität:** Zuerst durch strukturiertes, kurzes Projekt-Memory tatsächlich weniger Prompt-Tokens senden. Danach stabile Prompt-Präfixe und Conversation-Affinity messen, um mögliche KV-/Prefix-Cache-Treffer beim Upstream zu fördern. KV-Cache ist providerabhängig: er ist weder durch fünf Keys garantiert noch gleichbedeutend mit weniger abgerechneten Tokens.

## 2. Repo-Ist-Zustand und Randbedingungen

Im nim-proxy gilt derzeit:

- Eine Lane in `src/pool.rs` entspricht einem NIM-Key mit eigenem RPM-Limit, rollierendem 61-Sekunden-Fenster und Cooldown.
- `Pool::capacity_rpm()` addiert die Limits aktiver Keys. Fünf Keys zu je 40 RPM ergeben theoretisch 200 Requests/Minute, sofern keine zusätzlichen Konto-, Modell- oder Concurrency-Limits greifen.
- Reservierung zählt den Slot beim Grant, damit parallele Requests keinen Key überziehen.
- Sticky Conversation-Affinity wird bevorzugt; wenn die Lane nicht bereit ist, wählt der Pool eine verfügbare, weniger ausgelastete Lane.
- `src/dispatch.rs` vergibt Grants global FIFO und paced sie derzeit mit 25 ms Abstand.
- `src/governor.rs` behandelt modellweite Worker-Erschöpfung separat von einem Key-spezifischen 429.
- Rate-Zustand ist derzeit In-Memory. Mehrere Proxy-Prozesse oder ein Neustart mit demselben Key-Set können die lokale Zählung voneinander entkoppeln; dieses Design darf nicht stillschweigend fünf Keys in mehreren Instanzen duplizieren.
- Conversation-Affinity ist Best-Effort-Cache-Lokalität. NIM-Chat-Anfragen bleiben stateless und senden den Kontext erneut; ein Wechsel der Lane ist korrekt, kann nur einen möglichen Prefix-Cache-Hit kosten.

Der Plan ändert diese Sicherheitsinvarianten nicht. Memory- oder Orchestrator-Fehler müssen entweder kontrolliert ohne Memory weiterarbeiten oder vor dem Upstream-Send sauber fehlschlagen; sie dürfen keine ungezählten Sends erzeugen.

## 3. Zielbild

```text
Agent A ─┐
Agent B ─┼─> Orchestrator / Projekt-Room
Agent C ─┘       │
                 ├─ 1. Projekt-Memory suchen und validieren
                 ├─ 2. Token-budgetierten Kontext mit stabiler Prefixordnung bauen
                 ├─ 3. Provider + Modell + Conversation-Affinity bestimmen
                 └─ 4. nim-proxy: Model Governor -> FIFO Dispatcher -> eine Lane
                                                       ├─ Key 1
                                                       ├─ Key 2
                                                       ├─ Key 3
                                                       ├─ Key 4
                                                       └─ Key 5
Zuständigkeiten
Komponente	Zuständigkeit	Darf nicht
Agent-Orchestrator	Projekt-/Task-Identität, Agentenkoordination, Freigabe von Memory-Deltas	API-Key auswählen oder Limiter umgehen
Memory Store/Room	Provenienz, Versionen, Recall, Komprimierung und Retention	ungeprüfte private Daten global teilen
Context Builder	Relevante Fakten auswählen, Tokenbudget einhalten, Prefix stabilisieren	vollständige Transkripte oder Secret-Material standardmäßig anhängen
nim-proxy Pool	Per-Key RPM, Cooldown, Lane-Affinity und Reservation	Cache-Treffer oder Projekt-Memory als Rate-Limit-Ersatz interpretieren
Model Governor	Modellweite Concurrency kontrollieren	Worker-Exhaustion pauschal als Key-429 behandeln
```

## 4. Kombinierte Memory-Architektur

### 4.1 Datenmodell und Scope

Jede Memory-Einheit erhält mindestens:

```text
memory_id
project_id / room_id
memory_tier: working | episodic | semantic | procedural
kind: decision | fact | task | error | pattern | observation
content (redigiert)
author_type + author_id (human/agent)
source: conversation/task/file/commit + opaque reference
created_at, updated_at, valid_from, valid_until?
confidence / salience / importance
privacy: private | project_shared | explicitly_shared
version, supersedes[], contradicts[]
content_hash (dedup idempotency)
project_id ist ein expliziter Mandantenscope, kein aus beliebigem Prompttext abgeleiteter Name.
room_id gruppiert geteilte Projekterkenntnisse ähnlich Uteke Rooms. Ein Room ist nur eine logische Freigabegrenze; Autorisierung bleibt serverseitig.
Agenten und Tasks bekommen getrennte Identitäten. Jede Erinnerung wird einem schreibenden Agenten/Task zugeordnet.
Persönliche Präferenzen, Secrets und andere Projekte bleiben in separatem privaten Scope. Es gibt keine automatische Hochstufung von Projekt-Memory zu globalem Memory.
```

### 4.2 Vier Tiers mit kontrollierter Konsolidierung

- **Working Memory:** kleine, flüchtige Task-Zusammenfassung und aktuelle Fakten, die sofort für den nächsten Agenten gebraucht werden.
- **Episodic/Session Memory:** Ereignisse, Agentenübergaben, Tests, Fehler und Entscheidungen inklusive Zeit und Quelle. Rohtranskripte nur nach Richtlinie und nicht automatisch für alle Agenten freigeben.
- **Semantic/Project Memory:** deduplizierte, bestätigte Projektfakten, Architekturentscheidungen, Constraints und wiederkehrende Erkenntnisse.
- **Procedural Memory:** geprüfte Arbeitsabläufe, z. B. Build-/Test-/Release-Schritte, mit Versions-/Commit-Bezug.

Konsolidierung läuft asynchron oder bei Session-Ende, nicht im kritischen Upstream-Request-Pfad. Sie darf nur Einträge zusammenfassen, deren Quellen nachvollziehbar sind.

### 4.3 Provenienz, Updates und Widersprüche

- Memory-Schreibvorgänge sind idempotent (Content-Hash plus Scope) und versioniert.
- Agenten schreiben zunächst strukturierte Deltas/Vorschläge. Bestätigte Fakten können nach Policy direkt committen; riskante Entscheidungen erfordern Review oder eine vertrauenswürdige Quelle.
- Bei konkurrierenden Agenten-Updates wird mit Versionsnummer/CAS gearbeitet. Konflikte werden als Konflikte gespeichert, nicht still per Last-Write-Wins überschrieben.
- Eine Korrektur erzeugt eine neue Version und markiert die alte als superseded; historische Rekonstruktion bleibt möglich.
- Confidence-Werte beschreiben Unsicherheit und Evidenz, nicht eine künstliche Objektivität. Kritische Aussagen müssen Quellen/Artefakte haben.
- Retrieval-Ergebnisse aus Memory sind Daten, keine Anweisungen. Sie werden im Context klar abgegrenzt, z. B. als zitierte `<project-memory>`-Sektion, damit gespeicherte Prompt-Injection nicht als Systemregel behandelt wird.
- Secrets (API-Keys, Tokens, .env-Werte) werden vor Einbettung/Indexierung maskiert oder abgewiesen. Niemals Keys, vollständige Prompts oder private Logs als Metriklabel verwenden.

### 4.4 Recall mit token-budgetiertem Context

Vorgeschlagene Anfrage:

```text
recall(project_id, room_id, task_query, agent_role,
      as_of, allowed_tiers, max_items, max_tokens)
```

Vorgehen:

- Scope- und Berechtigungsfilter vor semantischer Suche anwenden.
- Kandidaten über Volltext + semantische Suche sammeln; optional strukturierte Fact-/Graph-Beziehungen ergänzen.
- Kandidaten nach Task-Relevanz, Quelle/Vertrauen, Aktualität, Nutzwert und expliziter Wichtigkeit ordnen. Signale und Gewichte müssen messbar/konfigurierbar sein.
- Deduplizieren; widersprüchliche Fakten zusammen mit Status/Quellen kenntlich machen.
- Tokenbudget hart einhalten. Erst kurze Projektzusammenfassung, dann relevante Entscheidungen/Constraints, dann Task-Episoden/Artefakte.
- Große Quelltexte, Testlogs und Dokumente durch Pfad/Commit/Zeilen-Referenz repräsentieren; nur nötige Ausschnitte nachladen.

## 5. Prompt-Cache und Token-Change

### 5.1 Klar unterscheiden, was eingespart wird

- **Weniger Prompt-Tokens senden:** kurzes Projekt-Memory, gezielte Recall-Auswahl, keine Wiederholung kompletter Agentenverläufe und nur notwendige Tools/Artefakte. Das ist direkt planbar und messbar.
- **Gleiche Präfixe wiederverwenden:** stabiler Promptaufbau kann einen providerseitigen KV-/Prefix-Cache begünstigen und TTFT/Compute senken. Ob gehostetes NIM ihn aktiviert, wie er scoped und abgerechnet wird, muss verifiziert werden.

### 5.2 Stabile Prompt-Struktur

Wenn semantisch und API-seitig möglich, Requests in folgender Reihenfolge aufbauen:

```text
unveränderlicher System-/Sicherheits-Prefix
unveränderte Agentenrolle und stabil sortierte Tool-Schemas
knapper, versionierter Projektsnapshot
task-spezifische Dateien/Ausschnitte
aktuelle Unterhaltung und neue Anfrage
```

Keine zufälligen IDs, aktuelle Uhrzeit, volatile Taskdetails oder dynamisch wechselnde Tool-Reihenfolge vor den stabilen Teilen platzieren. Stabilität darf nie Vorrang vor aktuellen Fakten oder Sicherheitsregeln haben.

### 5.3 Cache-Identität und Lane-Affinity

```text
affinity = HMAC(server_secret,
                provider_id || model_id || project_id || conversation_id)
```

Nicht den Prompt-/Memory-Klartext als Affinity-Key oder Metriklabel nutzen.

## 6. Lane-System für mehrere Keys pro Modell verbessern

- **Kapazitätsmodell:** Fünf Keys mit je 40 RPM → theoretisch 200 Requests/min (~3,33 RPS im Mittel). Real verfügbarer Durchsatz = Minimum aus: per-key aggregiertes RPM, provider-/accountweites Limit, modellweite Worker-Concurrency / effektive Generierungsdauer, Orchestrator- und Client-Demand.

- **Auswahlpolicy:** Affinity-Lane sofort verwenden (enabled, nicht im Cooldown, innerhalb RPM-Fenster); sonst bereite Lanes nach normalisierter RPM-Auslastung (`in_window / rpm`) sortieren; faire Tie-Breaks; Retry-After/Cooldown nur auf passendem Lane; bei nichts bereit → FIFO warten mit Deadlines.

- **Multi-Instance-/Neustart-Sicherheit:** Vor Verteilung auf mehrere Proxy-Instanzen gemeinsamer atomarer Limiter oder exklusive Key-Zuordnung erforderlich.

## 7. Orchestrator-Flow für mehrere Agenten

1. Authentifizieren und autorisierten Projekt-/Room-Scope feststellen.
2. Task-ID, Agentenrolle, Provider/Modell und Conversation-ID bestimmen.
3. Memory im erlaubten Scope abrufen; max. Memory-Tokenbudget anwenden.
4. Kontext mit stabiler Prefix-Reihenfolge bauen; Memory als zitierte Daten markieren.
5. Request an nim-proxy senden; Proxy übernimmt Governor, FIFO-Queue, Lane-Reservation und Streaming.
6. Jeden tatsächlich gesendeten Versuch (Retry/Hedge eingeschlossen) als RPM-Verbrauch zählen.

## 8. Schnittstelle (Entwurf)

**Orchestrator → Memory Service:**
```json
{
  "scope": { "project_id": "opaque", "room_id": "engineering" },
  "agent": { "id": "agent-opaque", "role": "implementation" },
  "task_id": "task-opaque",
  "query": "relevante Frage zum aktuellen Task",
  "memory_version": 42,
  "token_budget": 1200,
  "as_of": null
}
```

**Agent → Memory Delta:**
```json
{
  "scope": { "project_id": "opaque", "room_id": "engineering" },
  "expected_version": 42,
  "author": { "type": "agent", "id": "agent-opaque", "task_id": "task-opaque" },
  "entries": [
    { "kind": "learning", "content": "Der betroffene Test schlägt nur bei ... fehl.", "source_ref": "commit-or-file-reference", "privacy": "project_shared", "status": "proposal" }
  ]
}
```

## 9–15. (siehe vorherige Version des Dokuments: Risiken/Gegenmaßnahmen, Phasenplan, Testmatrix, Metriken, Referenzen — unverändert übernommen)

## 16. Nächste Schritte

- Vorhandenes Lane-/Racing-/Governor-Verhalten im Zielbranch und aktuelle Provider-/Key-Konfiguration prüfen; keine Keys in Artefakte übernehmen.
- Phase-0-Metrik- und enforcing-Mock-Baseline für fünf gleiche Modell-Keys erstellen.
- Lane-Auswahl und Conversation-ID-Affinity unabhängig von Memory verbessern und mit Unit-/Load-Tests absichern.
- Memory-Adapter und eine Room-Isolation als separaten Proof of Concept aufbauen.
- Context Builder mit engem Tokenbudget und Memory-Quellenhinweisen bauen; Input-Tokens pro Task messen.
- Erst danach kontrolliert testen, ob stabile Prefixe und dieselbe Lane tatsächlich einen messbaren NIM-KV-Cache-Effekt zeigen.
- Architektur- und Lizenzreview, Datenschutzreview und unabhängige Prüfung vor produktiver Aktivierung.

## 17. Referenzen und Übertragungsgrenzen

- `bkg-zig/nim-proxy` — vorhandener per-key Sliding Window, FIFO Dispatcher, Conversation-Affinity und separater Model Governor.
- `varun29ankuS/shodh-memory` — Apache-2.0 (Designvorlage).
- `codecoradev/uteke` — Apache-2.0 (Designvorlage).
- `gambletan/cortex` — MIT (Designvorlage).

Lizenzprüfung und Attribution vor Codeübernahme: Lizenzkompatibilität und NOTICE-/Attributionspflichten prüfen; eine Integration als separater Prozess/MCP-Server ist von Kopieren/Portieren von Quellcode zu unterscheiden.
