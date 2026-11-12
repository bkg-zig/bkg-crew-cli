//! HTTP API and Web UI server for bkgcrewcli.
//! Provides REST endpoints to manage/operate agents, models, providers, config and run tasks.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::{get, post},
    Router,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;

use crate::config::{AgentKind, OutputFormat, PermissionMode, TaskConfig};
use crate::models::ModelRegistry;
use crate::plugins::{self, CommonRequestFields, PluginManager, RequestOutcome, RunTracker, UsageRecordedEvent};
use crate::settings::{ProjectConfig, Settings};
use crate::{all_agents, create_runner, run_task_with_cancel};

use super::gateway;

#[derive(Clone)]
pub struct ServerState {
    pub cwd: PathBuf,
    pub project_config: Option<ProjectConfig>,
    pub settings: Settings,
    pub registry: ModelRegistry,
    pub plugins: PluginManager,
    pub port: u16,
}

#[derive(Serialize)]
struct AgentInfo {
    id: String,
    name: String,
    available: bool,
    version: Option<String>,
}

#[derive(Deserialize)]
pub struct RunRequest {
    pub agent: Option<String>,
    pub prompt: String,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub permissions: Option<String>,
}

#[derive(Serialize)]
struct RunResponse {
    ok: bool,
    exit_code: i32,
    output: String,
    events: Vec<Value>,
    error: Option<String>,
}

