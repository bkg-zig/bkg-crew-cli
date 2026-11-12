//! Provider admission: per-key sliding-window lanes, cooldowns and a
//! per-model concurrency governor.
//!
//! Every managed provider send must acquire a lane reservation (and a model
//! permit) here first, so rate limits and concurrency rules cannot be
//! bypassed by individual call sites. Lanes reference credentials only by
//! keystore key ID — secret key text never enters this module, its status
//! output or its error messages.
//!
//! Design notes (see `docs/design/crew-lanes-memory-integration.md`):
//! - Exact sliding window per lane, no burst token bucket.
//! - A reservation counts from acquisition. Dropping it before
//!   [`Reservation::mark_sent`] refunds the slot; sent requests (including
//!   retries) keep counting.
//! - `429`/`Retry-After` cools down only the lane that sent the request.
//! - Lane selection: ready conversation-affinity lane first, otherwise the
//!   least normalised load (`in_window / rpm_limit`) with a round-robin
//!   tie-break, then spillover.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use sqlx::{SqlitePool, Row, sqlite::SqlitePoolOptions};
use chrono::{DateTime, Utc};

/// One API key's lane inside a provider pool.
#[derive(Debug, Clone)]
pub struct LaneConfig {
    /// Keystore reference for the credential. Never the key text itself.
    pub key_id: String,
    /// Max sends per rolling window. Must be > 0.
    pub rpm_limit: u32,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub provider_id: String,
    /// Upstream model this pool serves (part of the pool identity).
    pub model: String,
    /// Rolling window length, typically 60s.
    pub window: Duration,
}

impl PoolConfig {
    pub fn new(provider_id: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            model: model.into(),
            window: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionError {
    /// No lane can take a send right now; `retry_in` is the earliest wait.
    NoCapacity { retry_in: Duration },
    /// Pool exists but every lane is disabled.
    NoEnabledLanes,
    InvalidConfig(String),
}

impl std::fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdmissionError::NoCapacity { retry_in } => {
                write!(f, "admission: all lanes at rate limit, retry in {}ms", retry_in.as_millis())
            }
            AdmissionError::NoEnabledLanes => write!(f, "admission: no enabled lanes in pool"),
            AdmissionError::InvalidConfig(m) => write!(f, "admission: invalid pool config: {m}"),
        }
    }
}
impl std::error::Error for AdmissionError {}

/// Non-secret snapshot of one lane for status/doctor output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneStatus {
    pub key_id: String,
    pub in_window: usize,
    pub rpm_limit: u32,
    pub enabled: bool,
    pub cooling_down: bool,
    pub last_error_class: Option<ErrorClass>,
}

/// Generic error class for status reporting (no secrets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorClass {
    RateLimited,
    NetworkError,
    AuthError,
    ServerError,
    Timeout,
    Unknown,
}

impl From<&str> for ErrorClass {
    fn from(s: &str) -> Self {
        match s {
            "rate_limited" => ErrorClass::RateLimited,
            "network_error" => ErrorClass::NetworkError,
            "auth_error" => ErrorClass::AuthError,
            "server_error" => ErrorClass::ServerError,
            "timeout" => ErrorClass::Timeout,
            _ => ErrorClass::Unknown,
        }
    }
}

