// ratatui inline REPL for the takusu agent.
//
// The REPL renders a multi-line text editor at the bottom of the terminal and a
// streaming conversation log above it. It uses `run_turn_stream` for each user
// input and integrates a custom `UserInputProvider` so tool questions and
// approval prompts are also rendered inside the same TUI.

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use crossterm::event::{self, Event as CtEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::{Frame, TerminalOptions, Viewport};
use ratatui_textarea::TextArea;

type TuiTextArea = TextArea<'static>;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::time::interval;

use takusu_agent::{
    AgentSession, ApprovalRequest, PermissionKey, SessionSnapshot, ToolError, TurnEvent,
    TurnResult, UserInputAnswer, UserInputProvider, UserInputQuestion,
};
use takusu_local_lib::error::{AppError, BadRequestKind};

use crate::agent::{
    ApprovalChoice, agent_err, agent_state_dir, load_session_snapshot, permissions_for_approval,
    write_private_file,
};

pub struct QuestionEvent {
    pub questions: Vec<UserInputQuestion>,
    pub answer_tx: oneshot::Sender<Vec<UserInputAnswer>>,
}

pub struct ReplUserInputProvider {
    tx: UnboundedSender<QuestionEvent>,
}

impl ReplUserInputProvider {
    pub fn new(tx: UnboundedSender<QuestionEvent>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl UserInputProvider for ReplUserInputProvider {
    async fn request(
        &self,
        _call_id: &str,
        questions: Vec<UserInputQuestion>,
    ) -> Result<Vec<UserInputAnswer>, ToolError> {
        let (answer_tx, answer_rx) = oneshot::channel();
        self.tx
            .send(QuestionEvent {
                questions,
                answer_tx,
            })
            .map_err(|_| ToolError::Cancelled)?;
        answer_rx.await.map_err(|_| ToolError::Cancelled)
    }

    async fn resolve(
        &self,
        _call_id: &str,
        _answers: Vec<UserInputAnswer>,
    ) -> Result<(), ToolError> {
        Ok(())
    }
}

enum ReplEvent {
    Turn(TurnEvent),
    Done(Box<TurnResult>),
    Error(AppError),
}

enum Message {
    User(String),
    Assistant(String),
    ToolCall(String),
    ToolResult(String, bool),
    Info(String),
}

enum ReplMode {
    Chat,
    Approval(ApprovalRequest),
    Question(QuestionState),
}

struct QuestionState {
    questions: Vec<UserInputQuestion>,
    answers: Vec<UserInputAnswer>,
    answer_tx: Option<oneshot::Sender<Vec<UserInputAnswer>>>,
}

impl QuestionState {
    fn current(&self) -> Option<&UserInputQuestion> {
        self.questions.get(self.answers.len())
    }
}

#[derive(Default)]
struct HistoryState {
    entries: Vec<Vec<String>>,
    index: Option<usize>,
    saved_input: Vec<String>,
}

impl HistoryState {
    fn prev(&self) -> Option<usize> {
        match self.index {
            None if !self.entries.is_empty() => Some(self.entries.len() - 1),
            Some(i) if i > 0 => Some(i - 1),
            _ => None,
        }
    }

    fn next(&self) -> Option<usize> {
        match self.index {
            None => None,
            Some(i) if i + 1 < self.entries.len() => Some(i + 1),
            _ => None,
        }
    }
}

struct ReplState {
    messages: Vec<Message>,
    pending_text: String,
    thinking: bool,
    mode: ReplMode,
}

pub async fn run(
    session: Arc<tokio::sync::Mutex<AgentSession>>,
    mut question_rx: UnboundedReceiver<QuestionEvent>,
    yes: bool,
) -> Result<(), AppError> {
    let mut terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(10),
    });

    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<KeyEvent>();
    let (turn_tx, mut turn_rx) = mpsc::unbounded_channel::<ReplEvent>();
    let (submit_tx, mut submit_rx) = mpsc::unbounded_channel::<String>();

    // crossterm event loop
    tokio::spawn(async move {
        loop {
            if event::poll(Duration::from_millis(250)).unwrap_or(false)
                && let Ok(CtEvent::Key(key)) = event::read()
                && key.kind == KeyEventKind::Press
                && key_tx.send(key).is_err()
            {
                break;
            }
        }
    });

    // worker: run turns and stream events back
    let session2 = Arc::clone(&session);
    tokio::spawn(async move {
        while let Some(text) = submit_rx.recv().await {
            let turn_tx = turn_tx.clone();
            let res = {
                let guard = session2.lock().await;
                guard
                    .run_turn_stream(
                        &text,
                        |event| {
                            let _ = turn_tx.send(ReplEvent::Turn(event));
                        },
                        |_block| {},
                    )
                    .await
            };
            match res {
                Ok(result) => {
                    let _ = turn_tx.send(ReplEvent::Done(Box::new(result)));
                }
                Err(e) => {
                    let _ = turn_tx.send(ReplEvent::Error(agent_err(e)));
                }
            }
        }
    });

    let mut textarea = TuiTextArea::default();
    textarea.set_block(Block::default());
    let mut history = HistoryState::default();

    let mut state = ReplState {
        messages: Vec::new(),
        pending_text: String::new(),
        thinking: false,
        mode: ReplMode::Chat,
    };

    let mut tick = interval(Duration::from_millis(100));
    let mut spinner_frame: u8 = 0;

    let result: Result<(), AppError> = loop {
        if let Err(e) = terminal.draw(|frame| draw(frame, &mut state, &mut textarea, spinner_frame))
        {
            break Err(AppError::Internal(e.to_string()));
        }

        spinner_frame = spinner_frame.wrapping_add(1);

        tokio::select! {
            _ = tick.tick() => {}
            Some(key) = key_rx.recv() => {
                match handle_key(
                    key,
                    &session,
                    &mut state,
                    &mut textarea,
                    &mut history,
                    &submit_tx,
                    yes,
                ).await {
                    Action::Continue => {}
                    Action::Exit => break Ok(()),
                }
            }
            Some(ev) = turn_rx.recv() => {
                handle_turn_event(ev, &mut state);
            }
            Some(q) = question_rx.recv() => {
                state.mode = ReplMode::Question(QuestionState {
                    questions: q.questions,
                    answers: Vec::new(),
                    answer_tx: Some(q.answer_tx),
                });
            }
        }
    };

    ratatui::restore();
    result
}

enum Action {
    Continue,
    Exit,
}

fn placeholder_text(mode: &ReplMode) -> &'static str {
    match mode {
        ReplMode::Chat => "ask the agent...",
        ReplMode::Approval(_) => "Approve? (y/n/a)",
        ReplMode::Question(_) => "answer...",
    }
}

