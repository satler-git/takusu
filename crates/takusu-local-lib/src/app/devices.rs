//! Multi-device arbitration and device lifecycle operations (WI-11).

use takusu_contracts::{
    CreateDevice, DeviceRow, RefreshEvaluatorHeartbeat, RefreshEvaluatorLease, ResidentAuthority,
    SpeechCapability, UpdateDevice,
};
use takusu_types::Timestamp;

use super::TakusuApp;
use crate::error::{AppError, storage_to_app};
use crate::validate::Validate;

impl TakusuApp {
    pub async fn register_device(&self, body: &CreateDevice) -> Result<DeviceRow, AppError> {
        body.validate()?;
        self.storage
            .register_device(body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn get_device(&self, id: &str) -> Result<DeviceRow, AppError> {
        self.storage.get_device(id).await.map_err(storage_to_app)
    }

    pub async fn list_devices(&self) -> Result<Vec<DeviceRow>, AppError> {
        self.storage.list_devices().await.map_err(storage_to_app)
    }

    pub async fn update_device(
        &self,
        id: &str,
        body: &UpdateDevice,
    ) -> Result<DeviceRow, AppError> {
        body.validate()?;
        self.storage
            .update_device(id, body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn delete_device(&self, id: &str) -> Result<(), AppError> {
        self.storage.delete_device(id).await.map_err(storage_to_app)
    }

    /// Extend the device's contact suppression window by `minutes` from now.
    pub async fn suppress_device(&self, id: &str, minutes: i64) -> Result<DeviceRow, AppError> {
        let now = Timestamp::from(jiff::Timestamp::now());
        let until = Timestamp::from_second(
            now.as_second().saturating_add(minutes.saturating_mul(60)),
        )
        .unwrap_or(now);
        let body = UpdateDevice {
            contact_suppress_until: Some(until),
            ..Default::default()
        };
        self.update_device(id, &body).await
    }

    pub async fn refresh_evaluator_heartbeat(
        &self,
        body: &RefreshEvaluatorHeartbeat,
    ) -> Result<DeviceRow, AppError> {
        self.storage
            .refresh_evaluator_heartbeat(&body.device_id, body.until)
            .await
            .map_err(storage_to_app)
    }

    pub async fn refresh_evaluator_lease(
        &self,
        body: &RefreshEvaluatorLease,
    ) -> Result<DeviceRow, AppError> {
        self.storage
            .refresh_evaluator_lease(&body.device_id, body.lease_until, body.next_eval_at)
            .await
            .map_err(storage_to_app)
    }

    pub async fn resolve_resident_authority(
        &self,
        candidate_id: &str,
    ) -> Result<ResidentAuthority, AppError> {
        self.storage
            .resolve_resident_authority(candidate_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn get_speech_capability(
        &self,
        device_id: &str,
    ) -> Result<SpeechCapability, AppError> {
        let device = self
            .storage
            .get_device(device_id)
            .await
            .map_err(storage_to_app)?;
        // `can_speak_proactively` is the physical ability to emit proactive
        // speech. The private-channel / ongoing-conversation privacy gate is
        // applied by `delivery_mode_for`, not here.
        Ok(SpeechCapability {
            can_speak_proactively: matches!(
                device.platform,
                takusu_contracts::DevicePlatform::Desktop
            ) || device.audio_service_running,
        })
    }
}