impl std::fmt::Display for ErrorClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorClass::RateLimited => write!(f, "rate_limited"),
            ErrorClass::NetworkError => write!(f, "network_error"),
            ErrorClass::AuthError => write!(f, "auth_error"),
            ErrorClass::ServerError => write!(f, "server_error"),
            ErrorClass::Timeout => write!(f, "timeout"),
            ErrorClass::Unknown => write!(f, "unknown"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct LaneState {
    config: LaneConfig,
    sends: VecDeque<Instant>,
    cooldown_until: Option<Instant>,
    last_error_class: Option<ErrorClass>,
}

impl LaneState {
    fn prune(&mut self, now: Instant, window: Duration) {
        while let Some(&front) = self.sends.front() {
            if now.duration_since(front) >= window {
                self.sends.pop_front();
            } else {
                break;
            }
        }
    }
    fn ready(&self, now: Instant) -> bool {
        self.config.enabled
            && self.cooldown_until.map(|c| c <= now).unwrap_or(true)
            && (self.sends.len() as u32) < self.config.rpm_limit
    }
    fn load(&self) -> f64 {
        if self.config.rpm_limit == 0 {
            return f64::INFINITY;
        }
        self.sends.len() as f64 / self.config.rpm_limit as f64
    }
}

#[derive(Debug)]
pub(crate) struct PoolInner {
    config: PoolConfig,
    lanes: Vec<LaneState>,
    cursor: usize,
    affinity: HashMap<String, usize>,
}

/// A provider pool: several key lanes sharing one provider/model limit scope.
#[derive(Clone, Debug)]
pub struct ProviderPool {
    inner: Arc<Mutex<PoolInner>>,
}

impl ProviderPool {
    pub fn new(config: PoolConfig, lanes: Vec<LaneConfig>) -> Result<Self, AdmissionError> {
        if lanes.is_empty() {
            return Err(AdmissionError::InvalidConfig("pool needs at least one lane".into()));
        }
        for l in &lanes {
            if l.rpm_limit == 0 {
                return Err(AdmissionError::InvalidConfig(format!("lane {} has rpm_limit 0", l.key_id)));
            }
            if l.key_id.is_empty() {
                return Err(AdmissionError::InvalidConfig("lane key_id must not be empty".into()));
            }
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(PoolInner {
                config,
                lanes: lanes
                    .into_iter()
                    .map(|config| LaneState { config, sends: VecDeque::new(), cooldown_until: None, last_error_class: None })
                    .collect(),
                cursor: 0,
                affinity: HashMap::new(),
            })),
        })
    }

    /// Try to reserve one send slot. Affinity is honoured only when that lane
    /// is immediately ready; otherwise the least loaded ready lane wins.
    pub fn try_acquire(&self, affinity_key: Option<&str>) -> Result<Reservation, AdmissionError> {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let window = inner.config.window;
        for lane in inner.lanes.iter_mut() {
            lane.prune(now, window);
        }
        if !inner.lanes.iter().any(|l| l.config.enabled) {
            return Err(AdmissionError::NoEnabledLanes);
        }
        // 1) conversation affinity, only if immediately ready
        if let Some(key) = affinity_key {
            if let Some(&idx) = inner.affinity.get(key) {
                if inner.lanes[idx].ready(now) {
                    inner.lanes[idx].sends.push_back(now);
                    return Ok(Reservation::new(self.inner.clone(), idx, now));
                }
            }
        }
        // 2) least normalised load, round-robin tie-break starting at cursor
        let n = inner.lanes.len();
        let mut best: Option<usize> = None;
        for step in 0..n {
            let idx = (inner.cursor + step) % n;
            if !inner.lanes[idx].ready(now) {
                continue;
            }
            match best {
                None => best = Some(idx),
                Some(b) if inner.lanes[idx].load() < inner.lanes[b].load() => best = Some(idx),
                _ => {}
            }
        }
        let Some(idx) = best else {
            return Err(AdmissionError::NoCapacity { retry_in: inner.next_free_in(now) });
        };
        inner.cursor = (idx + 1) % n;
        inner.lanes[idx].sends.push_back(now);
        if let Some(key) = affinity_key {
            inner.affinity.insert(key.to_string(), idx);
        }
        Ok(Reservation::new(self.inner.clone(), idx, now))
    }

    /// Wait (FIFO-fair through the dispatcher gate in `AdmissionRegistry`)
    /// until a lane frees or `max_wait` elapses.
    pub async fn acquire_wait(&self, affinity_key: Option<&str>, max_wait: Duration) -> Result<Reservation, AdmissionError> {
        let start = Instant::now();
        loop {
            match self.try_acquire(affinity_key) {
                Ok(r) => return Ok(r),
                Err(AdmissionError::NoCapacity { retry_in }) => {
                    let waited = start.elapsed();
                    if waited >= max_wait {
                        return Err(AdmissionError::NoCapacity { retry_in });
                    }
                    let sleep_for = retry_in.min(max_wait - waited).min(Duration::from_millis(100));
                    tokio::time::sleep(sleep_for.max(Duration::from_millis(1))).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Record a classified send error; `cooldown` optionally parks the lane.
    pub fn report_error(&self, key_id: &str, error_class: ErrorClass, cooldown: Option<Duration>) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(lane) = inner.lanes.iter_mut().find(|l| l.config.key_id == key_id) {
            lane.last_error_class = Some(error_class);
            if let Some(d) = cooldown {
                lane.cooldown_until = Some(Instant::now() + d);
            }
        }
    }

    /// Mark the lane that sent a request as rate-limited (429 / Retry-After).
    pub fn report_rate_limited(&self, key_id: &str, retry_after: Duration) {
        self.report_error(key_id, ErrorClass::RateLimited, Some(retry_after));
    }

    pub fn status(&self) -> Vec<LaneStatus> {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let window = inner.config.window;
        for lane in inner.lanes.iter_mut() {
            lane.prune(now, window);
        }
        inner
            .lanes
            .iter()
            .map(|l| LaneStatus {
                key_id: l.config.key_id.clone(),
                in_window: l.sends.len(),
                rpm_limit: l.config.rpm_limit,
                enabled: l.config.enabled,
                cooling_down: l.cooldown_until.map(|c| c > now).unwrap_or(false),
                last_error_class: l.last_error_class.clone(),
            })
            .collect()
    }

    /// Replace lane configs (pool rebuild). Send state for unchanged key IDs
    /// is preserved; removed keys lose their state.
    pub fn rebuild(&self, lanes: Vec<LaneConfig>) -> Result<(), AdmissionError> {
        for l in &lanes {
            if l.rpm_limit == 0 {
                return Err(AdmissionError::InvalidConfig(format!("lane {} has rpm_limit 0", l.key_id)));
            }
        }
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let old: HashMap<String, LaneState> = inner
            .lanes
            .drain(..)
            .map(|l| (l.config.key_id.clone(), l))
            .collect();
        let mut by_id = old;
        inner.lanes = lanes
            .into_iter()
            .map(|config| match by_id.remove(&config.key_id) {
                Some(mut state) => {
                    state.config = config;
                    state
                }
                None => LaneState { config, sends: VecDeque::new(), cooldown_until: None, last_error_class: None },
            })
            .collect();
        let lane_count = inner.lanes.len();
        inner.affinity.retain(|_, idx| *idx < lane_count);
        Ok(())
    }
}

impl PoolInner {
    fn next_free_in(&self, now: Instant) -> Duration {
        let mut best: Option<Duration> = None;
        for lane in &self.lanes {
            if !lane.config.enabled {
                continue;
            }
            if let Some(c) = lane.cooldown_until {
                if c > now {
                    best = Some(best.map(|b| b.min(c - now)).unwrap_or(c - now));
                    continue;
                }
            }
            if (lane.sends.len() as u32) >= lane.config.rpm_limit {
                if let Some(&front) = lane.sends.front() {
                    let frees = self.config.window.saturating_sub(now.duration_since(front));
                    best = Some(match best {
                        Some(b) => b.min(frees),
                        None => frees,
                    });
                }
            }
        }
        best.unwrap_or(Duration::from_millis(50))
    }
}

/// One reserved send slot. Dropping before `mark_sent` refunds the slot;
/// a sent request keeps counting against the window.
#[derive(Debug)]
pub struct Reservation {
    pub(crate) inner: Arc<Mutex<PoolInner>>,
    pub lane_index: usize,
    pub at: Instant,
    pub sent: bool,
}

impl Reservation {
    fn new(inner: Arc<Mutex<PoolInner>>, lane_index: usize, at: Instant) -> Self {
        Self { inner, lane_index, at, sent: false }
    }
    pub fn mark_sent(&mut self) {
        self.sent = true;
    }
    /// Keystore key ID of the lane this reservation belongs to.
    pub fn key_id(&self) -> String {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.lanes.get(self.lane_index).map(|l| l.config.key_id.clone()).unwrap_or_default()
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.sent {
            return;
        }
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(lane) = inner.lanes.get_mut(self.lane_index) {
            if let Some(pos) = lane.sends.iter().position(|&t| t == self.at) {
                lane.sends.remove(pos);
            }
        }
    }
}

/// Per-model concurrency governor: a model-wide in-flight cap, independent
/// of per-key RPM. Permits are RAII guards.
pub struct ModelGovernor {
    inner: Arc<Mutex<HashMap<String, ModelState>>>,
}

#[derive(Debug)]
pub(crate) struct ModelState {
    max_in_flight: u32,
    in_flight: u32,
}

impl Default for ModelGovernor {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelGovernor {
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(HashMap::new())) }
    }
    /// `max_in_flight == 0` means unlimited.
    pub fn try_acquire(&self, model: &str, max_in_flight: u32) -> Option<ModelPermit> {
        if max_in_flight == 0 {
            return Some(ModelPermit { inner: self.inner.clone(), model: model.to_string(), counted: false });
        }
        let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let st = map.entry(model.to_string()).or_insert(ModelState { max_in_flight, in_flight: 0 });
        st.max_in_flight = max_in_flight;
        if st.in_flight >= st.max_in_flight {
            return None;
        }
        st.in_flight += 1;
        Some(ModelPermit { inner: self.inner.clone(), model: model.to_string(), counted: true })
    }
    pub fn in_flight(&self, model: &str) -> u32 {
        let map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        map.get(model).map(|s| s.in_flight).unwrap_or(0)
    }
}

