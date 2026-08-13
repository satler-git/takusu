use std::collections::HashMap;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use takusu_contracts::{
    CommentRow, GenerateSchedule, HabitRow, Reschedule, ScheduleEntry, SettingsRow, SleepInput,
    TaskRow,
};
use takusu_local_lib::app::TakusuApp;
use takusu_types::{EnumLabel, ScheduleMode};

use crate::tabs::{habits, schedule, settings, tasks};
use crate::widgets::list::StatefulList;

pub enum Msg {
    Key(KeyEvent),
    Tick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Schedule,
    Tasks,
    Habits,
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Schedule, Tab::Tasks, Tab::Habits, Tab::Settings];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Schedule => "Schedule",
            Tab::Tasks => "Tasks",
            Tab::Habits => "Habits",
            Tab::Settings => "Settings",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modal {
    None,
    ConfirmDelete,
    CreateTask { field: usize },
    Help,
}

/// Load state of one task's comment timeline (WI-5). Distinguishes not-yet-
/// loaded, in-flight, empty, loaded, and failed so the detail pane never
/// renders an error as an empty timeline.
#[derive(Debug, Clone)]
pub enum CommentState {
    Loading,
    Empty,
    Loaded(Vec<CommentRow>),
    Error,
}

pub struct App {
    pub app: Arc<TakusuApp>,
    pub tz: jiff::tz::TimeZone,
    pub tab: Tab,
    pub modal: Modal,
    pub status_msg: Option<String>,

    pub tasks: Vec<TaskRow>,
    pub all_tasks: Vec<TaskRow>,
    pub task_list: StatefulList,
    pub task_filter: Option<String>,
    pub comments: HashMap<String, CommentState>,

    pub habits: Vec<HabitRow>,
    pub habit_list: StatefulList,

    pub schedule_entries: Vec<ScheduleEntry>,
    pub schedule_list: StatefulList,

    pub settings: Option<SettingsRow>,

    pub create_fields: Vec<String>,
}

impl App {
    pub fn new(app: Arc<TakusuApp>, tz: jiff::tz::TimeZone) -> Self {
        Self {
            app,
            tz,
            tab: Tab::Schedule,
            modal: Modal::None,
            status_msg: None,
            tasks: Vec::new(),
            all_tasks: Vec::new(),
            task_list: StatefulList::new(),
            task_filter: None,
            comments: HashMap::new(),
            habits: Vec::new(),
            habit_list: StatefulList::new(),
            schedule_entries: Vec::new(),
            schedule_list: StatefulList::new(),
            settings: None,
            create_fields: vec![String::new(); 3],
        }
    }

    pub async fn load_initial(&mut self) {
        self.reload_tasks().await;
        self.reload_schedule().await;
        self.reload_habits().await;
        self.reload_settings().await;
    }

    pub async fn reload_tasks(&mut self) {
        // Keep an unfiltered list so the schedule tab can resolve tasks
        // regardless of the Tasks-tab filter.
        if let Ok(t) = self.app.list_tasks(&Default::default()).await {
            self.all_tasks = t;
            self.tasks = match self.task_filter.as_deref() {
                Some(filter) => self
                    .all_tasks
                    .iter()
                    .filter(|task| task.status.as_str() == filter)
                    .cloned()
                    .collect(),
                None => self.all_tasks.clone(),
            };
            self.task_list.set_len(self.tasks.len());
        }
    }

    /// Lazily load the comment timeline for a single task. Only the task
    /// currently shown in the detail pane is fetched, avoiding an N+1 query
    /// over all tasks on every reload. `CommentState` distinguishes not-yet-
    /// loaded, in-flight, loaded, and failed so the UI never mistakes an error
    /// for an empty timeline (WI-5).
    pub async fn ensure_comments(&mut self, task_id: &str) {
        match self.comments.get(task_id) {
            // Already loaded or in flight; nothing to do.
            Some(CommentState::Loaded(_))
            | Some(CommentState::Loading)
            | Some(CommentState::Empty) => return,
            // Allow a retry after a failure.
            Some(CommentState::Error) | None => {}
        }
        self.comments
            .insert(task_id.to_string(), CommentState::Loading);
        match self.app.list_comments(task_id).await {
            Ok(rows) => {
                let state = if rows.is_empty() {
                    CommentState::Empty
                } else {
                    CommentState::Loaded(rows)
                };
                self.comments.insert(task_id.to_string(), state);
            }
            Err(_) => {
                self.comments
                    .insert(task_id.to_string(), CommentState::Error);
            }
        }
    }

