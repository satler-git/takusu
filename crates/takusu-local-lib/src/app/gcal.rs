//! Google Calendar sync, OAuth, and event cleanup (#11).
//!
//! Extracted from the `app.rs` god module. Holds the Google Calendar settings
//! wrappers, OAuth flow, `do_sync` (the bidirectional schedule ↔ calendar
//! synchroniser), and the explicit "delete all events" cleanup path.

use std::collections::HashMap;

use serde::Serialize;
use takusu_contracts::{
    GoogleCalEventRow, GoogleCalSettingsRow, ScheduleEntry, UpdateGoogleCalSettings,
};

use crate::error::storage_to_app;
use crate::error::{AppError, BadRequestKind};

/// Result of explicitly deleting every mapped Google Calendar event.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct DeleteAllGcalResult {
    pub deleted: usize,
    pub failed: Vec<DeleteAllGcalFailure>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct DeleteAllGcalFailure {
    pub task_id: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct GoogleCalSettingsOutput {
    pub enabled: bool,
    pub calendar_id: String,
    pub client_id: String,
    pub has_client_secret: bool,
    pub has_refresh_token: bool,
    pub reminder_minutes: Option<i64>,
    pub color_id: Option<i64>,
    pub visibility: Option<String>,
    pub transparency: Option<String>,
}

/// Response for `POST /api/sync/oauth/url`.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct OAuthUrlResponse {
    pub url: String,
}

/// Response for `POST /api/sync/oauth/callback`.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct OAuthCallbackResponse {
    pub refresh_token_set: bool,
}

/// Response for `POST /api/sync/trigger`.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SyncTriggerResponse {
    pub status: String,
}

impl super::TakusuApp {
    pub async fn get_gcal_settings(&self) -> Result<GoogleCalSettingsOutput, AppError> {
        let row = self
            .storage
            .get_gcal_settings()
            .await
            .map_err(storage_to_app)?;
        Ok(GoogleCalSettingsOutput {
            enabled: row.enabled,
            calendar_id: row.calendar_id,
            client_id: row.client_id,
            has_client_secret: !row.client_secret.is_empty(),
            has_refresh_token: row.refresh_token.is_some(),
            reminder_minutes: row.reminder_minutes,
            color_id: row.color_id,
            visibility: row.visibility.clone(),
            transparency: row.transparency.clone(),
        })
    }