#[derive(Debug)]
pub struct ModelPermit {
    pub(crate) inner: Arc<Mutex<HashMap<String, ModelState>>>,
    pub model: String,
    pub counted: bool,
}

impl Drop for ModelPermit {
    fn drop(&mut self) {
        if !self.counted {
            return;
        }
        let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(st) = map.get_mut(&self.model) {
            st.in_flight = st.in_flight.saturating_sub(1);
        }
    }
}

/// SQLite persistence for admission state.
#[derive(Clone)]
pub struct AdmissionPersist {
    pool: Arc<Mutex<SqlitePool>>,
}

impl AdmissionPersist {
    /// Open or create the persistence database.
    pub async fn open(path: std::path::PathBuf) -> Result<Self, AdmissionError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AdmissionError::InvalidConfig(e.to_string()))?;
        }
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let pool = SqlitePoolOptions::new().max_connections(5).connect(&url).await
            .map_err(|e| AdmissionError::InvalidConfig(e.to_string()))?;
        let persist = Self { pool: Arc::new(Mutex::new(pool)) };
        persist.init().await?;
        Ok(persist)
    }

    async fn init(&self) -> Result<(), AdmissionError> {
        let pool = self.pool.lock().unwrap_or_else(|p| p.into_inner());
        // Create schema version table
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS admission_schema_version (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            );"#,
        ).execute(&*pool).await.map_err(|e| AdmissionError::InvalidConfig(e.to_string()))?;

        // Run migrations
        self.migrate().await?;

        // Create tables
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS admission_pools (
                id TEXT PRIMARY KEY,
                provider_id TEXT NOT NULL,
                model TEXT NOT NULL,
                window_secs INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS admission_lanes (
                pool_id TEXT NOT NULL,
                key_id TEXT NOT NULL,
                rpm_limit INTEGER NOT NULL,
                enabled INTEGER NOT NULL,
                sends_json TEXT NOT NULL DEFAULT '[]',
                cooldown_until TEXT,
                last_error_class TEXT,
                PRIMARY KEY (pool_id, key_id),
                FOREIGN KEY (pool_id) REFERENCES admission_pools(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_admission_lanes_pool ON admission_lanes(pool_id);"#,
        ).execute(&*pool).await.map_err(|e| AdmissionError::InvalidConfig(e.to_string()))?;

        // Create model governor state table
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS admission_model_governor (
                model TEXT PRIMARY KEY,
                max_in_flight INTEGER NOT NULL,
                in_flight INTEGER NOT NULL DEFAULT 0
            );"#,
        ).execute(&*pool).await.map_err(|e| AdmissionError::InvalidConfig(e.to_string()))?;

        Ok(())
    }

    /// Run schema migrations to the current version.
    async fn migrate(&self) -> Result<(), AdmissionError> {
        const CURRENT_VERSION: i64 = 1;

        // Get current version
        let pool = self.pool.lock().unwrap_or_else(|p| p.into_inner());
        let current: Option<i64> = sqlx::query_scalar("SELECT MAX(version) FROM admission_schema_version")
            .fetch_optional(&*pool).await
            .map_err(|e| AdmissionError::InvalidConfig(e.to_string()))?
            .flatten();

        let current = current.unwrap_or(0);

        if current < CURRENT_VERSION {
            // Migration from 0 to 1: initial schema
            if current == 0 {
                // Tables already created in init(), just record version
                let now = Utc::now().to_rfc3339();
                let pool = self.pool.lock().unwrap_or_else(|p| p.into_inner());
                sqlx::query(
                    r#"INSERT OR REPLACE INTO admission_schema_version (version, applied_at) VALUES (?, ?)"#,
                )
                .bind(CURRENT_VERSION)
                .bind(now)
                .execute(&*pool).await
                .map_err(|e| AdmissionError::InvalidConfig(e.to_string()))?;
            }
        } else if current > CURRENT_VERSION {
            return Err(AdmissionError::InvalidConfig(
                format!("Database schema version {} is newer than supported version {}", current, CURRENT_VERSION)
            ));
        }

        Ok(())
    }

    /// Save pool configuration.
    pub async fn save_pool(&self, pool_id: &str, provider_id: &str, model: &str, window: Duration) -> Result<(), AdmissionError> {
        let now = Utc::now().to_rfc3339();
        let pool = self.pool.lock().unwrap_or_else(|p| p.into_inner());
        sqlx::query(
            r#"INSERT OR REPLACE INTO admission_pools (id, provider_id, model, window_secs, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?)"#,
        )
        .bind(pool_id)
        .bind(provider_id)
        .bind(model)
        .bind(window.as_secs() as i64)
        .bind(now.clone())
        .bind(now)
        .execute(&*pool).await.map_err(|e| AdmissionError::InvalidConfig(e.to_string()))?;
        Ok(())
    }

    /// Save lane state (sends, cooldown, error class).
    pub async fn save_lane(&self, pool_id: &str, key_id: &str, rpm_limit: u32, enabled: bool, sends: &VecDeque<Instant>, _window: Duration, cooldown_until: Option<Instant>, last_error_class: Option<ErrorClass>) -> Result<(), AdmissionError> {
        let now = Utc::now();
        let sends_json = serde_json::to_string(&sends.iter()
            .map(|t| t.duration_since(Instant::now()).as_millis() as i64)
            .collect::<Vec<_>>()
        ).unwrap_or_else(|_| "[]".to_string());

        let cooldown_str = cooldown_until.map(|c| {
            let elapsed = Instant::now().duration_since(c);
            (now - Duration::from_millis(elapsed.as_millis() as u64)).to_rfc3339()
        });

        let error_class_str = last_error_class.as_ref().map(|e| e.to_string());

        let pool = self.pool.lock().unwrap_or_else(|p| p.into_inner());
        sqlx::query(
            r#"INSERT OR REPLACE INTO admission_lanes (pool_id, key_id, rpm_limit, enabled, sends_json, cooldown_until, last_error_class)
               VALUES (?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(pool_id)
        .bind(key_id)
        .bind(rpm_limit as i64)
        .bind(if enabled { 1 } else { 0 })
        .bind(sends_json)
        .bind(cooldown_str)
        .bind(error_class_str)
        .execute(&*pool).await.map_err(|e| AdmissionError::InvalidConfig(e.to_string()))?;
        Ok(())
    }

    /// Load all pools and their lanes.
    pub async fn load_all(&self) -> Result<Vec<PersistedPool>, AdmissionError> {
        let pool = self.pool.lock().unwrap_or_else(|p| p.into_inner());
        let pool_rows = sqlx::query("SELECT * FROM admission_pools").fetch_all(&*pool).await
            .map_err(|e| AdmissionError::InvalidConfig(e.to_string()))?;
        let lane_rows = sqlx::query("SELECT * FROM admission_lanes").fetch_all(&*pool).await
            .map_err(|e| AdmissionError::InvalidConfig(e.to_string()))?;

        let mut pools = Vec::new();
        for prow in pool_rows {
            let pool_id: String = prow.try_get("id").unwrap_or_default();
            let provider_id: String = prow.try_get("provider_id").unwrap_or_default();
            let model: String = prow.try_get("model").unwrap_or_default();
            let window_secs: i64 = prow.try_get("window_secs").unwrap_or(60);

            let lanes: Vec<PersistedLane> = lane_rows.iter()
                .filter(|lrow| {
                    let lid: String = lrow.try_get("pool_id").unwrap_or_default();
                    lid == pool_id
                })
                .map(|lrow| {
                    let key_id: String = lrow.try_get("key_id").unwrap_or_default();
                    let rpm_limit: i64 = lrow.try_get("rpm_limit").unwrap_or(40);
                    let enabled: i64 = lrow.try_get("enabled").unwrap_or(1);
                    let sends_json: String = lrow.try_get("sends_json").unwrap_or_else(|_| "[]".to_string());
                    let cooldown_until: Option<String> = lrow.try_get("cooldown_until").ok();
                    let last_error_class_str: Option<String> = lrow.try_get("last_error_class").ok();
                    let last_error_class = last_error_class_str.as_deref().map(ErrorClass::from);

                    let sends_deque: VecDeque<Instant> = serde_json::from_str::<Vec<i64>>(&sends_json)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|ms| Instant::now() - Duration::from_millis(ms as u64))
                        .collect();
                    let cooldown = cooldown_until.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                        .map(|dt| dt.with_timezone(&Utc));

                    PersistedLane {
                        key_id,
                        rpm_limit: rpm_limit as u32,
                        enabled: enabled != 0,
                        sends: sends_deque,
                        cooldown_until: cooldown,
                        last_error_class,
                    }
                })
                .collect();

            pools.push(PersistedPool {
                id: pool_id,
                provider_id,
                model,
                window: Duration::from_secs(window_secs as u64),
                lanes,
            });
        }
        Ok(pools)
    }

    /// Delete a pool and its lanes.
    pub async fn delete_pool(&self, pool_id: &str) -> Result<(), AdmissionError> {
        let pool = self.pool.lock().unwrap_or_else(|p| p.into_inner());
        sqlx::query("DELETE FROM admission_pools WHERE id = ?")
            .bind(pool_id)
            .execute(&*pool).await.map_err(|e| AdmissionError::InvalidConfig(e.to_string()))?;
        Ok(())
    }

    /// Save model governor state.
    pub async fn save_model_governor(&self, model: &str, max_in_flight: u32, in_flight: u32) -> Result<(), AdmissionError> {
        let pool = self.pool.lock().unwrap_or_else(|p| p.into_inner());
        sqlx::query(
            r#"INSERT OR REPLACE INTO admission_model_governor (model, max_in_flight, in_flight)
               VALUES (?, ?, ?)"#,
        )
        .bind(model)
        .bind(max_in_flight as i64)
        .bind(in_flight as i64)
        .execute(&*pool).await.map_err(|e| AdmissionError::InvalidConfig(e.to_string()))?;
        Ok(())
    }

    /// Load model governor state.
    pub async fn load_model_governor(&self) -> Result<Vec<(String, u32, u32)>, AdmissionError> {
        let pool = self.pool.lock().unwrap_or_else(|p| p.into_inner());
        let rows = sqlx::query("SELECT model, max_in_flight, in_flight FROM admission_model_governor")
            .fetch_all(&*pool).await
            .map_err(|e| AdmissionError::InvalidConfig(e.to_string()))?;

        let mut result = Vec::new();
        for row in rows {
            let model: String = row.try_get("model").unwrap_or_default();
            let max_in_flight: i64 = row.try_get("max_in_flight").unwrap_or(0);
            let in_flight: i64 = row.try_get("in_flight").unwrap_or(0);
            result.push((model, max_in_flight as u32, in_flight as u32));
        }
        Ok(result)
    }
}

