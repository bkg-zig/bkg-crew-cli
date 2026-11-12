#![allow(non_snake_case)]

use dioxus::prelude::*;
use gloo_net::http::Request;
use serde::Deserialize;
use serde_json::{json, Value};

const STYLE: Asset = asset!("/assets/style.css");

fn main() {
    launch(App);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Models,
    Metrics,
    Run,
    Gateway,
    Admin,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct AgentInfo {
    #[serde(default)] id: String,
    #[serde(default)] available: bool,
    #[serde(default)] version: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize)]
struct AgentsResp { #[serde(default)] agents: Vec<AgentInfo> }

async fn fetch_text(method: &str, url: &str, token: Option<String>, body: Option<Value>) -> Result<String, String> {
    let mut req = match method { "POST" => Request::post(url), _ => Request::get(url) };
    if let Some(t) = token { if !t.is_empty() { req = req.header("x-api-key", &t); } }
    let resp = if let Some(b) = body {
        req.header("Content-Type", "application/json").json(&b).map_err(|e| e.to_string())?.send().await.map_err(|e| e.to_string())?
    } else { req.send().await.map_err(|e| e.to_string())? };
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if (200..300).contains(&status) { Ok(text) } else { Err(format!("HTTP {status}: {text}")) }
}
async fn fetch_json(url: &str, token: Option<String>) -> Value {
    match fetch_text("GET", url, token, None).await {
        Ok(t) => serde_json::from_str::<Value>(&t).unwrap_or(Value::String(t)),
        Err(e) => json!({"error": e}),
    }
}
async fn pretty(url: &str, token: Option<String>) -> String {
    let v = fetch_json(url, token).await;
    serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string())
}
fn get_stored_token() -> String {
    web_sys::window().and_then(|w| w.session_storage().ok().flatten()).and_then(|s| s.get_item("bkg_admin_token").ok().flatten()).unwrap_or_default()
}
fn set_stored_token(token: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.session_storage().ok().flatten()) {
        if token.is_empty() { let _ = storage.remove_item("bkg_admin_token"); } else { let _ = storage.set_item("bkg_admin_token", token); }
    }
}
fn s(v: &Value, k: &str) -> String { v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string() }
fn i64v(v: &Value, k: &str) -> i64 { v.get(k).and_then(|x| x.as_i64()).or_else(|| v.get(k).and_then(|x| x.as_u64()).map(|u| u as i64)).unwrap_or(0) }
fn f64v(v: &Value, k: &str) -> f64 { v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) }