pub fn create_base_router(state: Arc<ServerState>) -> Router {
    // Serve the WebUI when a dist can be located. Checked in order:
    // cwd, next to the executable, an installed share path and the central
    // data directory — so the console works no matter where serve was started.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let candidates: Vec<PathBuf> = [
        Some(state.cwd.join("webui").join("dist")),
        exe_dir.as_ref().map(|d| d.join("webui").join("dist")),
        exe_dir.as_ref().map(|d| d.join("share").join("bkgcrewcli").join("webui").join("dist")),
        Some(PathBuf::from("/srv/bkg/data/bkgcrewcli/webui/dist")),
        dirs::home_dir().map(|h| h.join(".bkgcrewcli").join("webui").join("dist")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let web_dist = candidates.into_iter().find(|c| c.join("index.html").is_file());
    let mut app = Router::new()
        .route("/api/health", get(health))
        .route("/api/agents", get(list_agents))
        .route("/api/models", get(list_models))
        .route("/api/config", get(get_config))
        .route("/api/run", post(run_agent))
        .with_state(state);
    match web_dist {
        Some(dist) => {
            // NOTE: use fallback_service (not nest_service) so no catch-all tail
            // route is registered — parametrized routes like /:provider/v1/* must
            // stay conflict-free in the merged router.
            app = app.fallback_service(tower_http::services::ServeDir::new(dist).not_found_service(get(fallback_ui)));
        }
        None => app = app.fallback(get(fallback_ui)),
    }
    app
}

pub fn create_router(state: ServerState) -> Router {
    // legacy simple router (no gateway merge); prefer create_full_router
    create_base_router(Arc::new(state)).layer(CorsLayer::permissive())
}

pub fn create_full_router(base: Arc<ServerState>, gw: gateway::GatewayState) -> Router {
    let base_router = create_base_router(base);
    let gw_router = gateway::gateway_router(gw);
    base_router.merge(gw_router).layer(CorsLayer::permissive())
}

async fn health() -> Json<Value> {
    Json(json!({"ok": true, "service": "bkgcrewcli-webui"}))
}

async fn list_agents(State(_st): State<Arc<ServerState>>) -> Json<Value> {
    let mut infos: Vec<AgentInfo> = Vec::new();
    for kind in all_agents() {
        let runner = create_runner(kind);
        let available = runner.is_available();
        let version = if available {
            let tmp = TaskConfig::new("", kind);
            runner.version(&tmp)
        } else {
            None
        };
        infos.push(AgentInfo {
            id: match kind { crate::config::AgentKind::Claude=>"claude", crate::config::AgentKind::OpenCode=>"opencode", crate::config::AgentKind::Codex=>"codex", crate::config::AgentKind::Cursor=>"cursor", crate::config::AgentKind::Pi=>"pi", crate::config::AgentKind::Copilot=>"copilot" }.to_string(),
            name: kind.display_name().to_string(),
            available,
            version,
        });
    }
    Json(json!({"agents": infos}))
}

#[derive(Deserialize)]
struct ModelsQuery {
    agent: Option<String>,
}

async fn list_models(
    State(st): State<Arc<ServerState>>,
    Query(q): Query<ModelsQuery>,
) -> Json<Value> {
    let mut out: Vec<Value> = Vec::new();
    for (id, m) in &st.registry.models {
        let mut v = json!({
            "id": id,
            "description": m.description,
            "provider": m.provider,
        });
        if let Some(a) = &q.agent {
            if let Ok(kind) = a.parse::<AgentKind>() {
                let res = st.registry.resolve(id, kind);
                match res {
                    crate::models::ModelResolution::Resolved { agent_id, canonical_name } => {
                        v["resolved"] = json!({"id": agent_id, "canonical": canonical_name, "source": "resolved"});
                    }
                    crate::models::ModelResolution::NoAgentMapping { canonical_name } => {
                        v["resolved"] = json!({"id": canonical_name, "source": "no_mapping"});
                    }
                    crate::models::ModelResolution::Passthrough { raw } => {
                        v["resolved"] = json!({"id": raw, "source": "passthrough"});
                    }
                }
            }
        }
        out.push(v);
    }
    out.sort_by(|a,b| a.get("id").and_then(|x| x.as_str()).cmp(&b.get("id").and_then(|x| x.as_str())));
    Json(json!({"models": out}))
}

async fn get_config(State(st): State<Arc<ServerState>>) -> Json<Value> {
    Json(json!({
        "cwd": st.cwd.to_string_lossy(),
        "default_agent": st.project_config.as_ref().and_then(|c| c.default_agent.clone()).or_else(|| st.settings.default_agent.clone()),
        "default_model": st.project_config.as_ref().and_then(|c| c.default_model.clone()).or_else(|| st.settings.default_model.clone()),
        "has_project_config": st.project_config.is_some(),
    }))
}

async fn run_agent(
    State(st): State<Arc<ServerState>>,
    Json(req): Json<RunRequest>,
) -> impl IntoResponse {
    let agent_id = req.agent.clone().unwrap_or_else(|| "claude".to_string());
    let request_id = plugins::new_request_id();
    let common = CommonRequestFields {
        id: request_id.clone(),
        trace_id: None,
        user_agent: "bkgcrewcli-webui".to_string(),
        session_id: None,
        path: "/api/run".to_string(),
        port: st.port,
        started_at: plugins::now_ms(),
        model: req.model.clone(),
        actual_model: None,
        agent: Some(agent_id.clone()),
        cwd: req.cwd.clone().or_else(|| Some(st.cwd.to_string_lossy().to_string())),
    };
    let mut tracker = RunTracker::new(st.plugins.clone(), common);
    if req.model.as_ref().map(|m| !m.trim().is_empty()).unwrap_or(false) {
        tracker.set_model(req.model.clone());
    }

    let Ok(kind) = agent_id.parse::<AgentKind>() else {
        let msg = format!("unknown agent: {agent_id}");
        tracker.finish(400, RequestOutcome::Error, Some(msg.clone()), None);
        return (
            StatusCode::BAD_REQUEST,
            Json(RunResponse { ok: false, exit_code: 2, output: String::new(), events: vec![], error: Some(msg) }),
        );
    };
    if req.prompt.trim().is_empty() {
        tracker.finish(400, RequestOutcome::Error, Some("prompt is required".into()), None);
        return (
            StatusCode::BAD_REQUEST,
            Json(RunResponse { ok: false, exit_code: 2, output: String::new(), events: vec![], error: Some("prompt is required".into()) }),
        );
    }

    let cwd = req.cwd.as_ref().map(PathBuf::from).unwrap_or_else(|| st.cwd.clone());
    let permissions = match req.permissions.as_deref().unwrap_or("full-access") {
        "read-only" | "readonly" => PermissionMode::ReadOnly,
        _ => PermissionMode::FullAccess,
    };

    let mut config = TaskConfig::new(req.prompt, kind);
    config.cwd = Some(cwd);
    config.permission_mode = permissions;
    config.output_format = OutputFormat::StreamJson;
    if let Some(m) = req.model.clone() {
        let mt = m.trim();
        if !mt.is_empty() {
            config.model = Some(mt.to_string());
            tracker.set_actual_model(Some(mt.to_string()));
        }
    }

    let handle = match run_task_with_cancel(&config, None).await {
        Ok(h) => h,
        Err(e) => {
            let msg = e.to_string();
            tracker.finish(500, RequestOutcome::Error, Some(msg.clone()), None);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RunResponse { ok: false, exit_code: 1, output: String::new(), events: vec![], error: Some(msg) }),
            );
        }
    };

    let mut stream = handle.stream;
    let mut events: Vec<Value> = Vec::new();
    let mut text = String::new();
    let mut ok = true;
    let mut err_msg: Option<String> = None;
    let mut usage_in: Option<u64> = None;
    let mut usage_out: Option<u64> = None;
    let mut usage_cache_r: Option<u64> = None;
    let mut usage_cache_w: Option<u64> = None;
    let mut cost_usd: Option<f64> = None;

    while let Some(item) = stream.next().await {
        match item {
            Ok(ev) => {
                if let Ok(v) = serde_json::to_value(&ev) {
                    let et = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
                    match et {
                        "session_start" => {
                            if let Some(sid) = v.get("session_id").and_then(|x| x.as_str()) {
                                tracker.set_session(Some(sid.to_string()));
                            }
                            if let Some(m) = v.get("model").and_then(|x| x.as_str()) {
                                tracker.set_actual_model(Some(m.to_string()));
                            }
                        }
                        "text_delta" => {
                            tracker.mark_first_token();
                            if let Some(t) = v.get("text").and_then(|x| x.as_str()) { text.push_str(t); }
                        }
                        "message" => {
                            if v.get("role").and_then(|x| x.as_str()) == Some("assistant") {
                                tracker.mark_first_token();
                                if let Some(t) = v.get("text").and_then(|x| x.as_str()) {
                                    if text.is_empty() { text.push_str(t); }
                                }
                            }
                        }
                        "tool_start" => { tracker.mark_first_token(); }
                        "usage_delta" => {
                            if let Some(u) = v.get("usage") {
                                usage_in = u.get("input_tokens").and_then(|x| x.as_u64()).or(usage_in);
                                usage_out = u.get("output_tokens").and_then(|x| x.as_u64()).or(usage_out);
                                usage_cache_r = u.get("cache_read_tokens").and_then(|x| x.as_u64()).or(usage_cache_r);
                                usage_cache_w = u.get("cache_creation_tokens").and_then(|x| x.as_u64()).or(usage_cache_w);
                                cost_usd = u.get("cost_usd").and_then(|x| x.as_f64()).or(cost_usd);
                            }
                        }
                        "result" => {
                            ok = v.get("success").and_then(|x| x.as_bool()).unwrap_or(ok);
                            if let Some(sid) = v.get("session_id").and_then(|x| x.as_str()) { tracker.set_session(Some(sid.to_string())); }
                            if text.is_empty() { if let Some(rt) = v.get("text").and_then(|x| x.as_str()) { text.push_str(rt); } }
                            if let Some(u) = v.get("usage") {
                                usage_in = u.get("input_tokens").and_then(|x| x.as_u64()).or(usage_in);
                                usage_out = u.get("output_tokens").and_then(|x| x.as_u64()).or(usage_out);
                                usage_cache_r = u.get("cache_read_tokens").and_then(|x| x.as_u64()).or(usage_cache_r);
                                usage_cache_w = u.get("cache_creation_tokens").and_then(|x| x.as_u64()).or(usage_cache_w);
                                cost_usd = u.get("cost_usd").and_then(|x| x.as_f64()).or(cost_usd);
                            }
                            cost_usd = v.get("total_cost_usd").and_then(|x| x.as_f64()).or(cost_usd);
                        }
                        "error" => {
                            ok = false;
                            err_msg = v.get("message").and_then(|x| x.as_str()).map(|s| s.to_string());
                        }
                        _ => {}
                    }
                    events.push(v);
                }
            }
            Err(e) => { ok = false; err_msg = Some(e.to_string()); }
        }
    }

    let total_tokens = match (usage_in, usage_out) {
        (Some(i), Some(o)) => Some(i + o),
        (Some(i), None) => Some(i),
        (None, Some(o)) => Some(o),
        _ => None,
    };
    let usage_ev = if usage_in.is_some() || usage_out.is_some() || cost_usd.is_some() {
        Some(UsageRecordedEvent {
            request_id: request_id.clone(),
            model: tracker.common.model.clone(),
            actual_model: tracker.common.actual_model.clone(),
            user_agent: tracker.common.user_agent.clone(),
            session_id: tracker.common.session_id.clone(),
            agent: tracker.common.agent.clone(),
            input_tokens: usage_in,
            output_tokens: usage_out,
            cache_read_tokens: usage_cache_r,
            cache_creation_tokens: usage_cache_w,
            total_tokens,
            cost_usd,
        })
    } else { None };

    let status = if ok { 200 } else { 500 };
    let outcome = if ok { RequestOutcome::Completed } else { RequestOutcome::Error };
    let err_clone = err_msg.clone();
    tracker.finish(status, outcome, err_clone.clone(), usage_ev);

    (
        StatusCode::OK,
        Json(RunResponse { ok, exit_code: if ok { 0 } else { 1 }, output: text, events, error: if ok { None } else { err_msg } }),
    )
}