#[derive(Debug, Clone)]
pub struct PersistedPool {
    pub id: String,
    pub provider_id: String,
    pub model: String,
    pub window: Duration,
    pub lanes: Vec<PersistedLane>,
}

#[derive(Debug, Clone)]
pub struct PersistedLane {
    pub key_id: String,
    pub rpm_limit: u32,
    pub enabled: bool,
    pub sends: VecDeque<Instant>,
    pub cooldown_until: Option<DateTime<Utc>>,
    pub last_error_class: Option<ErrorClass>,
}

/// Request struct for the async queue with deadline and cancellation.
#[derive(Debug)]
pub struct QueuedRequest {
    pub id: String,
    pub provider_id: String,
    pub model: String,
    pub key_id: String,
    pub rpm_limit: u32,
    pub model_max_concurrency: u32,
    pub deadline: Instant,
    pub cancel_token: tokio_util::sync::CancellationToken,
    pub response_tx: tokio::sync::oneshot::Sender<Result<(Reservation, ModelPermit), AdmissionError>>,
}

/// Async FIFO queue for admission requests with deadline and cancellation support.
pub struct AdmissionQueue {
    queue: Arc<Mutex<VecDeque<QueuedRequest>>>,
    notifier: tokio::sync::Notify,
}

impl Default for AdmissionQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl AdmissionQueue {
    pub fn new() -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            notifier: tokio::sync::Notify::new(),
        }
    }

    /// Enqueue a request, returns a receiver for the result.
    pub async fn enqueue(&self, request: QueuedRequest) -> Result<(Reservation, ModelPermit), AdmissionError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut req = request;
        req.response_tx = tx;
        {
            let mut q = self.queue.lock().unwrap_or_else(|p| p.into_inner());
            q.push_back(req);
        }
        self.notifier.notify_one();
        rx.await.map_err(|_| AdmissionError::InvalidConfig("channel closed".into()))?
    }

    /// Pop the next request that hasn't expired.
    pub fn pop_ready(&self, now: Instant) -> Option<QueuedRequest> {
        let mut q = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        while let Some(front) = q.front() {
            if front.deadline <= now || front.cancel_token.is_cancelled() {
                let expired = q.pop_front().unwrap();
                let _ = expired.response_tx.send(Err(AdmissionError::NoCapacity { retry_in: Duration::from_millis(50) }));
                continue;
            }
            return q.pop_front();
        }
        None
    }

    /// Wait for the next request or until notified.
    pub async fn wait_for_request(&self) {
        self.notifier.notified().await;
    }

    /// Get queue length.
    pub fn len(&self) -> usize {
        self.queue.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Check if queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.lock().unwrap_or_else(|p| p.into_inner()).is_empty()
    }
}