#[component]
fn App() -> Element {
    let mut tab = use_signal(|| Tab::Overview);
    let mut admin_token = use_signal(get_stored_token);
    let mut token_input = use_signal(get_stored_token);
    let mut admin_unlocked = use_signal(|| false);
    let mut auth_msg = use_signal(|| String::new());

    let mut agents = use_signal(|| Vec::<AgentInfo>::new());
    let mut agents_err = use_signal(|| String::new());
    let mut cfg_json = use_signal(|| Value::Null);
    let mut gw_json = use_signal(|| Value::Null);
    let mut gw_state = use_signal(|| "lädt…".to_string());

    let mut models_json = use_signal(|| Value::Null);
    let mut metrics_json = use_signal(|| Value::Null);
    let mut providers_json = use_signal(|| Value::Null);
    let mut usage_json = use_signal(|| Value::Null);

    let mut admin_cfg_text = use_signal(|| String::new());
    let mut mappings_text = use_signal(|| String::new());
    let mut admin_msg = use_signal(|| String::new());
    let mut map_msg = use_signal(|| String::new());

    let mut run_agent = use_signal(|| "claude".to_string());
    let mut run_model = use_signal(|| String::new());
    let mut run_cwd = use_signal(|| String::new());
    let mut run_prompt = use_signal(|| String::new());
    let mut run_out = use_signal(|| "(noch kein Lauf)".to_string());
    let mut run_busy = use_signal(|| false);

    let refresh_agents = move || spawn(async move {
        match fetch_text("GET", "/api/agents", None, None).await {
            Ok(t) => match serde_json::from_str::<AgentsResp>(&t) {
                Ok(v) => { agents.set(v.agents); agents_err.set(String::new()); }
                Err(_) => { agents_err.set(t); agents.set(Vec::new()); }
            },
            Err(e) => { agents_err.set(e); agents.set(Vec::new()); }
        }
    });
    let refresh_cfg = move || spawn(async move {
        cfg_json.set(fetch_json("/api/config", None).await);
        let gw = fetch_json("/api/gateway/config", None).await;
        let ks = gw.get("keystoreAvailable").and_then(|x| x.as_bool()).unwrap_or(false);
        let n = gw.get("providersConfigured").and_then(|x| x.as_i64()).unwrap_or(0);
        gw_state.set(format!("{} · {} Provider", if ks { "Keystore OK" } else { "Keystore n/a" }, n));
        gw_json.set(gw);
    });
    let refresh_models = move || spawn(async move { models_json.set(fetch_json("/v1/models", None).await); });
    let refresh_metrics = move || spawn(async move {
        let t = admin_token();
        let tok = if t.is_empty() { None } else { Some(t) };
        metrics_json.set(fetch_json("/api/metrics", tok.clone()).await);
        providers_json.set(fetch_json("/api/providers", tok).await);
    });
    let refresh_usage = move || spawn(async move { usage_json.set(fetch_json("/usage?limit=50", None).await); });

    use_effect(move || {
        refresh_agents();
        refresh_cfg();
        refresh_models();
        let t = admin_token();
        if !t.is_empty() {
            spawn(async move {
                let t2 = admin_token();
                if fetch_text("GET", "/admin/config/model-mappings", Some(t2.clone()), None).await.is_ok() {
                    admin_unlocked.set(true);
                    admin_cfg_text.set(pretty("/admin/config", Some(t2.clone())).await);
                    mappings_text.set(pretty("/admin/config/model-mappings", Some(t2.clone())).await);
                    metrics_json.set(fetch_json("/api/metrics", Some(t2.clone())).await);
                    providers_json.set(fetch_json("/api/providers", Some(t2)).await);
                } else { admin_unlocked.set(false); admin_token.set(String::new()); set_stored_token(""); }
            });
        }
    });

    let do_unlock = move |_: MouseEvent| spawn(async move {
        let t = token_input().trim().to_string();
        if t.is_empty() { auth_msg.set("Token fehlt".into()); return; }
        match fetch_text("GET", "/admin/config/model-mappings", Some(t.clone()), None).await {
            Ok(_) => {
                admin_token.set(t.clone()); set_stored_token(&t); admin_unlocked.set(true);
                auth_msg.set("Admin entsperrt".into());
                admin_cfg_text.set(pretty("/admin/config", Some(t.clone())).await);
                mappings_text.set(pretty("/admin/config/model-mappings", Some(t.clone())).await);
                metrics_json.set(fetch_json("/api/metrics", Some(t.clone())).await);
                providers_json.set(fetch_json("/api/providers", Some(t)).await);
            }
            Err(e) => { admin_unlocked.set(false); admin_token.set(String::new()); set_stored_token(""); auth_msg.set(format!("Ungültig: {e}")); }
        }
    });
    let do_lock = move |_: MouseEvent| {
        admin_unlocked.set(false); admin_token.set(String::new()); token_input.set(String::new()); set_stored_token("");
        auth_msg.set("Admin gesperrt".into()); tab.set(Tab::Overview);
    };

    let unlocked = admin_unlocked();
    let current = tab();

    rsx! {
        document::Stylesheet { href: STYLE }
        div { class: "shell",
            header { class: "topbar",
                div { class: "brand-wrap",
                    span { class: "dots", i {} i {} i {} }
                    div { div { class: "brand", "bkgcrewcli" } div { class: "sub", "native gateway · neon ui · {gw_state()}" } }
                }
                div { class: "auth",
                    span { class: if unlocked { "badge ok" } else { "badge no" }, if unlocked { "Admin entsperrt" } else { "Admin gesperrt" } }
                    input { r#type: "password", placeholder: "Admin-Token", value: "{token_input()}", oninput: move |e| token_input.set(e.value()) }
                    button { class: "neon-btn lime", onclick: do_unlock, "Entsperren" }
                    button { class: "neon-btn orange", onclick: do_lock, "Sperren" }
                }
            }
            if !auth_msg().is_empty() { div { class: "notice", "{auth_msg()}" } }

            nav { class: "tabs",
                button { class: if current==Tab::Overview { "neon-btn active" } else { "neon-btn" }, onclick: move |_| tab.set(Tab::Overview), "Übersicht" }
                button { class: if current==Tab::Models { "neon-btn active blue" } else { "neon-btn blue" }, onclick: move |_| tab.set(Tab::Models), "Models" }
                button { class: if current==Tab::Metrics { "neon-btn active orange" } else { "neon-btn orange" }, onclick: move |_| tab.set(Tab::Metrics), "Metrics/Provider" }
                button { class: if current==Tab::Run { "neon-btn active magenta" } else { "neon-btn magenta" }, onclick: move |_| tab.set(Tab::Run), "Run" }
                button {
                    class: if unlocked { if current==Tab::Gateway { "neon-btn active lime" } else { "neon-btn lime" } } else { "neon-btn lime locked" },
                    onclick: move |_| { if !unlocked { auth_msg.set("Bitte zuerst Admin-Token oben rechts entsperren.".into()); } else { tab.set(Tab::Gateway); } },
                    if unlocked { "Gateway" } else { "Gateway 🔒" }
                }
                button {
                    class: if unlocked { if current==Tab::Admin { "neon-btn active violet" } else { "neon-btn violet" } } else { "neon-btn violet locked" },
                    onclick: move |_| { if !unlocked { auth_msg.set("Bitte zuerst Admin-Token oben rechts entsperren.".into()); } else { tab.set(Tab::Admin); } },
                    if unlocked { "Admin/Config" } else { "Admin/Config 🔒" }
                }
            }

            if current == Tab::Overview { { render_overview(agents(), agents_err(), cfg_json(), gw_json(), refresh_agents, refresh_cfg) } }
            if current == Tab::Models { { render_models(models_json(), refresh_models) } }
            if current == Tab::Metrics { { render_metrics(metrics_json(), providers_json(), unlocked, refresh_metrics) } }
            if current == Tab::Run { { render_run(run_agent, run_model, run_cwd, run_prompt, run_out, run_busy) } }
            if current == Tab::Gateway { { render_gateway(unlocked, usage_json(), refresh_usage, gw_json()) } }
            if current == Tab::Admin { { render_admin(unlocked, admin_cfg_text, mappings_text, admin_msg, map_msg, admin_token) } }
        }
    }
}

fn render_overview(agents: Vec<AgentInfo>, err: String, cfg: Value, gw: Value, mut refresh_agents: impl FnMut() + 'static, mut refresh_cfg: impl FnMut() + 'static) -> Element {
    rsx! {
        div { class: "grid",
            div { class: "app",
                div { class: "app-bar", span { class: "app-title", "Agenten & Status" } button { class: "neon-btn", onclick: move |_| refresh_agents(), "Aktualisieren" } }
                div { class: "app-body",
                    if !err.is_empty() { div { class: "muted", "{err}" } }
                    else if agents.is_empty() { div { class: "muted", "Keine Agenten geladen." } }
                    else { for a in agents.iter() {
                        span { key: "{a.id}", class: if a.available { "pill ok" } else { "pill bad" },
                            "{a.id}" if a.available { " ✓" } else { " ✗" } if let Some(v)=&a.version { " · {v}" }
                        }
                    }}
                }
                div { class: "app-status", "API: " b { "/api/agents" } " · " b { "/api/run" } " · " b { "/api/config" } }
            }
            div { class: "app violet",
                div { class: "app-bar", span { class: "app-title", "System / Gateway" } button { class: "neon-btn violet", onclick: move |_| refresh_cfg(), "Aktualisieren" } }
                div { class: "app-body stack",
                    div { label { "App Config" } pre { "{serde_json::to_string_pretty(&cfg).unwrap_or_default()}" } }
                    div { label { "Gateway Status" } pre { "{serde_json::to_string_pretty(&gw).unwrap_or_default()}" } }
                }
                div { class: "app-status", "Gateway: " b { "/v1/models" } " · " b { "/v1/chat/completions" } " · " b { "/usage" } " · " b { "/api/metrics" } }
            }
        }
    }
}

fn render_models(models_json: Value, mut refresh: impl FnMut() + 'static) -> Element {
    let data = models_json.get("data").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    rsx! {
        div { class: "app blue",
            div { class: "app-bar", span { class: "app-title", "Models ({data.len()})" } button { class: "neon-btn blue", onclick: move |_| refresh(), "Aktualisieren" } }
            div { class: "app-body stack",
                if let Some(e) = models_json.get("error").and_then(|x| x.as_str()) { div { class: "notice", "{e}" } }
                if data.is_empty() {
                    div { class: "muted", "Keine Modelle geladen. (Gateway/Provider/Keystore prüfen)" }
                    pre { "{serde_json::to_string_pretty(&models_json).unwrap_or_default()}" }
                } else {
                    div { class: "table-wrap",
                        table { class: "sheet",
                            thead { tr { th { "Model ID" } th { "Owned By" } th { "Object" } th { "Info" } } }
                            tbody { for (idx,m) in data.iter().enumerate() {
                                tr { key: "{idx}-{s(m,\"id\")}",
                                    td { class: "mono", style: "color:#d7ff6a;text-shadow:0 0 8px rgba(215,255,106,.35)", "{s(m,\"id\")}" }
                                    td { "{s(m,\"owned_by\")}" }
                                    td { "{s(m,\"object\")}" }
                                    td { class: "muted", "{m.get(\"description\").and_then(|x| x.as_str()).unwrap_or(\"\")}" }
                                }
                            }}
                        }
                    }
                    details { summary { class: "muted", "Rohdaten (/v1/models)" } pre { "{serde_json::to_string_pretty(&models_json).unwrap_or_default()}" } }
                }
            }
            div { class: "app-status", "Quelle: " b { "/v1/models" } " · Registry + Gateway-Provider + Keystore" }
        }
    }
}

fn render_metrics(metrics: Value, providers: Value, unlocked: bool, mut refresh: impl FnMut() + 'static) -> Element {
    let totals = metrics.get("totals").cloned().unwrap_or(Value::Null);
    let by_provider = metrics.get("by_provider").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let by_model = metrics.get("by_model").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let plist = providers.get("providers").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    rsx! {
        div { class: "stack",
            div { class: "app orange",
                div { class: "app-bar", span { class: "app-title", "Metrics / Usage" }
                    if unlocked { button { class: "neon-btn orange", onclick: move |_| refresh(), "Aktualisieren" } }
                }
                div { class: "app-body stack",
                    if !unlocked { div { class: "notice", "Für Provider-Details/Metrics Admin-Token entsperren (oder Usage ohne Auth wo verfügbar)." } }
                    if totals.is_null() && metrics.get("error").is_some() { pre { "{serde_json::to_string_pretty(&metrics).unwrap_or_default()}" } }
                    else {
                        div { class: "stat-grid",
                            {stat("Requests", i64v(&totals,"requests").to_string(), "var(--neon-cyan)")}
                            {stat("Total Tokens", i64v(&totals,"total_tokens").to_string(), "var(--neon-lime)")}
                            {stat("Input Tokens", i64v(&totals,"input_tokens").to_string(), "var(--neon-blue)")}
                            {stat("Output Tokens", i64v(&totals,"output_tokens").to_string(), "var(--neon-magenta)")}
                            {stat("Avg Duration ms", format!("{:.0}", f64v(&totals,"avg_duration_ms")), "var(--neon-orange)")}
                            {stat("Cost USD", format!("{:.4}", f64v(&totals,"cost_usd")), "var(--neon-gold)")}
                        }
                    }
                    div { class: "grid",
                        div {
                            h3 { "By Provider" }
                            {mini_table(&["Provider","Requests","Total Tokens","Cost"], by_provider.iter().map(|r| vec![s(r,"provider"), i64v(r,"requests").to_string(), i64v(r,"total_tokens").to_string(), format!("{:.4}", f64v(r,"cost_usd"))]).collect())}
                        }
                        div {
                            h3 { "By Model" }
                            {mini_table(&["Model","Requests","In","Out","Total"], by_model.iter().map(|r| vec![s(r,"model"), i64v(r,"requests").to_string(), i64v(r,"input_tokens").to_string(), i64v(r,"output_tokens").to_string(), i64v(r,"total_tokens").to_string()]).collect())}
                        }
                    }
                }
                div { class: "app-status", "Quelle: " b { "/api/metrics" } " · " b { "/usage/summary" } }
            }
            div { class: "app violet",
                div { class: "app-bar", span { class: "app-title", "Provider ({plist.len()})" } }
                div { class: "app-body",
                    if plist.is_empty() {
                        div { class: "muted", "Keine Provider geladen (Admin-Token nötig für /api/providers) oder keine konfiguriert." }
                        if !providers.is_null() { pre { "{serde_json::to_string_pretty(&providers).unwrap_or_default()}" } }
                    } else {
                        div { class: "table-wrap",
                            table { class: "sheet",
                                thead { tr { th { "Name" } th { "Source" } th { "Type" } th { "Base URL" } th { "Enabled" } th { "Key" } th { "Models" } } }
                                tbody { for (idx,p) in plist.iter().enumerate() {
                                    let models = p.get("models").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                                    tr { key: "{idx}-{s(p,\"name\")}",
                                        td { class: "mono", style: "color:#7dff9f;text-shadow:0 0 8px rgba(125,255,159,.35)", "{s(p,\"name\")}" }
                                        td { "{s(p,\"source\")}" }
                                        td { "{s(p,\"type\")}" }
                                        td { class: "mono muted", "{s(p,\"base_url\")}" }
                                        td { if p.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false) { span { class: "pill ok", "enabled" } } else { span { class: "pill bad", "disabled" } } }
                                        td { if p.get("has_api_key").and_then(|x| x.as_bool()).unwrap_or(false) { span { class: "pill info", "key" } } else { span { class: "pill warn", "no key" } } }
                                        td { class: "muted", "{models}" }
                                    }
                                }}
                            }
                        }
                    }
                }
                div { class: "app-status", "Quelle: " b { "/api/providers" } " (Gateway-Config + Keystore, keine Secrets)" }
            }
        }
    }
}

