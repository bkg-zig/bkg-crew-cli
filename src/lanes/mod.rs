//! Lane routing and dispatcher traits for provider pool management.

use std::sync::Arc;
use std::time::Duration;
use crate::admission::{ProviderPool, Reservation, ModelPermit};
use crate::error::Result;

/// Trait for lane selection policies.
#[async_trait::async_trait]
pub trait LaneRouter: Send + Sync {
    /// Select a lane for the given request.
    async fn select_lane(&self, pool: &ProviderPool, affinity_key: Option<&str>) -> Result<usize>;
}

/// Trait for dispatching requests through lanes.
#[async_trait::async_trait]
pub trait Dispatcher: Send + Sync {
    /// Dispatch a request through the admission system.
    async fn dispatch(&self, provider_id: &str, model: &str, max_wait: Duration) -> Result<DispatchResult>;
}

/// Result of a successful dispatch.
pub struct DispatchResult {
    pub pool: ProviderPool,
    pub lane_index: usize,
    pub reservation: Reservation,
    pub permit: ModelPermit,
}

impl std::fmt::Debug for DispatchResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatchResult")
            .field("lane_index", &self.lane_index)
            .finish_non_exhaustive()
    }
}

impl Clone for DispatchResult {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            lane_index: self.lane_index,
            reservation: Reservation {
                inner: self.reservation.inner.clone(),
                lane_index: self.reservation.lane_index,
                at: self.reservation.at,
                sent: self.reservation.sent,
            },
            permit: ModelPermit {
                inner: self.permit.inner.clone(),
                model: self.permit.model.clone(),
                counted: self.permit.counted,
            },
        }
    }
}

/// Default lane router implementing the selection policy from the design doc.
/// Policy: 1) Ready affinity lane, 2) Least normalized load with round-robin tie-break, 3) Spillover.
pub struct DefaultLaneRouter;

#[async_trait::async_trait]
impl LaneRouter for DefaultLaneRouter {
    async fn select_lane(&self, pool: &ProviderPool, affinity_key: Option<&str>) -> Result<usize> {
        let reservation = pool.try_acquire(affinity_key)?;
        let lane_index = reservation.lane_index;
        Ok(lane_index)
    }
}

/// Default dispatcher with FIFO queue, deadline enforcement, and cancellation support.
pub struct DefaultDispatcher {
    registry: Arc<crate::admission::AdmissionRegistry>,
}

impl DefaultDispatcher {
    pub fn new(registry: Arc<crate::admission::AdmissionRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl Dispatcher for DefaultDispatcher {
    async fn dispatch(&self, provider_id: &str, model: &str, max_wait: Duration) -> Result<DispatchResult> {
        // For the dispatcher, we need a key_id and rpm_limit - these would come from pool config
        // Using the registry's pool_for to get/create the pool
        let pool = self.registry.pool_for(provider_id, model, "default", 40);
        let key_id = "default";
        let rpm_limit = 40;
        let model_max_concurrency = 10;

        let (reservation, permit) = self.registry.acquire(
            provider_id, model, key_id, rpm_limit, model_max_concurrency, max_wait
        ).await?;

        let lane_index = reservation.lane_index;
        Ok(DispatchResult { pool, lane_index, reservation, permit })
    }
}

/// Builder for creating dispatcher with pool configuration.
pub struct DispatcherBuilder {
    registry: Arc<crate::admission::AdmissionRegistry>,
}

impl DispatcherBuilder {
    pub fn new(registry: Arc<crate::admission::AdmissionRegistry>) -> Self {
        Self { registry }
    }

    pub fn build(self) -> DefaultDispatcher {
        DefaultDispatcher::new(self.registry)
    }
}