    pub async fn update_gcal_settings(
        &self,
        body: &UpdateGoogleCalSettings,
    ) -> Result<GoogleCalSettingsOutput, AppError> {
        if let Some(Some(m)) = body.reminder_minutes
            && m < 0
        {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "reminder_minutes must be non-negative".into(),
            )));
        }
        if let Some(Some(c)) = body.color_id
            && !(1..=11).contains(&c)
        {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "color_id must be between 1 and 11".into(),
            )));
        }
        if let Some(Some(v)) = &body.visibility
            && !matches!(
                v.as_str(),
                "default" | "public" | "private" | "confidential"
            )
        {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "visibility must be one of: default, public, private, confidential".into(),
            )));
        }
        if let Some(Some(t)) = &body.transparency
            && !matches!(t.as_str(), "opaque" | "transparent")
        {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "transparency must be either opaque or transparent".into(),
            )));
        }
        let row = self
            .storage
            .update_gcal_settings(body)
            .await
            .map_err(storage_to_app)?;
        Ok(GoogleCalSettingsOutput {
            enabled: row.enabled,
            calendar_id: row.calendar_id,
            client_id: row.client_id,
            has_client_secret: !row.client_secret.is_empty(),
            has_refresh_token: row.refresh_token.is_some(),
            reminder_minutes: row.reminder_minutes,
            color_id: row.color_id,
            visibility: row.visibility.clone(),
            transparency: row.transparency.clone(),
        })
    }

    pub async fn oauth_url(&self, redirect_uri: &str) -> Result<String, AppError> {
        let row = self
            .storage
            .get_gcal_settings()
            .await
            .map_err(storage_to_app)?;
        if row.client_id.is_empty() {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "google calendar settings not configured".into(),
            )));
        }
        Ok(google_cal::oauth_url(&row.client_id, redirect_uri))
    }

    pub async fn oauth_callback(
        &self,
        code: &str,
        redirect_uri: Option<&str>,
    ) -> Result<(), AppError> {
        let row = self
            .storage
            .get_gcal_settings()
            .await
            .map_err(storage_to_app)?;
        if row.client_id.is_empty() || row.client_secret.is_empty() {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "google calendar settings not configured".into(),
            )));
        }
        let tokens =
            google_cal::exchange_code(&row.client_id, &row.client_secret, code, redirect_uri)
                .await
                .map_err(|e| AppError::Internal(format!("oauth exchange failed: {e}")))?;
        self.storage
            .update_gcal_settings(&UpdateGoogleCalSettings {
                enabled: None,
                calendar_id: None,
                client_id: None,
                client_secret: None,
                refresh_token: Some(tokens.refresh_token),
                reminder_minutes: None,
                color_id: None,
                visibility: None,
                transparency: None,
            })
            .await
            .map_err(storage_to_app)?;
        Ok(())
    }

    pub async fn list_gcal_mappings(&self) -> Result<Vec<GoogleCalEventRow>, AppError> {
        self.storage
            .list_gcal_mappings()
            .await
            .map_err(storage_to_app)
    }

    pub async fn do_sync(&self) -> Result<(), String> {
        let settings = self
            .storage
            .get_gcal_settings()
            .await
            .map_err(|e| e.to_string())?;
        let (
            refresh_token,
            client_id,
            client_secret,
            calendar_id,
            reminder_minutes,
            color_id,
            visibility,
            transparency,
        ) = match &settings {
            s if s.enabled && s.refresh_token.is_some() => (
                s.refresh_token.clone().unwrap(),
                s.client_id.clone(),
                s.client_secret.clone(),
                s.calendar_id.clone(),
                s.reminder_minutes.filter(|&m| m > 0),
                s.color_id.filter(|&c| (1..=11).contains(&c)),
                s.visibility.clone().filter(|v| {
                    matches!(
                        v.as_str(),
                        "default" | "public" | "private" | "confidential"
                    )
                }),
                s.transparency
                    .clone()
                    .filter(|t| matches!(t.as_str(), "opaque" | "transparent")),
            ),
            _ => return Ok(()),
        };
        let refresh_token = if refresh_token.is_empty() {
            return Ok(());
        } else {
            refresh_token
        };

        let schedule_row = self
            .storage
            .get_schedule()
            .await
            .map_err(|e| e.to_string())?;
        let entries: Option<Vec<ScheduleEntry>> =
            schedule_row.map(|s| s.schedule.as_inner().clone());

        let client = google_cal::Client::new(client_id, client_secret, refresh_token, calendar_id)
            .map_err(|e| e.to_string())?;

        match entries {
            Some(entries) => {
                let task_ids: Vec<String> = entries.iter().map(|e| e.task_id.clone()).collect();
                let mut titles: HashMap<String, (String, Option<String>)> = HashMap::new();
                for id in &task_ids {
                    if let Ok(t) = self.storage.get_task(id).await {
                        titles.insert(t.id, (t.title, t.description));
                    }
                }
                let db_mappings = self
                    .storage
                    .list_gcal_mappings()
                    .await
                    .map_err(|e| e.to_string())?;
                let existing: HashMap<String, String> = db_mappings
                    .iter()
                    .map(|m| (m.task_id.clone(), m.google_event_id.clone()))
                    .collect();

                let sync_entries: Vec<google_cal::SyncEntry> = entries
                    .iter()
                    .map(|e| {
                        let (summary, description) = titles
                            .get(&e.task_id)
                            .cloned()
                            .unwrap_or_else(|| (e.task_id.clone(), None));
                        google_cal::SyncEntry {
                            task_id: e.task_id.clone(),
                            summary,
                            description,
                            start: e.start_at.to_string(),
                            end: e.end_at.to_string(),
                            reminder_minutes,
                            color_id,
                            visibility: visibility.clone(),
                            transparency: transparency.clone(),
                        }
                    })
                    .collect();

                let result = client
                    .sync(&sync_entries, &existing)
                    .await
                    .map_err(|e| e.to_string())?;

                let deleted_task_ids: Vec<String> = result
                    .deleted
                    .iter()
                    .filter_map(|eid| {
                        db_mappings
                            .iter()
                            .find(|m| &m.google_event_id == eid)
                            .map(|m| m.task_id.clone())
                    })
                    .collect();
                self.storage
                    .upsert_gcal_mappings(&result.mappings)
                    .await
                    .map_err(|e| e.to_string())?;
                self.storage
                    .delete_gcal_mappings(&deleted_task_ids)
                    .await
                    .map_err(|e| e.to_string())?;
                tracing::info!(
                    "google calendar sync: created/updated {}, deleted {}",
                    result.mappings.len(),
                    deleted_task_ids.len()
                );
                if !result.failed.is_empty() {
                    let summary = result
                        .failed
                        .iter()
                        .map(|f| format!("{}({}): {}", f.task_id, f.operation, f.error))
                        .collect::<Vec<_>>()
                        .join("; ");
                    tracing::warn!(
                        "google calendar sync: {} failure(s): {summary}",
                        result.failed.len()
                    );
                    return Err(format!(
                        "google calendar sync partially failed: {} operation(s) could not complete — DB and Calendar may diverge",
                        result.failed.len()
                    ));
                }
                Ok(())
            }
            None => {
                tracing::info!("no active schedule, clearing google calendar events");
                let result = self
                    .delete_all_gcal_events_with_settings(&settings)
                    .await
                    .map_err(|e| e.to_string())?;
                tracing::info!(
                    "deleted {} google calendar event(s), {} failure(s)",
                    result.deleted,
                    result.failed.len()
                );
                if !result.failed.is_empty() {
                    let summary = result
                        .failed
                        .iter()
                        .map(|f| format!("{}: {}", f.task_id, f.error))
                        .collect::<Vec<_>>()
                        .join("; ");
                    return Err(format!(
                        "google calendar delete all partially failed: {} event(s) could not be deleted: {summary}",
                        result.failed.len()
                    ));
                }
                Ok(())
            }
        }
    }

    /// Delete all events that are mapped to the local schedule on Google
    /// Calendar, then remove the local mappings. This is useful when the
    /// calendar has drifted or the user wants to clean up imported events
    /// from the Google side (#598).
    pub async fn delete_all_gcal_events(&self) -> Result<DeleteAllGcalResult, AppError> {
        let settings = self
            .storage
            .get_gcal_settings()
            .await
            .map_err(storage_to_app)?;
        self.delete_all_gcal_events_with_settings(&settings).await
    }

    /// Shared implementation used by the explicit delete command and the
    /// "no active schedule" sync cleanup path.
    async fn delete_all_gcal_events_with_settings(
        &self,
        settings: &GoogleCalSettingsRow,
    ) -> Result<DeleteAllGcalResult, AppError> {
        if settings.client_id.is_empty() {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "google calendar client_id not configured".into(),
            )));
        }
        if settings.client_secret.is_empty() {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "google calendar client_secret not configured".into(),
            )));
        }
        let refresh_token = settings
            .refresh_token
            .as_deref()
            .filter(|t| !t.is_empty())
            .ok_or_else(|| {
                AppError::BadRequest(BadRequestKind::Other(
                    "google calendar refresh token not configured".into(),
                ))
            })?;

        let mappings = self
            .storage
            .list_gcal_mappings()
            .await
            .map_err(storage_to_app)?;
        if mappings.is_empty() {
            return Ok(DeleteAllGcalResult {
                deleted: 0,
                failed: vec![],
            });
        }

        let client = google_cal::Client::new(
            settings.client_id.clone(),
            settings.client_secret.clone(),
            refresh_token.to_string(),
            settings.calendar_id.clone(),
        )
        .map_err(|e| AppError::Internal(format!("failed to create google calendar client: {e}")))?;

        let task_event_pairs: Vec<(String, String)> = mappings
            .iter()
            .map(|m| (m.task_id.clone(), m.google_event_id.clone()))
            .collect();

        let result = client.delete_all(&task_event_pairs).await.map_err(|e| {
            AppError::Internal(format!("failed to delete google calendar events: {e}"))
        })?;

        self.storage
            .delete_gcal_mappings(&result.deleted)
            .await
            .map_err(storage_to_app)?;

        Ok(DeleteAllGcalResult {
            deleted: result.deleted.len(),
            failed: result
                .failed
                .into_iter()
                .map(|f| DeleteAllGcalFailure {
                    task_id: f.task_id,
                    error: f.error,
                })
                .collect(),
        })
    }
}