    pub async fn reload_schedule(&mut self) {
        if let Ok(s) = self.app.get_schedule().await {
            self.schedule_entries = s.schedule.as_inner().clone();
            self.schedule_entries.sort_by_key(|e| e.start_at);
            self.schedule_list.set_len(self.schedule_entries.len());
        }
    }

    pub async fn reload_habits(&mut self) {
        if let Ok(h) = self.app.list_habits().await {
            self.habits = h;
            self.habit_list.set_len(self.habits.len());
        }
    }

    pub async fn reload_settings(&mut self) {
        self.settings = self.app.get_settings().await.ok();
    }

    pub async fn on_tick(&mut self) {}

    pub async fn do_generate(&mut self) {
        let input = GenerateSchedule {
            task_ids: None,
            sleep: SleepInput::Recommended,
        };
        match self.app.generate_schedule(&input).await {
            Ok(_) => {
                self.status_msg = Some("Schedule generated".into());
                self.reload_schedule().await;
                self.reload_tasks().await;
            }
            Err(e) => self.status_msg = Some(format!("Error: {e}")),
        }
    }

    pub async fn do_reschedule(&mut self) {
        let input = Reschedule {
            mode: ScheduleMode::Range,
            from: None,
            until: None,
            task_ids: None,
            pinned: Vec::new(),
            sleep: SleepInput::Recommended,
        };
        match self.app.reschedule(&input).await {
            Ok(_) => {
                self.status_msg = Some("Rescheduled".into());
                self.reload_schedule().await;
                self.reload_tasks().await;
            }
            Err(e) => self.status_msg = Some(format!("Error: {e}")),
        }
    }

