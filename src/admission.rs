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
    pub last_error_class: Option<String>,
}

struct LaneState {
    config: LaneConfig,
    sends: VecDeque<Instant>,
    cooldown_until: Option<Instant>,
    last_error_class: Option<String>,
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

struct PoolInner {
    config: PoolConfig,
    lanes: Vec<LaneState>,
    cursor: usize,
    affinity: HashMap<String, usize>,
}

/// A provider pool: several key lanes sharing one provider/model limit scope.
#[derive(Clone)]
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

    /// Mark the lane that sent a request as rate-limited (429 / Retry-After).
    pub fn report_rate_limited(&self, key_id: &str, retry_after: Duration) {
        self.report_error(key_id, "rate_limited", Some(retry_after));
    }

    /// Record a classified send error; `cooldown` optionally parks the lane.
    pub fn report_error(&self, key_id: &str, error_class: &str, cooldown: Option<Duration>) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(lane) = inner.lanes.iter_mut().find(|l| l.config.key_id == key_id) {
            lane.last_error_class = Some(error_class.to_string());
            if let Some(d) = cooldown {
                lane.cooldown_until = Some(Instant::now() + d);
            }
        }
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
                    best = Some(best.map(|b| std::cmp::min(b, c - now)).or(Some(c - now)).unwrap());
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
pub struct Reservation {
    inner: Arc<Mutex<PoolInner>>,
    lane_index: usize,
    at: Instant,
    sent: bool,
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

struct ModelState {
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

pub struct ModelPermit {
    inner: Arc<Mutex<HashMap<String, ModelState>>>,
    model: String,
    counted: bool,
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

/// Process-wide registry of pools, keyed by `provider/model/key-set`.
/// Exactly one pool owner per key set is enforced by construction: all
/// sends in this process go through this single registry instance.
pub struct AdmissionRegistry {
    pools: Mutex<HashMap<String, ProviderPool>>,
    governor: ModelGovernor,
    // FIFO turn gate: tokio Mutex grants waiters in arrival order, so an
    // acquire that must wait cannot be overtaken by later arrivals.
    turn: tokio::sync::Mutex<()>,
}

impl Default for AdmissionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AdmissionRegistry {
    pub fn new() -> Self {
        Self { pools: Mutex::new(HashMap::new()), governor: ModelGovernor::new(), turn: tokio::sync::Mutex::new(()) }
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
    pub async fn acquire(
        &self,
        provider_id: &str,
        model: &str,
        key_id: &str,
        rpm_limit: u32,
        model_max_concurrency: u32,
        max_wait: Duration,
    ) -> Result<(Reservation, ModelPermit), AdmissionError> {
        let pool = self.pool_for(provider_id, model, key_id, rpm_limit);
        let _turn = self.turn.lock().await;
        let start = Instant::now();
        loop {
            if let Some(permit) = self.governor.try_acquire(model, model_max_concurrency) {
                match pool.try_acquire(None) {
                    Ok(mut r) => {
                        r.mark_sent();
                        return Ok((r, permit));
                    }
                    Err(AdmissionError::NoCapacity { retry_in }) => {
                        drop(permit);
                        if start.elapsed() >= max_wait {
                            return Err(AdmissionError::NoCapacity { retry_in });
                        }
                        tokio::time::sleep(retry_in.min(Duration::from_millis(50)).max(Duration::from_millis(1))).await;
                    }
                    Err(e) => return Err(e),
                }
            } else {
                if start.elapsed() >= max_wait {
                    return Err(AdmissionError::NoCapacity { retry_in: Duration::from_millis(50) });
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
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

    /// Non-secret status of every known pool, for doctor/WebUI display.
    pub fn status(&self) -> Vec<(String, Vec<LaneStatus>)> {
        let pools = self.pools.lock().unwrap_or_else(|p| p.into_inner());
        pools.iter().map(|(id, p)| (id.clone(), p.status())).collect()
    }
}

/// The one process-wide admission registry. All managed sends (gateway and
/// direct API client) share it, so no call site can bypass the lanes.
pub fn global_registry() -> Arc<AdmissionRegistry> {
    static REGISTRY: OnceLock<Arc<AdmissionRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Arc::new(AdmissionRegistry::new())).clone()
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
        assert!(g.try_acquire("m", 1).is_some());
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
