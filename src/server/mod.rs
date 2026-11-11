// HTTP + WebSocket Server for bkgcrewcli
// Provides REST API, WebSocket event stream, metrics endpoint, and AI gateway.

pub mod api;
pub mod gateway;
pub mod metrics;
pub mod ws;

pub use api::{create_base_router, create_full_router, create_router, ServerState};
pub use metrics::MetricsServer;
pub use ws::{EventBroadcaster, WsServer};