fn stat(label: &str, value: String, color: &str) -> Element {
    rsx! { div { class: "stat", style: "--c: {color}", div { class: "stat-label", "{label}" } div { class: "stat-val", "{value}" } } }
}
fn mini_table(headers: &[&str], rows: Vec<Vec<String>>) -> Element {
    rsx! {
        if rows.is_empty() { div { class: "muted", "Keine Daten" } }
        else {
            div { class: "table-wrap", style: "max-height:280px",
                table { class: "sheet",
                    thead { tr { for h in headers { th { "{h}" } } } }
                    tbody { for (i,row) in rows.iter().enumerate() {
                        tr { key: "{i}", for (j,cell) in row.iter().enumerate() {
                            td { class: if j==0 { "mono" } else { "" }, "{cell}" }
                        }}
                    }}
                }
            }
        }
    }
}

fn render_run(
    mut run_agent: Signal<String>, mut run_model: Signal<String>, mut run_cwd: Signal<String>,
    mut run_prompt: Signal<String>, mut run_out: Signal<String>, mut run_busy: Signal<bool>,
) -> Element {
    let do_run = move |_: MouseEvent| spawn(async move {
        run_busy.set(true); run_out.set("Läuft…".into());
        let body = json!({
            "agent": run_agent(), "prompt": run_prompt(),
            "model": if run_model().trim().is_empty() { Value::Null } else { Value::String(run_model()) },
            "cwd": if run_cwd().trim().is_empty() { Value::Null } else { Value::String(run_cwd()) },
        });
        match fetch_text("POST", "/api/run", None, Some(body)).await {
            Ok(t) => match serde_json::from_str::<Value>(&t) {
                Ok(v) => {
                    let out = v.get("output").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
                    let err = v.get("error").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    run_out.set(if !out.is_empty() { out } else if !err.is_empty() { err } else { serde_json::to_string_pretty(&v).unwrap_or(t) });
                }
                Err(_) => run_out.set(t),
            },
            Err(e) => run_out.set(format!("Fehler: {e}")),
        }
        run_busy.set(false);
    });
    rsx! {
        div { class: "app magenta",
            div { class: "app-bar", span { class: "app-title", "Agent ausführen" } }
            div { class: "app-body",
                div { class: "grid",
                    div {
                        label { "Agent" }
                        select { value: "{run_agent()}", oninput: move |e| run_agent.set(e.value()),
                            option { value: "claude", "claude" } option { value: "opencode", "opencode" } option { value: "codex", "codex" }
                            option { value: "cursor", "cursor" } option { value: "pi", "pi" } option { value: "copilot", "copilot" }
                        }
                        label { "Modell (optional, z. B. OpenCode: apieysho/code)" }
                        input { value: "{run_model()}", placeholder: "leer = Default", oninput: move |e| run_model.set(e.value()) }
                        label { "Arbeitsverzeichnis (optional)" }
                        input { value: "{run_cwd()}", placeholder: "leer = Server-CWD", oninput: move |e| run_cwd.set(e.value()) }
                    }
                    div {
                        label { "Prompt" }
                        textarea { value: "{run_prompt()}", placeholder: "Aufgabe/Prompt an den Agenten…", oninput: move |e| run_prompt.set(e.value()) }
                        div { class: "spacer" }
                        button { class: "neon-btn magenta", disabled: run_busy(), onclick: do_run, if run_busy() { "Läuft…" } else { "Starten" } }
                    }
                }
                div { class: "spacer" } label { "Ergebnis" } pre { "{run_out()}" }
            }
            div { class: "app-status", "POST " b { "/api/run" } }
        }
    }
}