async fn fallback_ui() -> Html<&'static str> {
    Html(r##"<!doctype html>
<html lang="de"><head><meta charset="utf-8"/><meta name="viewport" content="width=device-width,initial-scale=1"/>
<title>bkgcrewcli Web UI</title>
<style>
:root{--bg:#050505;--fg:#a8f0ff;--cyan:#00ffcc;--mag:#ff00ff;--lime:#b5ff2b;--yellow:#ffff00;--orange:#ff8800;--blue:#4dc3ff;--violet:#b98cff;--red:#ff4d6d;--pist:#9dff9a;--gold:#ffc94d;--body:#a8f0ff}
*{box-sizing:border-box}
html,body{margin:0;min-height:100%;background:var(--bg);color:var(--fg);font-family:system-ui,-apple-system,Segoe UI,Roboto,sans-serif;scrollbar-width:none}
html::-webkit-scrollbar,body::-webkit-scrollbar,*::-webkit-scrollbar{display:none;width:0;height:0}
body{background-image:linear-gradient(rgba(0,255,204,.015) 1px,transparent 1px),linear-gradient(90deg,rgba(0,255,204,.015) 1px,transparent 1px);background-size:50px 50px}
body::after{content:"";position:fixed;inset:0;pointer-events:none;z-index:9999;background:repeating-linear-gradient(0deg,transparent,transparent 2px,rgba(0,0,0,.03) 2px,rgba(0,0,0,.03) 4px)}
.shell{max-width:1280px;margin:0 auto;padding:14px}
.top{position:sticky;top:0;z-index:20;display:flex;align-items:center;justify-content:space-between;gap:12px;padding:10px 14px;margin-bottom:12px;background:linear-gradient(90deg,rgba(0,255,204,.10),#07110f 60%);border:1px solid rgba(0,255,204,.28);border-radius:.75rem;box-shadow:0 0 0 1px #000,0 12px 30px -20px rgba(0,255,204,.45)}
.brand{font-family:ui-monospace,monospace;font-weight:700;color:var(--cyan);text-shadow:0 0 14px rgba(0,255,204,.45)}
.mut{color:var(--pist);font-size:.72rem}
.auth{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
.auth input{width:240px;max-width:38vw;background:rgba(0,255,204,.03);color:var(--body);border:1px solid rgba(185,140,255,.4);border-radius:.45rem;padding:6px 8px}
.badge{font-family:ui-monospace,monospace;font-size:.7rem;padding:3px 9px;border-radius:999px;border:1px solid}
.badge.ok{color:var(--lime);border-color:rgba(181,255,43,.5);background:rgba(181,255,43,.1)}.badge.no{color:var(--orange);border-color:rgba(255,136,0,.45);background:rgba(255,136,0,.08)}
.btn{display:inline-flex;align-items:center;border:1px solid color-mix(in srgb,currentColor 45%,transparent);border-radius:.5rem;padding:.38rem .75rem;background:color-mix(in srgb,currentColor 6%,transparent);font-family:ui-monospace,monospace;font-size:.75rem;color:var(--cyan);cursor:pointer}
.btn:hover{border-color:currentColor;background:color-mix(in srgb,currentColor 14%,transparent)}.btn.lime{color:var(--lime)}.btn.mag{color:var(--mag)}.btn.violet{color:var(--violet)}.btn.orange{color:var(--orange)}.btn.active{background:color-mix(in srgb,currentColor 18%,transparent);font-weight:600}.btn.locked{opacity:.4;cursor:not-allowed}
.tabs{display:flex;gap:8px;flex-wrap:wrap;margin:4px 0 12px}
.grid{display:grid;grid-template-columns:1fr 1fr;gap:14px}
.app{--app:var(--cyan);display:flex;flex-direction:column;border:1px solid color-mix(in srgb,var(--app) 38%,transparent);border-radius:.75rem;overflow:hidden;background:radial-gradient(900px 260px at 0 0,color-mix(in srgb,var(--app) 8%,transparent),transparent 70%),repeating-linear-gradient(0deg,rgba(0,255,204,.02) 0 1px,transparent 1px 3px),#050a09;box-shadow:0 0 0 1px #000,0 18px 50px -24px color-mix(in srgb,var(--app) 55%,transparent)}
.app.mag{--app:var(--mag)}.app.lime{--app:var(--lime)}.app.violet{--app:var(--violet)}
.app-bar{display:flex;align-items:center;justify-content:space-between;gap:10px;flex-wrap:wrap;padding:.65rem .95rem;background:linear-gradient(90deg,color-mix(in srgb,var(--app) 16%,#07110f),#07110f 65%);border-bottom:1px solid color-mix(in srgb,var(--app) 32%,transparent)}
.app-title{font-family:ui-monospace,monospace;font-weight:700;color:var(--app);text-shadow:0 0 14px color-mix(in srgb,var(--app) 55%,transparent)}
.app-body{padding:14px}.app-status{display:flex;flex-wrap:wrap;gap:.35rem 1rem;padding:.4rem .95rem;font-size:.7rem;color:var(--blue);font-family:ui-monospace,monospace;background:#07110f;border-top:1px solid color-mix(in srgb,var(--app) 22%,transparent)}
label{display:block;font-size:.7rem;color:var(--violet);margin:.55rem 0 .25rem;text-transform:uppercase;letter-spacing:.05em}
select,input,textarea{width:100%;padding:.55rem .7rem;border-radius:.45rem;background:rgba(0,255,204,.03);color:var(--body);border:1px solid rgba(185,140,255,.4);font:inherit;font-size:.85rem}
textarea{min-height:140px;resize:vertical}input:focus,select:focus,textarea:focus{outline:none;border-color:var(--yellow);box-shadow:0 0 0 3px rgba(255,255,0,.12)}
pre{margin:0;padding:.85rem 1rem;background:#0a0a0a;color:#cfefff;border:1px solid rgba(0,255,204,.18);border-radius:.5rem;overflow:auto;max-height:420px;white-space:pre-wrap;word-break:break-word;font-size:.78rem;line-height:1.55;font-family:ui-monospace,monospace}
.pill{display:inline-block;padding:2px 9px;border-radius:999px;font-size:.72rem;margin:2px 5px 2px 0;font-family:ui-monospace,monospace;border:1px solid}.pill.ok{color:var(--lime);border-color:rgba(181,255,43,.45);background:rgba(181,255,43,.1)}.pill.bad{color:var(--red);border-color:rgba(255,77,109,.45);background:rgba(255,77,109,.1)}
.notice{padding:10px 12px;border:1px dashed rgba(255,136,0,.45);border-radius:8px;background:rgba(255,136,0,.07);color:var(--gold);margin-bottom:12px;font-size:.85rem}
.row{display:flex;gap:8px;flex-wrap:wrap}.row>*{flex:1 1 140px}.hidden{display:none}.stack{display:flex;flex-direction:column;gap:12px}
@media(max-width:900px){.grid{grid-template-columns:1fr}.auth input{width:170px}}
</style></head><body>
<div class="shell">
  <div class="top">
    <div><div class="brand">bkgcrewcli</div><div class="mut">native gateway · agents · <span id="gwState">lädt…</span></div></div>
    <div class="auth">
      <span id="authBadge" class="badge no">Admin gesperrt</span>
      <input id="adminToken" type="password" placeholder="Admin-Token"/>
      <button class="btn lime" onclick="unlockAdmin()">Entsperren</button>
      <button class="btn orange" onclick="lockAdmin()">Sperren</button>
    </div>
  </div>
  <div class="tabs">
    <button class="btn active" data-tab="overview" onclick="showTab('overview')">Übersicht</button>
    <button class="btn mag" data-tab="run" onclick="showTab('run')">Run</button>
    <button class="btn lime locked" id="tab-gateway" data-tab="gateway" onclick="showTab('gateway')">Gateway 🔒</button>
    <button class="btn violet locked" id="tab-admin" data-tab="admin" onclick="showTab('admin')">Admin/Config 🔒</button>
  </div>

  <div id="view-overview"><div class="grid">
    <div class="app"><div class="app-bar"><span class="app-title">Agenten & Status</span><button class="btn" onclick="loadAgents()">Aktualisieren</button></div>
      <div class="app-body"><div id="agents">Lade…</div></div><div class="app-status">API: /api/agents · /api/run · /api/config</div></div>
    <div class="app violet"><div class="app-bar"><span class="app-title">System / Gateway</span><button class="btn violet" onclick="loadGatewayInfo()">Aktualisieren</button></div>
      <div class="app-body stack"><div><label>App Config</label><pre id="cfg">Lade…</pre></div><div><label>Gateway Status</label><pre id="gwcfg">Lade…</pre></div></div>
      <div class="app-status">Gateway: /v1/models · /v1/chat/completions · /v1/messages · /usage</div></div>
  </div></div>

  <div id="view-run" class="hidden"><div class="app mag"><div class="app-bar"><span class="app-title">Agent ausführen</span></div><div class="app-body">
    <div class="grid"><div>
      <label>Agent</label><select id="agent"><option>claude</option><option>opencode</option><option>codex</option><option>cursor</option><option>pi</option><option>copilot</option></select>
      <label>Modell (optional)</label><input id="model" placeholder="z. B. apieysho/code"/>
      <label>Arbeitsverzeichnis (optional)</label><input id="cwd" placeholder="leer = Server-CWD"/>
    </div><div>
      <label>Prompt</label><textarea id="prompt" style="min-height:180px" placeholder="Aufgabe/Prompt an den Agenten…"></textarea>
      <div style="margin-top:10px"><button id="runBtn" class="btn mag" onclick="runAgent()">Starten</button></div>
    </div></div>
    <label>Ergebnis</label><pre id="out">(noch kein Lauf)</pre>
  </div><div class="app-status">POST /api/run</div></div></div>

  <div id="view-gateway" class="hidden"><div id="gwLock" class="notice">Admin-Token oben rechts eingeben und „Entsperren“, um Gateway-Menüs zu öffnen.</div>
    <div id="gwBody" class="grid hidden">
      <div class="app lime"><div class="app-bar"><span class="app-title">Modelle & Usage</span></div><div class="app-body stack">
        <div class="row"><button class="btn lime" onclick="loadModels()">/v1/models laden</button><button class="btn lime" onclick="loadUsage()">/usage laden</button></div>
        <div><label>/v1/models</label><pre id="v1models">(nicht geladen)</pre></div>
        <div><label>/usage</label><pre id="usage">(nicht geladen)</pre></div>
      </div></div>
      <div class="app lime"><div class="app-bar"><span class="app-title">Gateway Status</span><button class="btn lime" onclick="loadGatewayInfo()">Aktualisieren</button></div>
        <div class="app-body"><pre id="gwinfo">(nicht geladen)</pre></div></div>
    </div></div>

  <div id="view-admin" class="hidden"><div id="admLock" class="notice">Admin-Token oben rechts eingeben und „Entsperren“, um Admin/Config zu bearbeiten.</div>
    <div id="admBody" class="hidden"><div class="grid">
      <div class="app violet"><div class="app-bar"><span class="app-title">Gateway Config (Admin)</span></div><div class="app-body stack">
        <div class="row"><button class="btn violet" onclick="loadAdminConfig()">Laden</button><button class="btn violet" onclick="saveAdminConfig()">Speichern</button></div>
        <label>gateway.json</label><textarea id="adminCfg" style="min-height:320px;font-family:ui-monospace,monospace;font-size:.78rem"></textarea>
        <div class="mut" id="adminMsg"></div>
      </div></div>
      <div class="app violet"><div class="app-bar"><span class="app-title">Model-Mappings (Admin)</span></div><div class="app-body stack">
        <div class="row"><button class="btn violet" onclick="loadMappings()">Laden</button><button class="btn violet" onclick="saveMappings()">Speichern</button></div>
        <label>JSON-Objekt { "source": "target" }</label><textarea id="mappings" style="min-height:240px;font-family:ui-monospace,monospace;font-size:.78rem"></textarea>
        <div class="mut">Defaults (codex-auto-review, gpt-reserve) werden gemerged.</div>
        <div class="mut" id="mapMsg"></div>
      </div></div>
    </div></div>
</div>
<script>
let adminToken=sessionStorage.getItem('bkg_admin_token')||'';
function authH(json){const h={}; if(json) h['Content-Type']='application/json'; if(adminToken) h['x-api-key']=adminToken; return h;}
function setAdminUI(ok){const b=document.getElementById('authBadge'); b.textContent=ok?'Admin entsperrt':'Admin gesperrt'; b.className='badge '+(ok?'ok':'no');
 document.getElementById('gwLock').classList.toggle('hidden',ok); document.getElementById('gwBody').classList.toggle('hidden',!ok);
 document.getElementById('admLock').classList.toggle('hidden',ok); document.getElementById('admBody').classList.toggle('hidden',!ok);
 const tg=document.getElementById('tab-gateway'), ta=document.getElementById('tab-admin');
 tg.classList.toggle('locked',!ok); ta.classList.toggle('locked',!ok);
 tg.textContent=ok?'Gateway':'Gateway 🔒'; ta.textContent=ok?'Admin/Config':'Admin/Config 🔒';
}
async function unlockAdmin(){const t=document.getElementById('adminToken').value.trim(); if(!t){setAdminUI(false);return;}
 try{const r=await fetch('/admin/config/model-mappings',{headers:{'x-api-key':t}});
  if(r.ok){adminToken=t; sessionStorage.setItem('bkg_admin_token',t); setAdminUI(true); loadAdminConfig(); loadMappings(); loadGatewayInfo();}
  else{adminToken=''; sessionStorage.removeItem('bkg_admin_token'); setAdminUI(false); alert('Admin-Token ungültig (HTTP '+r.status+')');}
 }catch(e){alert('Prüfung fehlgeschlagen: '+e);}
}
function lockAdmin(){adminToken=''; sessionStorage.removeItem('bkg_admin_token'); document.getElementById('adminToken').value=''; setAdminUI(false); showTab('overview');}
function showTab(name){const need=(name==='gateway'||name==='admin'); if(need && !adminToken){alert('Bitte zuerst Admin-Token oben rechts eingeben.'); return;}
 document.querySelectorAll('.tabs .btn').forEach(b=>b.classList.toggle('active', b.dataset.tab===name));
 ['overview','run','gateway','admin'].forEach(v=>document.getElementById('view-'+v).classList.toggle('hidden', v!==name));}
async function loadAgents(){try{const r=await fetch('/api/agents'); const d=await r.json();
 document.getElementById('agents').innerHTML=(d.agents||[]).map(a=>`<span class="pill ${a.available?'ok':'bad'}">${a.id}${a.available?' ✓':' ✗'}${a.version?(' · '+a.version):''}</span>`).join('')||'keine';}catch(e){document.getElementById('agents').textContent='Fehler: '+e;}}
async function loadCfg(){try{const r=await fetch('/api/config'); document.getElementById('cfg').textContent=JSON.stringify(await r.json(),null,2);}catch(e){document.getElementById('cfg').textContent=''+e;}}
async function loadGatewayInfo(){try{const r=await fetch('/api/gateway/config'); const d=await r.json();
 document.getElementById('gwcfg').textContent=JSON.stringify(d,null,2); document.getElementById('gwinfo').textContent=JSON.stringify(d,null,2);
 document.getElementById('gwState').textContent=(d.keystoreAvailable?'Keystore OK':'Keystore n/a')+' · '+(d.providersConfigured||0)+' Provider';}catch(e){document.getElementById('gwcfg').textContent=''+e;}}
async function loadModels(){try{const r=await fetch('/v1/models'); document.getElementById('v1models').textContent=JSON.stringify(await r.json(),null,2);}catch(e){document.getElementById('v1models').textContent=''+e;}}
async function loadUsage(){try{const r=await fetch('/usage'); document.getElementById('usage').textContent=JSON.stringify(await r.json(),null,2);}catch(e){document.getElementById('usage').textContent=''+e;}}
async function runAgent(){const btn=document.getElementById('runBtn'); btn.disabled=true; document.getElementById('out').textContent='Läuft…';
 const body={agent:document.getElementById('agent').value,prompt:document.getElementById('prompt').value,model:document.getElementById('model').value||null,cwd:document.getElementById('cwd').value||null};
 try{const r=await fetch('/api/run',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}); const d=await r.json();
  document.getElementById('out').textContent=(d.output&&String(d.output).trim())?d.output:(d.error||JSON.stringify(d,null,2));}catch(e){document.getElementById('out').textContent='Fehler: '+e;} finally{btn.disabled=false;}}
async function loadAdminConfig(){if(!adminToken)return; try{const r=await fetch('/admin/config',{headers:authH(false)}); const d=await r.json(); if(!r.ok) throw new Error(JSON.stringify(d)); document.getElementById('adminCfg').value=JSON.stringify(d,null,2); document.getElementById('adminMsg').textContent='Config geladen (sensible Werte maskiert).';}catch(e){document.getElementById('adminMsg').textContent='Fehler: '+e;}}
async function saveAdminConfig(){if(!adminToken)return; try{const body=JSON.parse(document.getElementById('adminCfg').value); const r=await fetch('/admin/config',{method:'POST',headers:authH(true),body:JSON.stringify(body)}); const d=await r.json(); if(!r.ok) throw new Error(JSON.stringify(d)); document.getElementById('adminMsg').textContent='Gespeichert.'; loadGatewayInfo();}catch(e){document.getElementById('adminMsg').textContent='Fehler: '+e;}}
async function loadMappings(){if(!adminToken)return; try{const r=await fetch('/admin/config/model-mappings',{headers:authH(false)}); const d=await r.json(); if(!r.ok) throw new Error(JSON.stringify(d)); document.getElementById('mappings').value=JSON.stringify(d,null,2); document.getElementById('mapMsg').textContent='Mappings geladen.';}catch(e){document.getElementById('mapMsg').textContent='Fehler: '+e;}}
async function saveMappings(){if(!adminToken)return; try{const body=JSON.parse(document.getElementById('mappings').value); const r=await fetch('/admin/config/model-mappings',{method:'POST',headers:authH(true),body:JSON.stringify(body)}); const d=await r.json(); if(!r.ok) throw new Error(JSON.stringify(d)); document.getElementById('mappings').value=JSON.stringify(d,null,2); document.getElementById('mapMsg').textContent='Mappings gespeichert.';}catch(e){document.getElementById('mapMsg').textContent='Fehler: '+e;}}
document.getElementById('adminToken').value=adminToken; if(adminToken){unlockAdmin();} else {setAdminUI(false);}
loadAgents(); loadCfg(); loadGatewayInfo();
</script></body></html>"##)
}