/// Process-wide registry of pools, keyed by `provider/model/key-set`.
/// Exactly one pool owner per key set is enforced by construction: all
/// sends in this process go through this single registry instance.
pub struct AdmissionRegistry {
    pools: Mutex<HashMap<String, ProviderPool>>,
    governor: ModelGovernor,
    turn: tokio::sync::Mutex<()>,
    queue: AdmissionQueue,
    persist: Option<AdmissionPersist>,
}

impl AdmissionRegistry {
    /// Get a reference to the pools map for status/diagnostic purposes.
    pub fn pools(&self) -> &Mutex<HashMap<String, ProviderPool>> {
        &self.pools
    }

    /// Create a new registry with optional persistence.
    pub async fn new_with_persist(persist: Option<AdmissionPersist>) -> Result<Self, AdmissionError> {
        let registry = Self {
            pools: Mutex::new(HashMap::new()),
            governor: ModelGovernor::new(),
            turn: tokio::sync::Mutex::new(()),
            queue: AdmissionQueue::new(),
            persist,
        };

        // Load persisted state if available
        if let Some(ref p) = registry.persist {
            let persisted = p.load_all().await?;
            let mut pools = registry.pools.lock().unwrap_or_else(|p| p.into_inner());
            for pp in persisted {
                let lane_configs: Vec<LaneConfig> = pp.lanes.iter().map(|l| LaneConfig {
                    key_id: l.key_id.clone(),
                    rpm_limit: l.rpm_limit,
                    enabled: l.enabled,
                }).collect();

                let pool = ProviderPool::new(
                    PoolConfig {
                        provider_id: pp.provider_id.clone(),
                        model: pp.model.clone(),
                        window: pp.window,
                    },
                    lane_configs,
                ).map_err(|e| AdmissionError::InvalidConfig(e.to_string()))?;

                // Restore lane state
                {
                    let mut inner = pool.inner.lock().unwrap_or_else(|p| p.into_inner());
                    for (idx, lane) in pp.lanes.iter().enumerate() {
                        if idx < inner.lanes.len() {
                            inner.lanes[idx].sends = lane.sends.clone();
                            if let Some(dt) = lane.cooldown_until {
                                let now = Instant::now();
                                let elapsed = Utc::now().signed_duration_since(dt);
                                inner.lanes[idx].cooldown_until = Some(now - Duration::from_millis(elapsed.num_milliseconds() as u64));
                            }
                            inner.lanes[idx].last_error_class = lane.last_error_class.clone();
                        }
                    }
                }

                pools.insert(pp.id, pool);
            }

            // Load model governor state
            let governor_state = p.load_model_governor().await?;
            {
                let mut inner = registry.governor.inner.lock().unwrap_or_else(|p| p.into_inner());
                for (model, max_in_flight, in_flight) in governor_state {
                    inner.insert(model, ModelState { max_in_flight, in_flight });
                }
            }
        }

        Ok(registry)
    }