fn render_gateway(unlocked: bool, usage: Value, mut refresh_usage: impl FnMut() + 'static, gw: Value) -> Element {
    let events = usage.get("events").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    rsx! {
        if !unlocked { div { class: "notice", "Admin-Token oben rechts eingeben und „Entsperren“, um Gateway-Menüs zu öffnen." } }
        else {
            div { class: "stack",
                div { class: "app lime",
                    div { class: "app-bar", span { class: "app-title", "Usage Events" } button { class: "neon-btn lime", onclick: move |_| refresh_usage(), "/usage laden" } }
                    div { class: "app-body",
                        if events.is_empty() { div { class: "muted", "Keine Events (oder /usage leer)." } pre { "{serde_json::to_string_pretty(&usage).unwrap_or_default()}" } }
                        else {
                            div { class: "table-wrap",
                                table { class: "sheet",
                                    thead { tr { th { "Zeit" } th { "Model" } th { "Provider" } th { "Endpoint" } th { "Tokens" } th { "Dauer" } th { "Status" } th { "Outcome" } } }
                                    tbody { for (i,e) in events.iter().enumerate() {
                                        tr { key: "{i}",
                                            td { class: "muted mono", "{i64v(e,\"created_at_ms\")}" }
                                            td { class: "mono", style: "color:#d7ff6a", "{e.get(\"actual_model\").or(e.get(\"model\")).and_then(|x| x.as_str()).unwrap_or(\"\")}" }
                                            td { "{s(e,\"provider\")}" }
                                            td { "{s(e,\"endpoint\")}" }
                                            td { "{i64v(e,\"total_tokens\")}" }
                                            td { "{i64v(e,\"duration_ms\")}ms" }
                                            td { "{i64v(e,\"status\")}" }
                                            td { "{s(e,\"outcome\")}" }
                                        }
                                    }}
                                }
                            }
                        }
                    }
                    div { class: "app-status", "Quelle: " b { "/usage" } }
                }
                div { class: "app lime",
                    div { class: "app-bar", span { class: "app-title", "Gateway Status" } }
                    div { class: "app-body", pre { "{serde_json::to_string_pretty(&gw).unwrap_or_default()}" } }
                    div { class: "app-status", "Info: " b { "/api/gateway/config" } }
                }
            }
        }
    }
}

