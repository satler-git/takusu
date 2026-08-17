//! # google-cal — Google Calendar API client
//!
//! OAuth2認証とGoogle Calendarイベントの同期を提供する。
//! イベント同期は差分ベース: 既存マッピングと比較して
//! 新規作成・更新・削除を行う。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_CALENDAR_BASE: &str = "https://www.googleapis.com/calendar/v3";
const GOOGLE_BATCH_URL: &str = "https://www.googleapis.com/batch/calendar/v3";
// Google returns 410 Gone when the event was already deleted on their side.
const ALREADY_DELETED: u16 = 410;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("OAuth2 error: {0}")]
    OAuth2(String),
    #[error("API error: {0}")]
    Api(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: i64,
    refresh_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEntry {
    pub task_id: String,
    pub summary: String,
    pub description: Option<String>,
    pub start: String,
    pub end: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reminder_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transparency: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncResult {
    pub mappings: Vec<(String, String)>,
    pub deleted: Vec<String>,
    /// Tasks whose create/update/delete on Google Calendar failed.
    /// Non-empty when the DB and Calendar may have diverged (#279).
    pub failed: Vec<SyncFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteAllResult {
    pub deleted: Vec<String>,
    pub failed: Vec<SyncFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncFailure {
    pub task_id: String,
    pub operation: String,
    pub error: String,
}

pub struct Client {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
    refresh_token: String,
    calendar_id: String,
    token_url: String,
    batch_url: String,
}

pub fn oauth_url(client_id: &str, redirect_uri: &str) -> String {
    format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope=https://www.googleapis.com/auth/calendar.events&access_type=offline&prompt=consent",
        GOOGLE_AUTH_URL,
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
    )
}

pub async fn exchange_code(
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: Option<&str>,
) -> Result<OAuthTokens> {
    let http = takusu_client::default_http_client(None)?;
    let mut form = vec![
        ("code", code.to_string()),
        ("client_id", client_id.to_string()),
        ("client_secret", client_secret.to_string()),
        ("grant_type", "authorization_code".to_string()),
    ];
    if let Some(uri) = redirect_uri {
        form.push(("redirect_uri", uri.to_string()));
    }
    let resp = http.post(GOOGLE_TOKEN_URL).form(&form).send().await?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::OAuth2(body));
    }

    let token: TokenResponse = resp.json().await?;
    let refresh_token = token
        .refresh_token
        .ok_or_else(|| Error::OAuth2("token response did not include a refresh_token".into()))?;
    Ok(OAuthTokens {
        access_token: token.access_token,
        refresh_token,
        expires_in: token.expires_in,
    })
}

mod urlencoding {
    pub fn encode(s: &str) -> String {
        s.bytes()
            .flat_map(|b| match b {
                b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'-' | b'_' | b'.' | b'~' => {
                    vec![b as char]
                }
                _ => format!("%{b:02X}").chars().collect(),
            })
            .collect()
    }
}

impl Client {
    pub fn new(
        client_id: String,
        client_secret: String,
        refresh_token: String,
        calendar_id: String,
    ) -> Result<Self> {
        Ok(Self {
            http: takusu_client::default_http_client(None)?,
            client_id,
            client_secret,
            refresh_token,
            calendar_id,
            token_url: GOOGLE_TOKEN_URL.to_string(),
            batch_url: GOOGLE_BATCH_URL.to_string(),
        })
    }

    async fn refresh_access_token(&self) -> Result<String> {
        let resp = self
            .http
            .post(&self.token_url)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", self.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::OAuth2(body));
        }

        let token: TokenResponse = resp.json().await?;
        Ok(token.access_token)
    }

    fn events_url(&self, suffix: &str) -> String {
        format!(
            "{}/calendars/{}/{}",
            GOOGLE_CALENDAR_BASE,
            urlencoding::encode(&self.calendar_id),
            suffix,
        )
    }

    fn event_url(&self, event_id: &str) -> String {
        format!(
            "{}/calendars/{}/events/{}",
            GOOGLE_CALENDAR_BASE,
            urlencoding::encode(&self.calendar_id),
            urlencoding::encode(event_id),
        )
    }

    fn event_body(&self, entry: &SyncEntry) -> serde_json::Value {
        let mut body = serde_json::json!({
            "summary": entry.summary,
            "start": { "dateTime": entry.start },
            "end": { "dateTime": entry.end },
            "extendedProperties": {
                "private": {
                    "takusuTaskId": entry.task_id,
                }
            },
        });
        if let Some(desc) = &entry.description {
            body["description"] = serde_json::Value::String(desc.clone());
        }
        if let Some(minutes) = entry.reminder_minutes.filter(|&m| m > 0) {
            body["reminders"] = serde_json::json!({
                "useDefault": false,
                "overrides": [
                    { "method": "popup", "minutes": minutes }
                ]
            });
        }
        if let Some(color_id) = entry.color_id.filter(|&c| (1..=11).contains(&c)) {
            body["colorId"] = serde_json::Value::String(color_id.to_string());
        }
        if let Some(visibility) = &entry.visibility
            && matches!(visibility.as_str(), "default" | "public" | "private" | "confidential")
        {
            body["visibility"] = serde_json::Value::String(visibility.clone());
        }
        if let Some(transparency) = &entry.transparency
            && matches!(transparency.as_str(), "opaque" | "transparent")
        {
            body["transparency"] = serde_json::Value::String(transparency.clone());
        }
        body
    }

    async fn create_event(&self, token: &str, entry: &SyncEntry) -> Result<String> {
        let url = self.events_url("events");
        let body = self.event_body(entry);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::Api(format!("create event ({status}): {text}")));
        }

        let result: serde_json::Value = resp.json().await?;
        result["id"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| Error::Api("missing event id in response".into()))
    }

    async fn delete_event(&self, token: &str, event_id: &str) -> Result<()> {
        let url = self.event_url(event_id);
        let resp = self.http.delete(&url).bearer_auth(token).send().await?;

        if !resp.status().is_success() && resp.status().as_u16() != ALREADY_DELETED {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::Api(format!("delete event ({status}): {text}")));
        }

        Ok(())
    }

    pub async fn sync(
        &self,
        entries: &[SyncEntry],
        existing: &HashMap<String, String>,
    ) -> Result<SyncResult> {
        let token = self.refresh_access_token().await?;
        let mut mappings = Vec::new();
        let mut deleted = Vec::new();
        let mut failed = Vec::new();

        let entry_ids: Vec<&str> = entries.iter().map(|e| e.task_id.as_str()).collect();

        let mut ops: Vec<BatchOp> = Vec::new();
        for (task_id, event_id) in existing {
            if !entry_ids.contains(&task_id.as_str()) {
                ops.push(BatchOp::Delete { task_id, event_id });
            }
        }
        for entry in entries {
            if let Some(event_id) = existing.get(&entry.task_id) {
                ops.push(BatchOp::Update {
                    task_id: &entry.task_id,
                    event_id,
                    entry,
                });
            } else {
                ops.push(BatchOp::Create {
                    task_id: &entry.task_id,
                    entry,
                });
            }
        }

        let results = self.batch_execute(&token, &ops).await?;
        for (i, result) in results.iter().enumerate() {
            let idx = op_index_from_content_id(&result.content_id).unwrap_or(i);
            let op = ops
                .get(idx)
                .ok_or_else(|| Error::Api(format!("batch response index {idx} out of bounds")))?;
            match op {
                BatchOp::Delete { task_id, event_id } => {
                    if result.status == 200
                        || result.status == 204
                        || result.status == ALREADY_DELETED
                    {
                        deleted.push(event_id.to_string());
                    } else {
                        tracing::warn!("failed to delete event {event_id}: {}", result.status);
                        failed.push(SyncFailure {
                            task_id: task_id.to_string(),
                            operation: "delete".to_string(),
                            error: format!("batch delete ({}): {}", result.status, result.body),
                        });
                    }
                }
                BatchOp::Update {
                    task_id,
                    event_id,
                    entry,
                } => {
                    if result.status == 200 {
                        if let Some(id) = parse_event_id(&result.body) {
                            mappings.push((task_id.to_string(), id));
                        } else {
                            failed.push(SyncFailure {
                                task_id: task_id.to_string(),
                                operation: "update".to_string(),
                                error: format!("missing event id in response: {}", result.body),
                            });
                        }
                    } else {
                        tracing::warn!(
                            "failed to update event {event_id} for task {task_id}: {}, trying delete+create",
                            result.status
                        );
                        if let Err(de) = self.delete_event(&token, event_id).await {
                            tracing::warn!(
                                "best-effort delete of orphaned event {event_id} failed: {de}"
                            );
                        }
                        match self.create_event(&token, entry).await {
                            Ok(id) => mappings.push((task_id.to_string(), id)),
                            Err(e2) => {
                                failed.push(SyncFailure {
                                    task_id: task_id.to_string(),
                                    operation: "update+create".to_string(),
                                    error: format!("batch update ({}); then {e2}", result.status),
                                });
                            }
                        }
                    }
                }
                BatchOp::Create { task_id, entry: _ } => {
                    if result.status == 200 {
                        if let Some(id) = parse_event_id(&result.body) {
                            mappings.push((task_id.to_string(), id));
                        } else {
                            failed.push(SyncFailure {
                                task_id: task_id.to_string(),
                                operation: "create".to_string(),
                                error: format!("missing event id in response: {}", result.body),
                            });
                        }
                    } else {
                        failed.push(SyncFailure {
                            task_id: task_id.to_string(),
                            operation: "create".to_string(),
                            error: format!("batch create ({}): {}", result.status, result.body),
                        });
                    }
                }
            }
        }

        Ok(SyncResult {
            mappings,
            deleted,
            failed,
        })
    }

    pub async fn delete_all(
        &self,
        task_event_pairs: &[(String, String)],
    ) -> Result<DeleteAllResult> {
        let token = self.refresh_access_token().await?;

        let ops: Vec<BatchOp> = task_event_pairs
            .iter()
            .map(|(task_id, event_id)| BatchOp::Delete {
                task_id: task_id.as_str(),
                event_id: event_id.as_str(),
            })
            .collect();

        let results = self.batch_execute(&token, &ops).await?;
        let mut deleted = Vec::new();
        let mut failed = Vec::new();
        for (i, result) in results.iter().enumerate() {
            let idx = op_index_from_content_id(&result.content_id).unwrap_or(i);
            if let Some(BatchOp::Delete { task_id, .. }) = ops.get(idx) {
                if result.status == 200 || result.status == 204 || result.status == ALREADY_DELETED
                {
                    deleted.push(task_id.to_string());
                } else {
                    tracing::warn!(
                        "failed to delete event for task {task_id}: {} {}",
                        result.status,
                        result.body
                    );
                    failed.push(SyncFailure {
                        task_id: task_id.to_string(),
                        operation: "delete".to_string(),
                        error: format!("batch delete ({}): {}", result.status, result.body),
                    });
                }
            }
        }

        Ok(DeleteAllResult { deleted, failed })
    }

    async fn batch_execute(&self, token: &str, ops: &[BatchOp<'_>]) -> Result<Vec<BatchResult>> {
        const BATCH_MAX: usize = 1000;
        if ops.is_empty() {
            return Ok(Vec::new());
        }

        let mut all_results = Vec::with_capacity(ops.len());
        let mut start_idx = 0;
        for chunk in ops.chunks(BATCH_MAX) {
            let (boundary, body) = self.build_batch_body(chunk, start_idx)?;
            let resp = self
                .http
                .post(&self.batch_url)
                .bearer_auth(token)
                .header(
                    "Content-Type",
                    format!("multipart/mixed; boundary={boundary}"),
                )
                .body(body)
                .send()
                .await?;

            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                return Err(Error::Api(format!("batch request ({status}): {text}")));
            }

            let content_type = resp
                .headers()
                .get("content-type")
                .and_then(|h| h.to_str().ok())
                .unwrap_or("");
            let response_boundary = parse_boundary(content_type)
                .ok_or_else(|| Error::Api("missing boundary in batch response".into()))?;
            let text = resp.text().await?;
            let results = parse_batch_response(&text, &response_boundary)?;
            all_results.extend(results);
            start_idx += chunk.len();
        }

        Ok(all_results)
    }

    fn build_batch_body(&self, ops: &[BatchOp<'_>], start_idx: usize) -> Result<(String, String)> {
        let boundary = format!("batch_{:032x}", rand::random::<u128>());
        let mut body = String::new();

        for (idx, op) in ops.iter().enumerate() {
            let content_id = format!("item-{}", start_idx + idx);
            let inner = self.build_inner_request(op)?;
            body.push_str("--");
            body.push_str(&boundary);
            body.push_str("\r\n");
            body.push_str("Content-Type: application/http\r\n");
            body.push_str("Content-ID: ");
            body.push_str(&content_id);
            body.push_str("\r\n\r\n");
            body.push_str(&inner);
            body.push_str("\r\n");
        }

        body.push_str("--");
        body.push_str(&boundary);
        body.push_str("--\r\n");

        Ok((boundary, body))
    }

    fn build_inner_request(&self, op: &BatchOp<'_>) -> Result<String> {
        let calendar = urlencoding::encode(&self.calendar_id);
        match op {
            BatchOp::Delete { event_id, .. } => {
                let path = format!(
                    "/calendar/v3/calendars/{calendar}/events/{}",
                    urlencoding::encode(event_id)
                );
                Ok(format!("DELETE {path} HTTP/1.1\r\n"))
            }
            BatchOp::Update {
                event_id, entry, ..
            } => {
                let path = format!(
                    "/calendar/v3/calendars/{calendar}/events/{}",
                    urlencoding::encode(event_id)
                );
                let body = serde_json::to_string(&self.event_body(entry))
                    .map_err(|e| Error::Api(e.to_string()))?;
                Ok(format!(
                    "PUT {path} HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                ))
            }
            BatchOp::Create { entry, .. } => {
                let path = format!("/calendar/v3/calendars/{calendar}/events");
                let body = serde_json::to_string(&self.event_body(entry))
                    .map_err(|e| Error::Api(e.to_string()))?;
                Ok(format!(
                    "POST {path} HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                ))
            }
        }
    }
}

enum BatchOp<'a> {
    Delete {
        task_id: &'a str,
        event_id: &'a str,
    },
    Update {
        task_id: &'a str,
        event_id: &'a str,
        entry: &'a SyncEntry,
    },
    Create {
        task_id: &'a str,
        entry: &'a SyncEntry,
    },
}

struct BatchResult {
    content_id: String,
    status: u16,
    body: String,
}

fn parse_event_id(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["id"].as_str().map(|s| s.to_string()))
}

fn op_index_from_content_id(content_id: &str) -> Option<usize> {
    let trimmed = content_id.trim().trim_matches(|c| c == '<' || c == '>');
    let s = trimmed.strip_prefix("response-").unwrap_or(trimmed);
    s.strip_prefix("item-")
        .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|s| s.parse().ok())
}

fn parse_boundary(content_type: &str) -> Option<String> {
    content_type
        .split(';')
        .map(str::trim)
        .find(|part| part.starts_with("boundary="))
        .map(|part| {
            part.trim_start_matches("boundary=")
                .trim_matches('"')
                .to_string()
        })
}

fn parse_batch_response(body: &str, boundary: &str) -> Result<Vec<BatchResult>> {
    let delimiter = format!("--{boundary}");
    let mut results = Vec::new();

    for (i, part) in body.split(&delimiter).enumerate() {
        if i == 0 {
            // Everything before the first boundary marker is preamble and should be empty.
            continue;
        }
        let part = part.strip_prefix("\r\n").unwrap_or(part);
        let part = part.strip_suffix("\r\n").unwrap_or(part);
        if part == "--" || part.is_empty() {
            // Final boundary marker (`--boundary--`) or trailing empty part.
            continue;
        }
        results.push(parse_batch_part(part)?);
    }

    Ok(results)
}

fn parse_batch_part(part: &str) -> Result<BatchResult> {
    let (part_headers, http_response) = split_once_crlf2(part).ok_or_else(|| {
        Error::Api("malformed batch part: missing part header/body separator".into())
    })?;
    let content_id = part_headers
        .lines()
        .find(|line| line.trim().starts_with("Content-ID") || line.trim().starts_with("content-id"))
        .and_then(|line| line.split_once(':'))
        .map(|(_, value)| {
            value
                .trim()
                .trim_matches(|c| c == '<' || c == '>')
                .strip_prefix("response-")
                .unwrap_or(value.trim())
                .to_string()
        })
        .unwrap_or_default();

    let (status, body) = parse_inner_http_response(http_response)?;
    Ok(BatchResult {
        content_id,
        status,
        body,
    })
}

fn parse_inner_http_response(response: &str) -> Result<(u16, String)> {
    let buf = response.as_bytes();
    let mut headers = [httparse::EMPTY_HEADER; 128];
    let mut resp = httparse::Response::new(&mut headers);
    let body_offset = match resp.parse(buf).map_err(|e| Error::Api(e.to_string()))? {
        httparse::Status::Complete(idx) => idx,
        httparse::Status::Partial => {
            return Err(Error::Api("incomplete inner HTTP response".into()));
        }
    };
    let status = resp
        .code
        .ok_or_else(|| Error::Api("missing status code in inner HTTP response".into()))?;

    let content_length = resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-length"))
        .and_then(|h| std::str::from_utf8(h.value).ok())
        .and_then(|v| v.trim().parse::<usize>().ok());

    let body = &response[body_offset..];
    let body = if let Some(len) = content_length {
        body.as_bytes()
            .get(..len)
            .map(|b| std::str::from_utf8(b).unwrap_or(body).to_string())
            .unwrap_or_else(|| body.to_string())
    } else {
        body.to_string()
    };

    Ok((status, body))
}

fn split_once_crlf2(s: &str) -> Option<(&str, &str)> {
    if let Some(pos) = s.find("\r\n\r\n") {
        return Some((&s[..pos], &s[pos + 4..]));
    }
    s.find("\n\n").map(|pos| (&s[..pos], &s[pos + 2..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_batch_response_with_create_and_delete() {
        let boundary = "abc";
        let body = concat!(
            "--abc\r\n",
            "Content-Type: application/http\r\n",
            "Content-ID: response-item-0\r\n",
            "\r\n",
            "HTTP/1.1 200 OK\r\n",
            "Content-Type: application/json\r\n",
            "Content-Length: 10\r\n",
            "\r\n",
            "{\"id\":\"x\"}\r\n",
            "--abc\r\n",
            "Content-Type: application/http\r\n",
            "Content-ID: response-item-1\r\n",
            "\r\n",
            "HTTP/1.1 204 No Content\r\n",
            "\r\n",
            "\r\n",
            "--abc--\r\n"
        );

        let results = parse_batch_response(body, boundary).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].content_id, "item-0");
        assert_eq!(results[0].status, 200);
        assert_eq!(results[0].body, "{\"id\":\"x\"}");
        assert_eq!(results[1].content_id, "item-1");
        assert_eq!(results[1].status, 204);
        assert_eq!(results[1].body, "");
    }

    #[test]
    fn parse_inner_http_response_without_content_length() {
        let response = "HTTP/1.1 410 Gone\r\n\r\n";
        let (status, body) = parse_inner_http_response(response).unwrap();
        assert_eq!(status, 410);
        assert_eq!(body, "");
    }

    #[test]
    fn parse_boundary_with_quotes() {
        assert_eq!(
            parse_boundary("multipart/mixed; boundary=\"foo\""),
            Some("foo".to_string())
        );
        assert_eq!(
            parse_boundary("multipart/mixed; boundary=bar"),
            Some("bar".to_string())
        );
    }

    #[test]
    fn op_index_from_content_id_variants() {
        assert_eq!(op_index_from_content_id("item-0"), Some(0));
        assert_eq!(op_index_from_content_id("response-item-1"), Some(1));
        assert_eq!(op_index_from_content_id("<response-item-2>"), Some(2));
        assert_eq!(op_index_from_content_id("item-3@example.com"), Some(3));
        assert_eq!(
            op_index_from_content_id("<response-item-4@example.com>"),
            Some(4)
        );
        assert_eq!(op_index_from_content_id(""), None);
    }

    #[tokio::test]
    async fn delete_all_returns_deleted_task_ids() {
        let base_url = start_mock_google_server(vec![204, 204]).await;
        let client = Client {
            http: reqwest::Client::new(),
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            refresh_token: "refresh".to_string(),
            calendar_id: "primary".to_string(),
            token_url: format!("{base_url}/token"),
            batch_url: format!("{base_url}/batch"),
        };

        let result = client
            .delete_all(&[
                ("t1".to_string(), "e1".to_string()),
                ("t2".to_string(), "e2".to_string()),
            ])
            .await
            .unwrap();

        assert_eq!(result.deleted, vec!["t1", "t2"]);
        assert!(result.failed.is_empty());
    }

    #[tokio::test]
    async fn delete_all_reports_partial_failures() {
        let base_url = start_mock_google_server(vec![204, 403]).await;
        let client = Client {
            http: reqwest::Client::new(),
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            refresh_token: "refresh".to_string(),
            calendar_id: "primary".to_string(),
            token_url: format!("{base_url}/token"),
            batch_url: format!("{base_url}/batch"),
        };

        let result = client
            .delete_all(&[
                ("t1".to_string(), "e1".to_string()),
                ("t2".to_string(), "e2".to_string()),
            ])
            .await
            .unwrap();

        assert_eq!(result.deleted, vec!["t1"]);
        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].task_id, "t2");
    }

    #[tokio::test]
    async fn delete_all_treats_410_as_deleted() {
        let base_url = start_mock_google_server(vec![410]).await;
        let client = Client {
            http: reqwest::Client::new(),
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            refresh_token: "refresh".to_string(),
            calendar_id: "primary".to_string(),
            token_url: format!("{base_url}/token"),
            batch_url: format!("{base_url}/batch"),
        };

        let result = client
            .delete_all(&[("t1".to_string(), "e1".to_string())])
            .await
            .unwrap();

        assert_eq!(result.deleted, vec!["t1"]);
        assert!(result.failed.is_empty());
    }

    fn build_batch_multipart_response(boundary: &str, statuses: &[u16]) -> String {
        let mut body = String::new();
        for (idx, status) in statuses.iter().enumerate() {
            let reason = match *status {
                200 => "OK",
                204 => "No Content",
                403 => "Forbidden",
                410 => "Gone",
                _ => "",
            };
            body.push_str("--");
            body.push_str(boundary);
            body.push_str("\r\n");
            body.push_str("Content-Type: application/http\r\n");
            body.push_str(&format!("Content-ID: response-item-{idx}\r\n"));
            body.push_str("\r\n");
            body.push_str(&format!("HTTP/1.1 {status} {reason}\r\n"));
            body.push_str("\r\n");
            body.push_str("\r\n");
        }
        body.push_str("--");
        body.push_str(boundary);
        body.push_str("--\r\n");
        body
    }

    async fn start_mock_google_server(statuses: Vec<u16>) -> String {
        use std::time::Duration;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let token_body = r#"{"access_token":"test_token","expires_in":3600}"#;
        let token_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{token_body}",
            token_body.len()
        )
        .into_bytes();
        let batch_body = build_batch_multipart_response("test_boundary", &statuses);
        let batch_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: multipart/mixed; boundary=test_boundary\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            batch_body.len(),
            batch_body
        )
        .into_bytes();

        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let token = token_response.clone();
                let batch = batch_response.clone();
                tokio::spawn(async move {
                    let headers = read_http_headers(&mut stream).await;
                    let req = String::from_utf8_lossy(&headers);
                    let content_length = parse_content_length(&headers);
                    if content_length > 0 {
                        let mut body = vec![0u8; content_length];
                        let _ = stream.read_exact(&mut body).await;
                    }
                    let response: &[u8] = if req.starts_with("POST /token") {
                        &token
                    } else if req.starts_with("POST /batch") {
                        &batch
                    } else {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    };
                    let _ = stream.write_all(response).await;
                    let _ = stream.flush().await;
                });
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        format!("http://{addr}")
    }

    async fn read_http_headers(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match stream.read(&mut byte).await {
                Ok(0) => break,
                Ok(_) => {
                    buf.push(byte[0]);
                    if buf.len() >= 4 && &buf[buf.len() - 4..] == b"\r\n\r\n" {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        buf
    }

    #[test]
    fn event_body_includes_reminders_when_set() {
        let client = Client {
            http: reqwest::Client::new(),
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            refresh_token: "refresh".to_string(),
            calendar_id: "primary".to_string(),
            token_url: GOOGLE_TOKEN_URL.to_string(),
            batch_url: GOOGLE_BATCH_URL.to_string(),
        };
        let entry = SyncEntry {
            task_id: "t1".to_string(),
            summary: "task".to_string(),
            description: None,
            start: "2025-06-01T10:00:00Z".to_string(),
            end: "2025-06-01T11:00:00Z".to_string(),
            reminder_minutes: Some(15),
            color_id: None,
            visibility: None,
            transparency: None,
        };
        let body = client.event_body(&entry);
        assert_eq!(body["reminders"]["useDefault"], false);
        assert_eq!(body["reminders"]["overrides"][0]["method"], "popup");
        assert_eq!(body["reminders"]["overrides"][0]["minutes"], 15);
    }

    #[test]
    fn event_body_omits_reminders_when_not_set() {
        let client = Client {
            http: reqwest::Client::new(),
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            refresh_token: "refresh".to_string(),
            calendar_id: "primary".to_string(),
            token_url: GOOGLE_TOKEN_URL.to_string(),
            batch_url: GOOGLE_BATCH_URL.to_string(),
        };
        let entry = SyncEntry {
            task_id: "t1".to_string(),
            summary: "task".to_string(),
            description: None,
            start: "2025-06-01T10:00:00Z".to_string(),
            end: "2025-06-01T11:00:00Z".to_string(),
            reminder_minutes: None,
            color_id: None,
            visibility: None,
            transparency: None,
        };
        let body = client.event_body(&entry);
        assert!(body["reminders"].is_null());
    }

    #[test]
    fn event_body_omits_reminders_when_zero_or_negative() {
        let client = Client {
            http: reqwest::Client::new(),
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            refresh_token: "refresh".to_string(),
            calendar_id: "primary".to_string(),
            token_url: GOOGLE_TOKEN_URL.to_string(),
            batch_url: GOOGLE_BATCH_URL.to_string(),
        };
        for minutes in [0, -5] {
            let entry = SyncEntry {
                task_id: "t1".to_string(),
                summary: "task".to_string(),
                description: None,
                start: "2025-06-01T10:00:00Z".to_string(),
                end: "2025-06-01T11:00:00Z".to_string(),
                reminder_minutes: Some(minutes),
                color_id: None,
                visibility: None,
                transparency: None,
            };
            let body = client.event_body(&entry);
            assert!(
                body["reminders"].is_null(),
                "reminders should be omitted for {minutes}"
            );
        }
    }

    #[test]
    fn event_body_includes_color_visibility_transparency() {
        let client = Client {
            http: reqwest::Client::new(),
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            refresh_token: "refresh".to_string(),
            calendar_id: "primary".to_string(),
            token_url: GOOGLE_TOKEN_URL.to_string(),
            batch_url: GOOGLE_BATCH_URL.to_string(),
        };
        let entry = SyncEntry {
            task_id: "t1".to_string(),
            summary: "task".to_string(),
            description: None,
            start: "2025-06-01T10:00:00Z".to_string(),
            end: "2025-06-01T11:00:00Z".to_string(),
            reminder_minutes: None,
            color_id: Some(5),
            visibility: Some("private".to_string()),
            transparency: Some("transparent".to_string()),
        };
        let body = client.event_body(&entry);
        assert_eq!(body["colorId"], "5");
        assert_eq!(body["visibility"], "private");
        assert_eq!(body["transparency"], "transparent");
    }

    #[test]
    fn event_body_ignores_invalid_common_settings() {
        let client = Client {
            http: reqwest::Client::new(),
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            refresh_token: "refresh".to_string(),
            calendar_id: "primary".to_string(),
            token_url: GOOGLE_TOKEN_URL.to_string(),
            batch_url: GOOGLE_BATCH_URL.to_string(),
        };
        let entry = SyncEntry {
            task_id: "t1".to_string(),
            summary: "task".to_string(),
            description: None,
            start: "2025-06-01T10:00:00Z".to_string(),
            end: "2025-06-01T11:00:00Z".to_string(),
            reminder_minutes: None,
            color_id: Some(12),
            visibility: Some("invalid".to_string()),
            transparency: Some("unknown".to_string()),
        };
        let body = client.event_body(&entry);
        assert!(body["colorId"].is_null());
        assert!(body["visibility"].is_null());
        assert!(body["transparency"].is_null());
    }

    fn parse_content_length(headers: &[u8]) -> usize {
        let text = String::from_utf8_lossy(headers).to_lowercase();
        for line in text.lines() {
            if line.starts_with("content-length:") {
                return line
                    .split(':')
                    .nth(1)
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
            }
        }
        0
    }
}