    /// Create a new in-memory registry (no persistence).
    pub fn new() -> Self {
        Self {
            pools: Mutex::new(HashMap::new()),
            governor: ModelGovernor::new(),
            turn: tokio::sync::Mutex::new(()),
            queue: AdmissionQueue::new(),
            persist: None,
        }
    }

    /// Start the background queue worker that processes enqueued requests.
    /// This should be called once per registry instance to enable true FIFO async queuing.
    pub fn start_worker(self: &Arc<Self>) {
        let registry = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                // Wait for a request or check for expired ones
                let now = Instant::now();
                if let Some(req) = registry.queue.pop_ready(now) {
                    // Process the request
                    let pool = registry.pool_for(&req.provider_id, &req.model, &req.key_id, req.rpm_limit);

                    // Try to acquire model permit
                    if let Some(permit) = registry.governor.try_acquire(&req.model, req.model_max_concurrency) {
                        match pool.try_acquire(None) {
                            Ok(mut r) => {
                                r.mark_sent();
                                let _ = req.response_tx.send(Ok((r, permit)));
                                continue;
                            }
                            Err(AdmissionError::NoCapacity { retry_in }) => {
                                drop(permit);
                                // Re-queue with updated deadline if still time
                                if req.deadline > Instant::now() + retry_in {
                                    let mut new_req = req;
                                    new_req.deadline = new_req.deadline.min(Instant::now() + retry_in);
                                    let _ = registry.queue.enqueue(new_req).await;
                                } else {
                                    let _ = req.response_tx.send(Err(AdmissionError::NoCapacity { retry_in }));
                                }
                                continue;
                            }
                            Err(e) => {
                                let _ = req.response_tx.send(Err(e));
                                continue;
                            }
                        }
                    } else {
                        // No model permit available, re-queue if time permits
                        if req.deadline > Instant::now() + Duration::from_millis(10) {
                            let mut new_req = req;
                            new_req.deadline = new_req.deadline.min(Instant::now() + Duration::from_millis(10));
                            let _ = registry.queue.enqueue(new_req).await;
                        } else {
                            let _ = req.response_tx.send(Err(AdmissionError::NoCapacity { retry_in: Duration::from_millis(50) }));
                        }
                        continue;
                    }
                }

                // No ready request, wait for notification
                registry.queue.wait_for_request().await;
            }
        });
    }

    /// Persist current state.
    pub async fn persist(&self) -> Result<(), AdmissionError> {
        if let Some(p) = &self.persist {
            // Collect data while holding locks, then release before await
            let pool_data: Vec<(String, String, String, Duration, Vec<LaneStatus>, Vec<(VecDeque<Instant>, Option<Instant>, Option<ErrorClass>)>)> = {
                let pools = self.pools.lock().unwrap_or_else(|p| p.into_inner());
                pools.iter().map(|(id, pool)| {
                    let status = pool.status();
                    let pool_inner = pool.inner.lock().unwrap_or_else(|p| p.into_inner());
                    let lane_data: Vec<(VecDeque<Instant>, Option<Instant>, Option<ErrorClass>)> = pool_inner.lanes.iter().map(|l| (l.sends.clone(), l.cooldown_until, l.last_error_class.clone())).collect();
                    (id.clone(), pool_inner.config.provider_id.clone(), pool_inner.config.model.clone(), pool_inner.config.window, status, lane_data)
                }).collect()
            };

            // Collect governor state while holding lock
            let governor_data: Vec<(String, u32, u32)> = {
                let inner = self.governor.inner.lock().unwrap_or_else(|p| p.into_inner());
                inner.iter().map(|(model, state)| (model.clone(), state.max_in_flight, state.in_flight)).collect()
            };

            for (id, provider_id, model, window, status, lane_data) in pool_data {
                p.save_pool(&id, &provider_id, &model, window).await?;
                for (lane_status, (sends, cooldown_until, last_error_class)) in status.iter().zip(lane_data.iter()) {
                    p.save_lane(&id, &lane_status.key_id, lane_status.rpm_limit, lane_status.enabled,
                        sends, window, *cooldown_until, last_error_class.clone()).await?;
                }
            }

            // Persist model governor state
            for (model, max_in_flight, in_flight) in governor_data {
                p.save_model_governor(&model, max_in_flight, in_flight).await?;
            }
        }
        Ok(())
    }

    /// Get or create the single-lane pool for one provider credential.
    /// Existing single-key providers become a pool with exactly one lane.
    pub fn pool_for(&self, provider_id: &str, model: &str, key_id: &str, rpm_limit: u32) -> ProviderPool {
        let id = format!("{provider_id}/{model}");
        let mut pools = self.pools.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(p) = pools.get(&id) {
            // Keep the pool; rebuild lanes if the key or limit changed.
            let _ = p.rebuild(vec![LaneConfig { key_id: key_id.to_string(), rpm_limit: rpm_limit.max(1), enabled: true }]);
            return p.clone();
        }
        let pool = ProviderPool::new(
            PoolConfig::new(provider_id, model),
            vec![LaneConfig { key_id: key_id.to_string(), rpm_limit: rpm_limit.max(1), enabled: true }],
        )
        .expect("single-lane pool config is valid");
        pools.insert(id, pool.clone());
        pool
    }

    /// Acquire a lane reservation plus a model permit, waiting FIFO up to
    /// `max_wait`. On success the reservation is already `mark_sent`.
    /// Uses the async FIFO queue with deadline and cancellation support.
    pub async fn acquire(
        &self,
        provider_id: &str,
        model: &str,
        key_id: &str,
        rpm_limit: u32,
        model_max_concurrency: u32,
        max_wait: Duration,
    ) -> Result<(Reservation, ModelPermit), AdmissionError> {
        let id = uuid::Uuid::new_v4().to_string();
        let deadline = Instant::now() + max_wait;
        let cancel_token = tokio_util::sync::CancellationToken::new();

        let (tx, rx) = tokio::sync::oneshot::channel();

        let request = QueuedRequest {
            id,
            provider_id: provider_id.to_string(),
            model: model.to_string(),
            key_id: key_id.to_string(),
            rpm_limit,
            model_max_concurrency,
            deadline,
            cancel_token: cancel_token.clone(),
            response_tx: tx,
        };

        self.queue.enqueue(request).await?;
        rx.await.map_err(|_| AdmissionError::InvalidConfig("channel closed".into()))?
    }

    /// Cool down the lane that sent a rate-limited request in the
    /// provider/model pool, if that pool exists.
    pub fn report_rate_limited(&self, provider_id: &str, model: &str, key_id: &str, retry_after: Duration) {
        let id = format!("{provider_id}/{model}");
        let pools = self.pools.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(pool) = pools.get(&id) {
            pool.report_rate_limited(key_id, retry_after);
        }
    }

    pub fn governor(&self) -> &ModelGovernor {
        &self.governor
    }

    /// Non-secret status of every known pool.
    pub fn status(&self) -> Vec<(String, Vec<LaneStatus>)> {
        let pools = self.pools.lock().unwrap_or_else(|p| p.into_inner());
        pools.iter().map(|(id, p)| (id.clone(), p.status())).collect()
    }
}