    /// Returns true if the app should quit.
    pub async fn handle_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut ratatui::DefaultTerminal,
    ) -> bool {
        if self.modal != Modal::None {
            return self.handle_modal_key(key).await;
        }

        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
            KeyCode::Char('?') => self.modal = Modal::Help,
            KeyCode::Char('1') => self.tab = Tab::Schedule,
            KeyCode::Char('l') | KeyCode::Tab => self.next_tab(),
            KeyCode::Char('h') => self.prev_tab(),
            KeyCode::Char('2') => self.tab = Tab::Tasks,
            KeyCode::Char('3') => self.tab = Tab::Habits,
            KeyCode::Char('4') => self.tab = Tab::Settings,
            KeyCode::BackTab => self.prev_tab(),
            _ => {}
        }

        match self.tab {
            Tab::Schedule => schedule::handle_key(self, key).await,
            Tab::Tasks => tasks::handle_key(self, key, terminal).await,
            Tab::Habits => habits::handle_key(self, key).await,
            Tab::Settings => settings::handle_key(self, key).await,
        }

        false
    }

    async fn handle_modal_key(&mut self, key: KeyEvent) -> bool {
        match self.modal {
            Modal::Help => {
                if matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
                ) {
                    self.modal = Modal::None;
                }
            }
            Modal::ConfirmDelete => match key.code {
                KeyCode::Char('y') => {
                    self.modal = Modal::None;
                    self.do_delete().await;
                }
                _ => self.modal = Modal::None,
            },
            Modal::CreateTask { ref mut field } => match key.code {
                KeyCode::Esc => self.modal = Modal::None,
                KeyCode::Enter => {
                    if *field < 2 {
                        *field += 1;
                    } else {
                        self.modal = Modal::None;
                        self.do_create_task().await;
                    }
                }
                KeyCode::BackTab | KeyCode::Up => {
                    if *field > 0 {
                        *field -= 1;
                    }
                }
                KeyCode::Tab | KeyCode::Down => {
                    if *field < 2 {
                        *field += 1;
                    }
                }
                KeyCode::Backspace => {
                    self.create_fields[*field].pop();
                }
                KeyCode::Char(c) => {
                    self.create_fields[*field].push(c);
                }
                _ => {}
            },
            Modal::None => {}
        }
        false
    }

    fn next_tab(&mut self) {
        let idx = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        self.tab = Tab::ALL[(idx + 1) % Tab::ALL.len()];
    }

    fn prev_tab(&mut self) {
        let idx = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        self.tab = Tab::ALL[(idx + Tab::ALL.len() - 1) % Tab::ALL.len()];
    }

    async fn do_delete(&mut self) {
        match self.tab {
            Tab::Tasks => {
                if let Some(task) = self.selected_task() {
                    let id = task.id.clone();
                    match self.app.delete_task(&id).await {
                        Ok(()) => {
                            self.status_msg = Some(format!("Deleted task {id}"));
                            self.reload_tasks().await;
                            self.reload_schedule().await;
                        }
                        Err(e) => self.status_msg = Some(format!("Error: {e}")),
                    }
                }
            }
            Tab::Habits => {
                if let Some(habit) = self.selected_habit() {
                    let id = habit.id.clone();
                    match self.app.delete_habit(&id).await {
                        Ok(()) => {
                            self.status_msg = Some(format!("Deleted habit {id}"));
                            self.reload_habits().await;
                        }
                        Err(e) => self.status_msg = Some(format!("Error: {e}")),
                    }
                }
            }
            _ => {}
        }
    }

    async fn do_create_task(&mut self) {
        let title = self.create_fields[0].clone();
        let end_at = self.create_fields[1].clone();
        let avg = self.create_fields[2].parse::<i64>().unwrap_or(30);
        if title.is_empty() || end_at.is_empty() {
            self.status_msg = Some("Title and deadline required".into());
            return;
        }
        let end_at = match takusu_types::Timestamp::parse_with_tz(&end_at, &self.tz) {
            Ok(ts) => ts,
            Err(e) => {
                self.status_msg = Some(format!("Invalid deadline: {e}"));
                return;
            }
        };
        let body = takusu_contracts::CreateTask {
            title,
            description: None,
            start_at: None,
            end_at,
            avg_minutes: avg,
            sigma_minutes: Some(10),
            depends: None,
            parallelizable: None,
            allows_parallel: None,
            abandonability: None,
            ical_uid: None,
            habit_id: None,
            fixed: None,
            habit_step_id: None,
            quantity_total: None,
            quantity_done: None,
            quantity_unit: None,
            original_quantity_total: None,
        };
        match self.app.create_task(&body).await {
            Ok(t) => {
                self.status_msg = Some(format!("Created task #{}", t.display_id));
                self.reload_tasks().await;
            }
            Err(e) => self.status_msg = Some(format!("Error: {e}")),
        }
        self.create_fields = vec![String::new(); 3];
    }

    pub fn selected_task(&self) -> Option<&TaskRow> {
        self.task_list.selected().and_then(|i| self.tasks.get(i))
    }

    pub fn selected_habit(&self) -> Option<&HabitRow> {
        self.habit_list.selected().and_then(|i| self.habits.get(i))
    }

    pub fn selected_entry(&self) -> Option<&ScheduleEntry> {
        self.schedule_list
            .selected()
            .and_then(|i| self.schedule_entries.get(i))
    }

    pub fn task_by_id(&self, id: &str) -> Option<&TaskRow> {
        self.all_tasks.iter().find(|t| t.id == id)
    }

    /// The id of the task shown in the right-hand detail pane, if any.
    fn detail_task_id(&self) -> Option<String> {
        match self.tab {
            Tab::Schedule => self
                .selected_entry()
                .and_then(|e| self.task_by_id(&e.task_id))
                .map(|t| t.id.clone()),
            Tab::Tasks => self.selected_task().map(|t| t.id.clone()),
            _ => None,
        }
    }

    /// Lazily load comments for the task currently in the detail pane (WI-5).
    pub async fn ensure_selected_comments(&mut self) {
        if let Some(id) = self.detail_task_id() {
            self.ensure_comments(&id).await;
        }
    }
}
