//! Work session lifecycle (#1393).
//!
//! Work sessions are top-level entities. A session may be started with or
//! without a task, and can be attached to a task or converted into one later.

use takusu_contracts::{
    AttachWorkSession, ConvertWorkSession, RecordWorkSessionProgress, StartWorkSession,
    TaskRow, WorkSessionProgressResult, WorkSessionRow,
};

use crate::error::storage_to_app;
use crate::error::{AppError, BadRequestKind};

impl super::TakusuApp {
    pub async fn start_work_session(
        &self,
        body: &StartWorkSession,
        operation_id: Option<&str>,
    ) -> Result<WorkSessionRow, AppError> {
        self.storage
            .start_work_session(body, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn pause_work_session(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> Result<WorkSessionRow, AppError> {
        self.storage
            .pause_work_session(id, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn complete_work_session(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> Result<WorkSessionRow, AppError> {
        self.storage
            .complete_work_session(id, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn record_work_session_progress(
        &self,
        id: &str,
        body: &RecordWorkSessionProgress,
        operation_id: Option<&str>,
    ) -> Result<WorkSessionProgressResult, AppError> {
        self.storage
            .record_work_session_progress(id, body, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn get_work_session(&self, id: &str) -> Result<WorkSessionRow, AppError> {
        self.storage.get_work_session(id).await.map_err(storage_to_app)
    }

    pub async fn list_work_sessions(
        &self,
        task_id: Option<&str>,
    ) -> Result<Vec<WorkSessionRow>, AppError> {
        self.storage
            .list_work_sessions(task_id)
            .await
            .map_err(storage_to_app)
    }

    /// Return the single open work session for a task, or `None` if there is
    /// none. Returns a bad-request error if more than one open session is
    /// found, which indicates a data-integrity violation.
    pub async fn open_work_session_for_task(
        &self,
        task_id: &str,
    ) -> Result<Option<WorkSessionRow>, AppError> {
        let sessions = self.list_work_sessions(Some(task_id)).await?;
        let open: Vec<_> = sessions.into_iter().filter(|s| s.ended_at.is_none()).collect();
        if open.len() > 1 {
            return Err(AppError::BadRequest(BadRequestKind::Other(format!(
                "multiple open work sessions for task {task_id}"
            ))));
        }
        Ok(open.into_iter().next())
    }

    pub async fn attach_work_session(
        &self,
        id: &str,
        body: &AttachWorkSession,
        operation_id: Option<&str>,
    ) -> Result<WorkSessionRow, AppError> {
        self.storage
            .attach_work_session(id, body, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn convert_work_session(
        &self,
        id: &str,
        body: &ConvertWorkSession,
        operation_id: Option<&str>,
    ) -> Result<TaskRow, AppError> {
        self.storage
            .convert_work_session(id, body, operation_id)
            .await
            .map_err(storage_to_app)
    }
}