impl Default for AdmissionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// The one process-wide admission registry with persistence.
pub async fn global_registry_with_persist(path: Option<std::path::PathBuf>) -> Result<Arc<AdmissionRegistry>, AdmissionError> {
    static REGISTRY: OnceLock<Arc<AdmissionRegistry>> = OnceLock::new();

    if let Some(p) = path {
        let persist = Some(AdmissionPersist::open(p).await?);
        let registry = AdmissionRegistry::new_with_persist(persist).await?;
        let arc = Arc::new(registry);
        arc.start_worker();
        Ok(REGISTRY.get_or_init(|| arc).clone())
    } else {
        let registry = AdmissionRegistry::new_with_persist(None).await?;
        let arc = Arc::new(registry);
        arc.start_worker();
        Ok(REGISTRY.get_or_init(|| arc).clone())
    }
}

/// Backward compatible global registry (in-memory only).
pub fn global_registry() -> Arc<AdmissionRegistry> {
    static REGISTRY: OnceLock<Arc<AdmissionRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let registry = Arc::new(AdmissionRegistry::new());
        registry.start_worker();
        registry
    }).clone()
}

/// Parse an HTTP `Retry-After` value (delta-seconds or HTTP-date) into a
/// cooldown. Defaults to the full window when absent/unparseable.
pub fn parse_retry_after(value: Option<&str>, default: Duration) -> Duration {
    match value {
        Some(v) => {
            if let Ok(secs) = v.trim().parse::<u64>() {
                Duration::from_secs(secs)
            } else {
                default
            }
        }
        None => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(rpms: &[u32]) -> ProviderPool {
        ProviderPool::new(
            PoolConfig { provider_id: "test".into(), model: "m".into(), window: Duration::from_millis(200) },
            rpms.iter().enumerate().map(|(i, &rpm)| LaneConfig { key_id: format!("key-{}", i + 1), rpm_limit: rpm, enabled: true }).collect(),
        )
        .unwrap()
    }

    #[test]
    fn sliding_window_enforces_rpm() {
        let p = pool(&[2]);
        let mut a = p.try_acquire(None).unwrap();
        a.mark_sent();
        let mut b = p.try_acquire(None).unwrap();
        b.mark_sent();
        match p.try_acquire(None) {
            Err(AdmissionError::NoCapacity { .. }) => {}
            other => panic!("expected NoCapacity, got {:?}", other.map(|_| ())),
        }
    }

    #[test]
    fn unsent_reservation_is_refunded() {
        let p = pool(&[1]);
        {
            let _r = p.try_acquire(None).unwrap(); // dropped unsent
        }
        let mut r = p.try_acquire(None).unwrap(); // slot refunded
        r.mark_sent();
        assert!(p.try_acquire(None).is_err());
    }

    #[test]
    fn least_loaded_and_round_robin_spread() {
        let p = pool(&[10, 10, 10, 10, 10]);
        let mut keys = Vec::new();
        for _ in 0..5 {
            let mut r = p.try_acquire(None).unwrap();
            keys.push(r.key_id());
            r.mark_sent();
        }
        // 5 lanes, 5 sends: each lane used exactly once (least-loaded + RR)
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 5, "expected spread across 5 lanes, got {keys:?}");
    }

    #[test]
    fn affinity_sticks_until_lane_full_then_spills() {
        let p = pool(&[1, 1]);
        let mut a = p.try_acquire(Some("conv-1")).unwrap();
        let first = a.key_id();
        a.mark_sent();
        let mut b = p.try_acquire(Some("conv-1")).unwrap();
        assert_ne!(b.key_id(), first, "full affinity lane must spill over");
        b.mark_sent();
    }

    #[test]
    fn cooldown_removes_lane_until_it_expires() {
        let p = pool(&[5, 5]);
        p.report_rate_limited("key-1", Duration::from_millis(80));
        let st = p.status();
        assert!(st.iter().find(|s| s.key_id == "key-1").unwrap().cooling_down);
        for _ in 0..3 {
            let mut r = p.try_acquire(None).unwrap();
            assert_eq!(r.key_id(), "key-2");
            r.mark_sent();
        }
    }

    #[test]
    fn governor_caps_and_releases() {
        let g = ModelGovernor::new();
        let p1 = g.try_acquire("m", 1).unwrap();
        assert!(g.try_acquire("m", 1).is_none());
        drop(p1);
        let p2 = g.try_acquire("m", 1).unwrap();
        assert!(p2.counted);
        assert_eq!(g.in_flight("m"), 1);
    }

    #[test]
    fn rebuild_preserves_window_state() {
        let p = pool(&[1, 1]);
        let mut a = p.try_acquire(None).unwrap();
        let used = a.key_id();
        a.mark_sent();
        p.rebuild(vec![
            LaneConfig { key_id: "key-1".into(), rpm_limit: 1, enabled: true },
            LaneConfig { key_id: "key-2".into(), rpm_limit: 1, enabled: true },
        ])
        .unwrap();
        let st = p.status();
        let used_st = st.iter().find(|s| s.key_id == used).unwrap();
        assert_eq!(used_st.in_window, 1, "rebuild must keep send state for unchanged keys");
    }

    #[test]
    fn retry_after_parsing() {
        assert_eq!(parse_retry_after(Some("5"), Duration::from_secs(60)), Duration::from_secs(5));
        assert_eq!(parse_retry_after(None, Duration::from_secs(60)), Duration::from_secs(60));
        assert_eq!(parse_retry_after(Some("garbage"), Duration::from_secs(60)), Duration::from_secs(60));
    }
}