fn render_markdown(src: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut styles: Vec<Style> = Vec::new();
    let mut code_block = String::new();
    let mut in_code_block = false;
    let mut list_depth: usize = 0;
    let mut pending_prefix: String = String::new();
    let mut link_url: Option<String> = None;
    let mut image_url: Option<String> = None;

    let current_style = |styles: &[Style]| {
        let mut s = Style::default();
        for st in styles {
            s = s.patch(*st);
        }
        s
    };

    for event in Parser::new(src) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Strong => styles.push(Style::default().add_modifier(Modifier::BOLD)),
                Tag::Emphasis => styles.push(Style::default().add_modifier(Modifier::ITALIC)),
                Tag::Strikethrough => {
                    styles.push(Style::default().add_modifier(Modifier::CROSSED_OUT))
                }
                Tag::Link { dest_url, .. } => {
                    styles.push(
                        Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::UNDERLINED),
                    );
                    link_url = Some(dest_url.to_string());
                }
                Tag::Image { dest_url, .. } => {
                    image_url = Some(dest_url.to_string());
                }
                Tag::Heading { level, .. } => {
                    pending_prefix = "#".repeat(level as usize);
                    pending_prefix.push(' ');
                    styles.push(Style::default().add_modifier(Modifier::BOLD));
                }
                Tag::List(_) => list_depth += 1,
                Tag::Item => {
                    if current.is_empty() {
                        current.push(Span::raw("  ".repeat(list_depth.saturating_sub(1))));
                        current.push(Span::raw("• "));
                    }
                }
                Tag::CodeBlock(_) => in_code_block = true,
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Heading(_) => {
                    styles.pop();
                }
                TagEnd::Link => {
                    styles.pop();
                    if let Some(url) = link_url.take() {
                        current.push(Span::styled(
                            format!(" ({url})"),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                }
                TagEnd::Image => {
                    if let Some(url) = image_url.take() {
                        current.push(Span::styled(
                            format!(" ({url})"),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                }
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    if !code_block.is_empty() {
                        for l in code_block.split('\n') {
                            lines.push(Line::styled(
                                format!("  {l}"),
                                Style::default().fg(Color::DarkGray),
                            ));
                        }
                        code_block.clear();
                    }
                }
                TagEnd::Paragraph | TagEnd::Item => {
                    if !current.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut current)));
                    }
                }
                TagEnd::List(_) => list_depth = list_depth.saturating_sub(1),
                _ => {}
            },
            Event::Text(text) => {
                if in_code_block {
                    code_block.push_str(&text);
                } else {
                    if !pending_prefix.is_empty() {
                        current.push(Span::raw(std::mem::take(&mut pending_prefix)));
                    }
                    current.push(Span::styled(text.to_string(), current_style(&styles)));
                }
            }
            Event::Code(code) => {
                current.push(Span::styled(
                    code.to_string(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            Event::SoftBreak => {
                if in_code_block {
                    code_block.push('\n');
                } else {
                    current.push(Span::raw(" "));
                }
            }
            Event::HardBreak => {
                if in_code_block {
                    code_block.push('\n');
                } else {
                    lines.push(Line::from(std::mem::take(&mut current)));
                }
            }
            Event::Rule => lines.push(Line::from("---")),
            Event::TaskListMarker(checked) => {
                current.push(Span::raw(if checked { "[x] " } else { "[ ] " }));
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                current.push(Span::raw(html.to_string()));
            }
            Event::FootnoteReference(label) => {
                current.push(Span::styled(
                    format!("[^{label}]"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            _ => {}
        }
    }

    if !current.is_empty() {
        lines.push(Line::from(current));
    }
    if in_code_block && !code_block.is_empty() {
        for l in code_block.split('\n') {
            lines.push(Line::styled(
                format!("  {l}"),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    lines
}

fn draw(frame: &mut Frame, state: &mut ReplState, textarea: &mut TuiTextArea, spinner: u8) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(9), Constraint::Length(1)])
        .split(area);

    let content_area = chunks[0];
    let input_area = chunks[1];

    // content area
    let mut lines = Vec::new();
    for msg in &state.messages {
        match msg {
            Message::User(text) => {
                for (i, l) in text.lines().enumerate() {
                    if i == 0 {
                        lines.push(Line::from(vec![
                            Span::styled(
                                "you ",
                                Style::default()
                                    .fg(Color::Cyan)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(l),
                        ]));
                    } else {
                        lines.push(Line::raw(format!("     {l}")));
                    }
                }
            }
            Message::Assistant(text) => {
                lines.extend(render_markdown(text));
            }
            Message::ToolCall(name) => {
                lines.push(Line::from(vec![
                    Span::styled("[tool call] ", Style::default().fg(Color::Blue)),
                    Span::raw(name.clone()),
                ]));
            }
            Message::ToolResult(name, is_error) => {
                let status = if *is_error { "error" } else { "ok" };
                let color = if *is_error { Color::Red } else { Color::Green };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("[tool result] {name}: "),
                        Style::default().fg(Color::Blue),
                    ),
                    Span::styled(status.to_string(), Style::default().fg(color)),
                ]));
            }
            Message::Info(text) => {
                lines.push(Line::styled(
                    text.clone(),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }
    }

    if state.thinking {
        let spinner_chars = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let ch = spinner_chars[spinner as usize % spinner_chars.len()];
        lines.push(Line::from(vec![
            Span::styled("thinking ", Style::default().fg(Color::Yellow)),
            Span::raw(ch),
        ]));
    }

    if !state.pending_text.is_empty() {
        for l in state.pending_text.lines() {
            lines.push(Line::raw(l.to_string()));
        }
    }

    match &state.mode {
        ReplMode::Approval(approval) => approval_lines(approval, &mut lines),
        ReplMode::Question(qs) => {
            if let Some(q) = qs.current() {
                lines.push(Line::from(vec![
                    Span::styled("question ", Style::default().fg(Color::Magenta)),
                    Span::raw(&q.purpose),
                ]));
                if !q.text.is_empty() {
                    for l in q.text.lines() {
                        lines.push(Line::raw(format!("  {l}")));
                    }
                }
            }
        }
        _ => {}
    }

    let scroll_y = (lines.len() as u16).saturating_sub(content_area.height);
    let content = Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: true })
        .scroll((0, scroll_y));
    frame.render_widget(content, content_area);

    // input area (single-line, frameless)
    textarea.set_placeholder_text(placeholder_text(&state.mode));
    textarea.set_block(Block::default());
    frame.render_widget(&*textarea, input_area);

    let cursor = textarea.screen_cursor();
    let inner = textarea.block().map_or(input_area, |b| b.inner(input_area));
    frame.set_cursor_position((
        inner.x + (cursor.col as u16).min(inner.width.saturating_sub(1)),
        inner.y + (cursor.row as u16).min(inner.height.saturating_sub(1)),
    ));
}

fn approval_lines(approval: &ApprovalRequest, lines: &mut Vec<Line>) {
    lines.push(Line::from(vec![
        Span::styled("approval ", Style::default().fg(Color::Magenta)),
        Span::raw("proposal(s) require confirmation"),
    ]));
    if !approval.why.is_empty() {
        lines.push(Line::raw(format!("why: {}", approval.why)));
    }
    if !approval.warnings.is_empty() {
        for w in &approval.warnings {
            lines.push(Line::styled(
                format!("warning: {w}"),
                Style::default().fg(Color::Yellow),
            ));
        }
    }
    for (i, change) in approval.changes.iter().enumerate() {
        lines.push(Line::raw(format!(
            "  {} {} #{}",
            i + 1,
            change.operation,
            change.target.display_id
        )));
        if !change.description.is_empty() {
            lines.push(Line::raw(format!("    {}", change.description)));
        }
        if let Some(before) = &change.before {
            lines.push(Line::raw(format!("    before: {}", before)));
        }
        if let Some(after) = &change.after {
            lines.push(Line::raw(format!("    after:  {}", after)));
        }
    }
    lines.push(Line::styled(
        "Approve? (y/n/a)".to_string(),
        Style::default().fg(Color::Cyan),
    ));
}

async fn handle_key(
    key: KeyEvent,
    session: &Arc<tokio::sync::Mutex<AgentSession>>,
    state: &mut ReplState,
    textarea: &mut TuiTextArea,
    history: &mut HistoryState,
    submit_tx: &UnboundedSender<String>,
    yes: bool,
) -> Action {
    match std::mem::replace(&mut state.mode, ReplMode::Chat) {
        ReplMode::Approval(approval) => {
            handle_approval_key(key, session, state, approval, yes).await
        }
        ReplMode::Question(qs) => {
            let mut qs = qs;
            let action = handle_question_key(key, state, &mut qs, textarea).await;
            if qs.answer_tx.is_some() {
                state.mode = ReplMode::Question(qs);
            }
            action
        }
        ReplMode::Chat => {
            if key.code == KeyCode::Esc
                || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
            {
                return Action::Exit;
            }

            // history navigation
            let up = key.code == KeyCode::Up
                || (key.code == KeyCode::Char('p')
                    && key.modifiers.contains(KeyModifiers::CONTROL));
            let down = key.code == KeyCode::Down
                || (key.code == KeyCode::Char('n')
                    && key.modifiers.contains(KeyModifiers::CONTROL));

            if up && textarea.cursor().0 == 0 {
                if history.index.is_none() && !textarea.is_empty() {
                    history.saved_input = textarea.lines().to_vec();
                }
                if let Some(idx) = history.prev() {
                    history.index = Some(idx);
                    *textarea = TuiTextArea::new(history.entries[idx].clone());
                    textarea.set_block(Block::default());
                }
                return Action::Continue;
            }
            if down && textarea.cursor().0 == textarea.lines().len().saturating_sub(1) {
                if let Some(idx) = history.next() {
                    history.index = Some(idx);
                    *textarea = TuiTextArea::new(history.entries[idx].clone());
                    textarea.set_block(Block::default());
                } else {
                    history.index = None;
                    *textarea = TuiTextArea::new(history.saved_input.clone());
                    textarea.set_block(Block::default());
                }
                return Action::Continue;
            }

            // submit
            if key.code == KeyCode::Enter && !key.modifiers.contains(KeyModifiers::SHIFT) {
                let lines: Vec<String> = textarea.lines().iter().map(|s| s.to_string()).collect();
                let text = lines.join("\n").trim_end().to_string();
                if text.is_empty() {
                    return Action::Continue;
                }
                if history.entries.last() != Some(&lines) {
                    history.entries.push(lines);
                }
                history.index = None;
                history.saved_input.clear();
                *textarea = TuiTextArea::default();
                textarea.set_block(Block::default());

                if let Some(cmd) = text.strip_prefix('/') {
                    return handle_slash(cmd, session, state).await;
                }

                state.messages.push(Message::User(text.clone()));
                if submit_tx.send(text).is_err() {
                    state.messages.push(Message::Info(
                        "failed to send message: worker closed".into(),
                    ));
                }
                return Action::Continue;
            }

            // shift+enter -> newline
            if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT) {
                textarea.insert_newline();
                return Action::Continue;
            }

            // other editing
            textarea.input(key);
            Action::Continue
        }
    }
}

async fn handle_approval_key(
    key: KeyEvent,
    session: &Arc<tokio::sync::Mutex<AgentSession>>,
    state: &mut ReplState,
    approval: ApprovalRequest,
    yes: bool,
) -> Action {
    let choice = if yes {
        ApprovalChoice::Yes
    } else {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => ApprovalChoice::Yes,
            KeyCode::Char('a') | KeyCode::Char('A') => ApprovalChoice::Always,
            _ => ApprovalChoice::No,
        }
    };

    let approve = choice != ApprovalChoice::No;

    if choice == ApprovalChoice::Always {
        let perms = permissions_for_approval(&approval, None);
        let _ = session.lock().await.set_session_permissions(perms);
    }

    let res = session
        .lock()
        .await
        .resolve_approval(&approval.id, approve, None)
        .await
        .map_err(agent_err);

    match res {
        Ok(result) => {
            state.messages.push(Message::Info(format!(
                "{} {} change(s)",
                if result.approved {
                    "approved"
                } else {
                    "denied"
                },
                result.changes.len()
            )));
            for r in &result.changes {
                state.messages.push(Message::Info(format!(
                    "  {} {}: {}",
                    r.operation, r.target.target_type, r.target.target_id
                )));
            }
            if result.schedule_dirty {
                state
                    .messages
                    .push(Message::Info("schedule dirty: true".into()));
            }
        }
        Err(e) => state
            .messages
            .push(Message::Info(format!("approval failed: {e}"))),
    }

    state.mode = ReplMode::Chat;
    Action::Continue
}

async fn handle_question_key(
    key: KeyEvent,
    state: &mut ReplState,
    qs: &mut QuestionState,
    textarea: &mut TuiTextArea,
) -> Action {
    if key.code == KeyCode::Esc {
        // cancel: use the original question text for remaining answers
        while let Some(q) = qs.questions.get(qs.answers.len()) {
            qs.answers.push(UserInputAnswer {
                text: q.text.clone(),
            });
        }
        if let Some(tx) = qs.answer_tx.take() {
            let _ = tx.send(qs.answers.clone());
        }
        state.mode = ReplMode::Chat;
        *textarea = TuiTextArea::default();
        textarea.set_block(Block::default());
        return Action::Continue;
    }

    if key.code == KeyCode::Enter && !key.modifiers.contains(KeyModifiers::SHIFT) {
        let text = textarea.lines().join("\n").trim_end().to_string();
        if let Some(q) = qs.questions.get(qs.answers.len()) {
            qs.answers.push(UserInputAnswer {
                text: if text.is_empty() {
                    q.text.clone()
                } else {
                    text
                },
            });
        }
        if qs.answers.len() >= qs.questions.len() {
            if let Some(tx) = qs.answer_tx.take() {
                let _ = tx.send(qs.answers.clone());
            }
            state.mode = ReplMode::Chat;
            *textarea = TuiTextArea::default();
            textarea.set_block(Block::default());
        } else {
            // next question: clear textarea
            *textarea = TuiTextArea::default();
            textarea.set_block(Block::default());
        }
        return Action::Continue;
    }

    if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT) {
        textarea.insert_newline();
        return Action::Continue;
    }

    textarea.input(key);
    Action::Continue
}

async fn handle_slash(
    cmd: &str,
    session: &Arc<tokio::sync::Mutex<AgentSession>>,
    state: &mut ReplState,
) -> Action {
    let mut parts = cmd.splitn(3, ' ');
    let command = parts.next().unwrap_or("");
    let arg = parts.next();
    let _extra = parts.next();

    match command {
        "exit" | "quit" => Action::Exit,
        "clear" => {
            state.messages.clear();
            let _ = session.lock().await.set_history(vec![]);
            let _ = session.lock().await.set_compaction_summary(None);
            let _ = session.lock().await.set_schedule_dirty(false);
            state.messages.push(Message::Info("cleared".into()));
            Action::Continue
        }
        "new" => {
            let id = format!("session-{}", uuid::Uuid::now_v7());
            let mut guard = session.lock().await;
            guard.set_session_id(id);
            let _ = guard.set_history(vec![]);
            let _ = guard.set_compaction_summary(None);
            let _ = guard.set_schedule_dirty(false);
            drop(guard);
            state.messages.push(Message::Info("new session".into()));
            Action::Continue
        }
        "continue" => match load_session_snapshot() {
            Ok(Some(snapshot)) => {
                let mut guard = session.lock().await;
                match guard.restore_from_snapshot(&snapshot) {
                    Ok(()) => state.messages.push(Message::Info("loaded".into())),
                    Err(e) => state
                        .messages
                        .push(Message::Info(format!("load failed: {e}"))),
                }
                drop(guard);
                Action::Continue
            }
            Ok(None) => {
                state
                    .messages
                    .push(Message::Info("no saved session".into()));
                Action::Continue
            }
            Err(e) => {
                state
                    .messages
                    .push(Message::Info(format!("load failed: {e}")));
                Action::Continue
            }
        },
        "save" => {
            let name = arg.unwrap_or("default");
            match save_named_session(session, name).await {
                Ok(()) => state
                    .messages
                    .push(Message::Info(format!("saved '{name}'"))),
                Err(e) => state
                    .messages
                    .push(Message::Info(format!("save failed: {e}"))),
            }
            Action::Continue
        }
        "load" => {
            let name = arg.unwrap_or("default");
            match load_named_session(session, name).await {
                Ok(()) => state
                    .messages
                    .push(Message::Info(format!("loaded '{name}'"))),
                Err(e) => state
                    .messages
                    .push(Message::Info(format!("load failed: {e}"))),
            }
            Action::Continue
        }
        "history" => {
            match list_named_sessions() {
                Ok(names) => {
                    if names.is_empty() {
                        state
                            .messages
                            .push(Message::Info("no saved sessions".into()));
                    } else {
                        for n in names {
                            state.messages.push(Message::Info(n));
                        }
                    }
                }
                Err(e) => state
                    .messages
                    .push(Message::Info(format!("list failed: {e}"))),
            }
            Action::Continue
        }
        "allow" | "deny" => {
            if let Some(key) = arg {
                let allowed = command == "allow";
                match set_session_permission(session, key, allowed).await {
                    Ok(()) => state.messages.push(Message::Info(format!(
                        "{} {}",
                        if allowed { "allowed" } else { "denied" },
                        key
                    ))),
                    Err(e) => state
                        .messages
                        .push(Message::Info(format!("permission failed: {e}"))),
                }
            } else {
                state
                    .messages
                    .push(Message::Info(format!("usage: /{command} <perm>")));
            }
            Action::Continue
        }
        "help" => {
            let help = "commands: /exit /clear /new /continue /save [name] /load [name] /history /allow <perm> /deny <perm> /help";
            state.messages.push(Message::Info(help.into()));
            Action::Continue
        }
        _ => {
            state
                .messages
                .push(Message::Info(format!("unknown command: /{command}")));
            Action::Continue
        }
    }
}

fn named_session_path(name: &str) -> PathBuf {
    let mut path = agent_state_dir();
    path.push("takusu");
    if name == "default" {
        path.push("agent-session.json");
    } else {
        path.push(format!("agent-session-{name}.json"));
    }
    path
}

async fn save_named_session(
    session: &Arc<tokio::sync::Mutex<AgentSession>>,
    name: &str,
) -> Result<(), AppError> {
    let snapshot = session
        .lock()
        .await
        .snapshot()
        .map_err(|e| AppError::Internal(format!("failed to snapshot: {e}")))?;
    let path = named_session_path(name);
    let content = serde_json::to_string_pretty(&snapshot)
        .map_err(|e| AppError::Internal(format!("failed to serialize: {e}")))?;
    tokio::task::spawn_blocking(move || write_private_file(&path, &content))
        .await
        .map_err(|e| AppError::Internal(format!("write task failed: {e}")))?
        .map_err(|e| AppError::Internal(format!("failed to write: {e}")))?;
    Ok(())
}

async fn load_named_session(
    session: &Arc<tokio::sync::Mutex<AgentSession>>,
    name: &str,
) -> Result<(), AppError> {
    let path = named_session_path(name);
    let content = tokio::task::spawn_blocking(move || std::fs::read_to_string(&path))
        .await
        .map_err(|e| AppError::Internal(format!("read task failed: {e}")))?
        .map_err(|e| AppError::Internal(format!("failed to read: {e}")))?;
    let snapshot: SessionSnapshot = serde_json::from_str(&content)
        .map_err(|e| AppError::Internal(format!("invalid snapshot: {e}")))?;
    let mut guard = session.lock().await;
    guard
        .restore_from_snapshot(&snapshot)
        .map_err(|e| AppError::Internal(format!("failed to restore: {e}")))?;
    Ok(())
}

fn list_named_sessions() -> Result<Vec<String>, AppError> {
    let mut dir = agent_state_dir();
    dir.push("takusu");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .map_err(|e| AppError::Internal(format!("failed to read state dir: {e}")))?
    {
        let entry = entry.map_err(|e| AppError::Internal(format!("dir entry: {e}")))?;
        let name = entry.file_name();
        if let Some(s) = name.to_str() {
            if s == "agent-session.json" {
                names.push("default".into());
            } else if let Some(n) = s
                .strip_prefix("agent-session-")
                .and_then(|x| x.strip_suffix(".json"))
            {
                names.push(n.into());
            }
        }
    }
    names.sort();
    Ok(names)
}

async fn set_session_permission(
    session: &Arc<tokio::sync::Mutex<AgentSession>>,
    key: &str,
    allowed: bool,
) -> Result<(), AppError> {
    let parsed = PermissionKey::from_str(key)
        .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
    let guard = session.lock().await;
    let mut perms = guard
        .session_permissions()
        .map_err(|e| AppError::Internal(format!("failed to get permissions: {e}")))?;
    perms.set(parsed.target, parsed.operation, allowed);
    guard
        .set_session_permissions(perms)
        .map_err(|e| AppError::Internal(format!("failed to set permissions: {e}")))?;
    Ok(())
}

fn handle_turn_event(ev: ReplEvent, state: &mut ReplState) {
    match ev {
        ReplEvent::Turn(TurnEvent::Text(delta)) => {
            state.thinking = false;
            state.pending_text.push_str(&delta);
        }
        ReplEvent::Turn(TurnEvent::Thinking(_)) => {
            state.thinking = true;
        }
        ReplEvent::Turn(TurnEvent::ToolCall { name, .. }) => {
            state.thinking = false;
            state.messages.push(Message::ToolCall(name));
        }
        ReplEvent::Turn(TurnEvent::ToolResult { name, is_error, .. }) => {
            state.messages.push(Message::ToolResult(name, is_error));
        }
        ReplEvent::Done(result) => {
            state.thinking = false;
            if !state.pending_text.is_empty() {
                state
                    .messages
                    .push(Message::Assistant(state.pending_text.clone()));
                state.pending_text.clear();
            } else if !result.text.is_empty() {
                state.messages.push(Message::Assistant(result.text));
            }
            if let Some(approval) = result.approval_request {
                state.mode = ReplMode::Approval(approval);
            } else {
                if !result.changes.is_empty() {
                    for r in &result.changes {
                        state.messages.push(Message::Info(format!(
                            "change: {} {}: {}",
                            r.operation, r.target.target_type, r.target.target_id
                        )));
                    }
                }
                if result.schedule_dirty {
                    state
                        .messages
                        .push(Message::Info("schedule dirty: true".into()));
                }
            }
        }
        ReplEvent::Turn(TurnEvent::Error(_)) | ReplEvent::Turn(TurnEvent::Done(_)) => {}
        ReplEvent::Error(e) => {
            state.thinking = false;
            state.pending_text.clear();
            state.messages.push(Message::Info(format!("error: {e}")));
        }
    }
}