fn render_admin(unlocked: bool, mut admin_cfg_text: Signal<String>, mut mappings_text: Signal<String>, mut admin_msg: Signal<String>, mut map_msg: Signal<String>, admin_token: Signal<String>) -> Element {
    let load_cfg = move |_: MouseEvent| spawn(async move {
        let t = admin_token(); if t.is_empty() { admin_msg.set("Gesperrt".into()); return; }
        admin_cfg_text.set(pretty("/admin/config", Some(t)).await); admin_msg.set("Config geladen (sensible Werte maskiert).".into());
    });
    let save_cfg = move |_: MouseEvent| spawn(async move {
        let t = admin_token(); if t.is_empty() { admin_msg.set("Gesperrt".into()); return; }
        match serde_json::from_str::<Value>(&admin_cfg_text()) {
            Ok(v) => match fetch_text("POST", "/admin/config", Some(t), Some(v)).await {
                Ok(_) => admin_msg.set("Gespeichert.".into()),
                Err(e) => admin_msg.set(format!("Fehler: {e}")),
            },
            Err(e) => admin_msg.set(format!("Ungültiges JSON: {e}")),
        }
    });
    let load_map = move |_: MouseEvent| spawn(async move {
        let t = admin_token(); if t.is_empty() { map_msg.set("Gesperrt".into()); return; }
        mappings_text.set(pretty("/admin/config/model-mappings", Some(t)).await); map_msg.set("Mappings geladen.".into());
    });
    let save_map = move |_: MouseEvent| spawn(async move {
        let t = admin_token(); if t.is_empty() { map_msg.set("Gesperrt".into()); return; }
        match serde_json::from_str::<Value>(&mappings_text()) {
            Ok(v) => match fetch_text("POST", "/admin/config/model-mappings", Some(t), Some(v)).await {
                Ok(txt) => { mappings_text.set(serde_json::from_str::<Value>(&txt).ok().and_then(|v| serde_json::to_string_pretty(&v).ok()).unwrap_or(txt)); map_msg.set("Mappings gespeichert.".into()); }
                Err(e) => map_msg.set(format!("Fehler: {e}")),
            },
            Err(e) => map_msg.set(format!("Ungültiges JSON: {e}")),
        }
    });
    rsx! {
        if !unlocked { div { class: "notice", "Admin-Token oben rechts eingeben und „Entsperren“, um Admin/Config zu bearbeiten." } }
        else {
            div { class: "grid",
                div { class: "app violet",
                    div { class: "app-bar", span { class: "app-title", "Gateway Config (Admin)" } }
                    div { class: "app-body stack",
                        div { class: "row", button { class: "neon-btn violet", onclick: load_cfg, "Laden" } button { class: "neon-btn violet", onclick: save_cfg, "Speichern" } }
                        label { "gateway.json" }
                        textarea { style: "min-height:340px;font-family:var(--font-mono);font-size:0.8rem;", value: "{admin_cfg_text()}", oninput: move |e| admin_cfg_text.set(e.value()) }
                        div { class: "muted", "{admin_msg()}" }
                    }
                    div { class: "app-status", "Admin: " b { "/admin/config" } }
                }
                div { class: "app violet",
                    div { class: "app-bar", span { class: "app-title", "Model-Mappings (Admin)" } }
                    div { class: "app-body stack",
                        div { class: "row", button { class: "neon-btn violet", onclick: load_map, "Laden" } button { class: "neon-btn violet", onclick: save_map, "Speichern" } }
                        label { "JSON-Objekt {{ \"source\": \"target\" }}" }
                        textarea { style: "min-height:260px;font-family:var(--font-mono);font-size:0.8rem;", value: "{mappings_text()}", oninput: move |e| mappings_text.set(e.value()) }
                        div { class: "muted", "Defaults (codex-auto-review, gpt-reserve) werden gemerged." }
                        div { class: "muted", "{map_msg()}" }
                    }
                    div { class: "app-status", "Admin: " b { "/admin/config/model-mappings" } }
                }
            }
        }
    }
}
