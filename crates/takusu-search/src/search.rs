use std::collections::HashMap;

use jiff::{Timestamp, civil::Date, tz::TimeZone};
use serde::{Deserialize, Serialize};

use crate::date::parse_date_expression_with;

/// A task that can be searched.
pub trait Task {
    fn id(&self) -> &str;
    fn display_id(&self) -> i64;
    fn title(&self) -> &str;
    fn description(&self) -> Option<&str>;
    fn status(&self) -> &str;
    fn start_at(&self) -> Option<&str>;
    fn end_at(&self) -> &str;
    fn depends(&self) -> &str;
    fn habit_id(&self) -> Option<&str>;
    fn fixed(&self) -> bool;
    fn parallelizable(&self) -> bool;
    fn allows_parallel(&self) -> bool;
    fn completed_at(&self) -> Option<&str>;

    fn scheduled_start<'c>(&self, ctx: &'c EvalContext) -> Option<&'c str> {
        ctx.schedule.get(self.id()).map(|(s, _)| s.as_str())
    }

    fn scheduled_end<'c>(&self, ctx: &'c EvalContext) -> Option<&'c str> {
        ctx.schedule.get(self.id()).map(|(_, e)| e.as_str())
    }
}

/// A habit that can be referenced from a query.
pub trait Habit {
    fn id(&self) -> &str;
    fn display_id(&self) -> i64;
}

/// Implement `search::Task` for a row type with the expected fields.
#[macro_export]
macro_rules! impl_search_task {
    ($ty:ty) => {
        impl $crate::search::Task for $ty {
            fn id(&self) -> &str {
                &self.id
            }
            fn display_id(&self) -> i64 {
                self.display_id
            }
            fn title(&self) -> &str {
                &self.title
            }
            fn description(&self) -> Option<&str> {
                self.description.as_deref()
            }
            fn status(&self) -> &str {
                self.status.as_str()
            }
            fn start_at(&self) -> Option<&str> {
                self.start_at.as_deref()
            }
            fn end_at(&self) -> &str {
                &self.end_at
            }
            fn depends(&self) -> &str {
                &self.depends
            }
            fn habit_id(&self) -> Option<&str> {
                self.habit_id.as_deref()
            }
            fn fixed(&self) -> bool {
                self.fixed
            }
            fn parallelizable(&self) -> bool {
                self.parallelizable
            }
            fn allows_parallel(&self) -> bool {
                self.allows_parallel
            }
            fn completed_at(&self) -> Option<&str> {
                self.completed_at.as_deref()
            }
        }
    };
}

/// Implement `search::Habit` for a row type with the expected fields.
#[macro_export]
macro_rules! impl_search_habit {
    ($ty:ty) => {
        impl $crate::search::Habit for $ty {
            fn id(&self) -> &str {
                &self.id
            }
            fn display_id(&self) -> i64 {
                self.display_id
            }
        }
    };
}

/// Evaluation context for a query.
pub struct EvalContext {
    pub now: Timestamp,
    pub tz: TimeZone,
    pub(crate) schedule: HashMap<String, (String, String)>,
    pub(crate) task_ref_to_id: HashMap<String, String>,
    pub(crate) task_id_to_display: HashMap<String, i64>,
    pub(crate) habit_ref_to_id: HashMap<String, String>,
    pub(crate) habit_id_to_display: HashMap<String, i64>,
    /// For each task id, the set of task ids that depend on it.
    pub(crate) dependents: HashMap<String, std::collections::HashSet<String>>,
}

impl EvalContext {
    pub fn new<S, T, H>(
        tz: TimeZone,
        now: Timestamp,
        schedule: S,
        tasks: &[T],
        habits: &[H],
    ) -> Self
    where
        S: IntoIterator<Item = (String, (String, String))>,
        T: Task,
        H: Habit,
    {
        let mut task_ref_to_id = HashMap::new();
        let mut task_id_to_display = HashMap::new();
        let mut dependents: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
        for t in tasks {
            let id = t.id().to_string();
            let display_id = t.display_id();
            task_ref_to_id.insert(format!("#{display_id}"), id.clone());
            task_ref_to_id.insert(display_id.to_string(), id.clone());
            task_id_to_display.insert(id.clone(), display_id);

            if let Ok(ids) = serde_json::from_str::<Vec<String>>(t.depends()) {
                for dep in ids {
                    dependents.entry(dep).or_default().insert(id.clone());
                }
            }
        }

        let mut habit_ref_to_id = HashMap::new();
        let mut habit_id_to_display = HashMap::new();
        for h in habits {
            let id = h.id().to_string();
            let display_id = h.display_id();
            habit_ref_to_id.insert(format!("h{display_id}"), id.clone());
            habit_id_to_display.insert(id, display_id);
        }
        Self {
            now,
            tz,
            schedule: schedule.into_iter().collect(),
            task_ref_to_id,
            task_id_to_display,
            habit_ref_to_id,
            habit_id_to_display,
            dependents,
        }
    }

    /// Build a context with no schedule/habit data (used mainly for completion
    /// where only `today` matters).
    pub fn empty(tz: TimeZone, now: Timestamp) -> Self {
        Self {
            now,
            tz,
            schedule: HashMap::new(),
            task_ref_to_id: HashMap::new(),
            task_id_to_display: HashMap::new(),
            habit_ref_to_id: HashMap::new(),
            habit_id_to_display: HashMap::new(),
            dependents: HashMap::new(),
        }
    }

    fn today(&self) -> Date {
        self.now.to_zoned(self.tz.clone()).date()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Completion {
    /// Full query value after selecting this completion.
    pub value: String,
    /// Label shown in the completion UI.
    pub label: String,
}

// ── Tokenizer ───────────────────────────────────────────

#[derive(Debug, Clone)]
enum TokenKind {
    Word(String),
    Qualifier(String, String),
    LParen,
    RParen,
    Op(String),
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    start: usize,
    end: usize,
}

fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut i = 0;
    let chars: Vec<char> = input.chars().collect();
    let mut char_indices: Vec<usize> = input.char_indices().map(|(i, _)| i).collect();
    char_indices.push(input.len());

    while i < chars.len() {
        let start = char_indices[i];
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '(' {
            tokens.push(Token {
                kind: TokenKind::LParen,
                start,
                end: char_indices[i + 1],
            });
            i += 1;
            continue;
        }
        if c == ')' {
            tokens.push(Token {
                kind: TokenKind::RParen,
                start,
                end: char_indices[i + 1],
            });
            i += 1;
            continue;
        }
        if c == '"' {
            i += 1;
            let quote_start = char_indices[i];
            let mut s = String::new();
            while i < chars.len() && chars[i] != '"' {
                s.push(chars[i]);
                i += 1;
            }
            let end = if i < chars.len() {
                i += 1;
                char_indices[i]
            } else {
                input.len()
            };
            tokens.push(Token {
                kind: TokenKind::Word(s),
                start: quote_start,
                end,
            });
            continue;
        }

        let word_start = start;
        while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '(' && chars[i] != ')' {
            i += 1;
        }
        let word_end = char_indices[i];
        let raw = &input[word_start..word_end];
        if raw.is_empty() {
            continue;
        }

        // Leading '-' => NOT, then process the rest.
        if let Some(rest) = raw.strip_prefix('-').filter(|r| !r.is_empty()) {
            tokens.push(Token {
                kind: TokenKind::Op("NOT".to_string()),
                start: word_start,
                end: word_start + '-'.len_utf8(),
            });
            let rest_start = word_start + '-'.len_utf8();
            tokens.extend(classify_word(rest, rest_start, word_end));
        } else {
            tokens.extend(classify_word(raw, word_start, word_end));
        }
    }

    tokens
}

fn classify_word(word: &str, start: usize, end: usize) -> Vec<Token> {
    let upper = word.to_uppercase();
    if upper == "OR" || upper == "AND" || upper == "NOT" {
        return vec![Token {
            kind: TokenKind::Op(upper),
            start,
            end,
        }];
    }
    if let Some(colon) = word.find(':') {
        let key = word[..colon].to_string();
        let value = word[colon + 1..].to_string();
        return vec![Token {
            kind: TokenKind::Qualifier(key, value),
            start,
            end,
        }];
    }
    vec![Token {
        kind: TokenKind::Word(word.to_string()),
        start,
        end,
    }]
}

// ── Parser / AST ────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Qualifier { key: String, value: String },
    Text(String),
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn consume(&mut self) -> Option<&Token> {
        let t = self.tokens.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn match_op(&mut self, op: &str) -> bool {
        if let Some(t) = self.peek()
            && let TokenKind::Op(v) = &t.kind
            && v == op
        {
            self.pos += 1;
            return true;
        }
        false
    }

    fn parse_expression(&mut self) -> Result<Expr, String> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while self.match_op("OR") {
            if self.pos >= self.tokens.len() {
                break;
            }
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        loop {
            if self.pos >= self.tokens.len() {
                break;
            }
            if let Some(t) = self.peek() {
                match &t.kind {
                    TokenKind::Op(v) if v == "OR" => break,
                    TokenKind::RParen => break,
                    TokenKind::Op(v) if v == "AND" => {
                        self.pos += 1;
                        if self.pos >= self.tokens.len() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if self.pos >= self.tokens.len() {
                break;
            }
            let right = self.parse_unary()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if self.pos >= self.tokens.len() {
            return Ok(Expr::Text(String::new()));
        }
        if let Some(t) = self.peek()
            && let TokenKind::Op(v) = &t.kind
            && v == "NOT"
        {
            self.pos += 1;
            let expr = self.parse_unary()?;
            return Ok(Expr::Not(Box::new(expr)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        if self.pos >= self.tokens.len() {
            return Ok(Expr::Text(String::new()));
        }
        let t = self.consume().unwrap();
        match &t.kind {
            TokenKind::LParen => {
                let expr = self.parse_expression()?;
                match self.peek() {
                    Some(nt) if matches!(nt.kind, TokenKind::RParen) => {
                        self.pos += 1;
                        Ok(expr)
                    }
                    _ => Err("missing closing parenthesis".to_string()),
                }
            }
            TokenKind::Qualifier(key, value) => Ok(Expr::Qualifier {
                key: key.clone(),
                value: value.clone(),
            }),
            TokenKind::Word(w) => Ok(Expr::Text(w.to_lowercase())),
            TokenKind::RParen | TokenKind::Op(_) => Err(format!(
                "unexpected token at position {}: {:?}",
                t.start, t.kind
            )),
        }
    }
}

/// Parse a query string into an expression.
pub fn parse(input: &str) -> Result<Expr, String> {
    let tokens = tokenize(input);
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.parse_expression()?;
    if parser.pos != parser.tokens.len() {
        return Err("unexpected trailing tokens".to_string());
    }
    Ok(expr)
}

// ── Evaluation ──────────────────────────────────────────

pub fn filter_tasks<T: Task>(
    tasks: Vec<T>,
    query: &str,
    ctx: &EvalContext,
) -> Result<Vec<T>, String> {
    let expr = parse(query)?;
    Ok(tasks.into_iter().filter(|t| eval(&expr, t, ctx)).collect())
}

fn eval<T: Task>(expr: &Expr, task: &T, ctx: &EvalContext) -> bool {
    match expr {
        Expr::And(a, b) => eval(a, task, ctx) && eval(b, task, ctx),
        Expr::Or(a, b) => eval(a, task, ctx) || eval(b, task, ctx),
        Expr::Not(a) => !eval(a, task, ctx),
        Expr::Text(s) => {
            if s.is_empty() {
                return true;
            }
            let title = task.title().to_lowercase();
            if title.contains(s) {
                return true;
            }
            task.description()
                .map(|d| d.to_lowercase().contains(s))
                .unwrap_or(false)
        }
        Expr::Qualifier { key, value } => eval_qualifier(key, value, task, ctx),
    }
}

fn task_dep_ids(task: &impl Task) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(task.depends()).unwrap_or_default()
}

fn resolve_task_ref<'c>(ctx: &'c EvalContext, v: &'c str) -> Option<&'c str> {
    let v = v.trim();
    if let Some(id) = ctx.task_ref_to_id.get(v) {
        return Some(id.as_str());
    }
    if let Some(id) = ctx.task_ref_to_id.get(&format!("#{v}")) {
        return Some(id.as_str());
    }
    if ctx.task_id_to_display.contains_key(v) {
        return Some(v);
    }
    None
}

fn eval_compare(value: &str, against: i64) -> bool {
    let (op, rest) = parse_op_value(value);
    let Ok(n) = rest.trim().parse::<i64>() else {
        return false;
    };
    match op {
        ">" => against > n,
        ">=" => against >= n,
        "<" => against < n,
        "<=" => against <= n,
        "=" | "" => against == n,
        _ => false,
    }
}

fn eval_qualifier<T: Task>(key: &str, value: &str, task: &T, ctx: &EvalContext) -> bool {
    let v = value.trim();
    let vl = v.to_lowercase();
    match key {
        "status" => {
            if v == "overdue" {
                task.status() != "completed"
                    && task.status() != "skipped"
                    && timestamp_lt(task.end_at(), &ctx.now)
            } else {
                task.status() == v
            }
        }
        "title" => task.title().to_lowercase().contains(&vl),
        "desc" | "description" => task
            .description()
            .map(|d| d.to_lowercase().contains(&vl))
            .unwrap_or(false),
        "from" => eval_date(task.end_at(), v, ctx, ">="),
        "until" => task
            .start_at()
            .map(|s| eval_date(s, v, ctx, "<="))
            .unwrap_or(true),
        "start" => eval_date(task.start_at().unwrap_or(""), v, ctx, default_op_for(key)),
        "end" => eval_date(task.end_at(), v, ctx, default_op_for(key)),
        "scheduled-start" => eval_date(
            task.scheduled_start(ctx).unwrap_or(""),
            v,
            ctx,
            default_op_for(key),
        ),
        "scheduled-end" => eval_date(
            task.scheduled_end(ctx).unwrap_or(""),
            v,
            ctx,
            default_op_for(key),
        ),
        "habit" => {
            let wanted = v.strip_prefix('h').unwrap_or(v);
            if let Some(id) = ctx.habit_ref_to_id.get(v) {
                task.habit_id() == Some(id.as_str())
            } else if let Ok(num) = wanted.parse::<i64>() {
                ctx.habit_id_to_display
                    .iter()
                    .any(|(id, disp)| *disp == num && task.habit_id() == Some(id.as_str()))
            } else {
                false
            }
        }
        "depends" => {
            let Some(target) = resolve_task_ref(ctx, v) else {
                return false;
            };
            task_dep_ids(task).contains(&target.to_string())
        }
        "dependents" => {
            let Some(target) = resolve_task_ref(ctx, v) else {
                return false;
            };
            ctx.dependents
                .get(target)
                .map(|set| set.contains(task.id()))
                .unwrap_or(false)
        }
        "deps_count" => eval_compare(v, task_dep_ids(task).len() as i64),
        "is" => match v {
            "overdue" => {
                task.status() != "completed"
                    && task.status() != "skipped"
                    && timestamp_lt(task.end_at(), &ctx.now)
            }
            "fixed" => task.fixed(),
            "parallelizable" => task.parallelizable(),
            "allows_parallel" => task.allows_parallel(),
            _ => false,
        },
        "has" => match v {
            "description" => task
                .description()
                .map(|d| !d.trim().is_empty())
                .unwrap_or(false),
            "completed_at" => task.completed_at().is_some(),
            "schedule" => task.scheduled_start(ctx).is_some(),
            "depends" => !task_dep_ids(task).is_empty(),
            _ => false,
        },
        _ => false,
    }
}

fn default_op_for(key: &str) -> &'static str {
    match key {
        "start" | "scheduled-start" | "from" => ">=",
        "end" | "scheduled-end" | "until" => "<=",
        _ => "=",
    }
}

fn eval_date(task_value: &str, value: &str, ctx: &EvalContext, default_op: &str) -> bool {
    let task_ts = match task_value.parse::<Timestamp>() {
        Ok(t) => t,
        Err(_) => return false,
    };
    let today = ctx.today();
    let tz = &ctx.tz;
    let now = &ctx.now;

    // ".." range: "2026-07-25..2026-07-28"
    if let Some((l, r)) = value.split_once("..") {
        let start = match parse_date_expression_with(l, tz, today, false, *now) {
            Ok(t) => t,
            Err(_) => return false,
        };
        let end = match parse_date_expression_with(r, tz, today, true, *now) {
            Ok(t) => t,
            Err(_) => return false,
        };
        return task_ts >= start && task_ts <= end;
    }

    let (op, rest) = parse_op_value(value);
    let op = if op.is_empty() { default_op } else { op };

    let start = match parse_date_expression_with(rest, tz, today, false, *now) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let end = match parse_date_expression_with(rest, tz, today, true, *now) {
        Ok(t) => t,
        Err(_) => return false,
    };

    match op {
        ">" => task_ts > end,
        ">=" => task_ts >= start,
        "<" => task_ts < start,
        "<=" => task_ts <= end,
        "=" => task_ts >= start && task_ts <= end,
        _ => false,
    }
}

fn timestamp_lt(s: &str, now: &Timestamp) -> bool {
    s.parse::<Timestamp>().map(|t| t < *now).unwrap_or(false)
}

/// Returns (operator, value_without_operator). The operator is empty when none
/// is present, so the caller can apply its own default.
fn parse_op_value(value: &str) -> (&str, &str) {
    if let Some(rest) = value.strip_prefix(">=") {
        return (">=", rest);
    }
    if let Some(rest) = value.strip_prefix("<=") {
        return ("<=", rest);
    }
    if let Some(rest) = value.strip_prefix('>') {
        return (">", rest);
    }
    if let Some(rest) = value.strip_prefix('<') {
        return ("<", rest);
    }
    if let Some(rest) = value.strip_prefix('=') {
        return ("=", rest);
    }
    ("", value)
}

// ── Completion ──────────────────────────────────────────

const QUALIFIERS: &[(&str, &str)] = &[
    ("status", "status filter"),
    ("title", "text in title"),
    ("desc", "text in description"),
    ("description", "text in description"),
    ("start", "task start_at"),
    ("end", "task end_at"),
    ("scheduled-start", "scheduled start"),
    ("scheduled-end", "scheduled end"),
    ("from", "alias for end:>="),
    ("until", "alias for start:<="),
    ("habit", "habit reference"),
    ("depends", "task depends on ref (#N or UUID)"),
    ("dependents", "tasks that depend on this ref"),
    ("deps_count", "dependency count (e.g. >0, 2)"),
    ("is", "boolean / state flag"),
    ("has", "field exists"),
];

const STATUS_VALUES: &[&str] = &[
    "pending",
    "scheduled",
    "in_progress",
    "completed",
    "skipped",
    "overdue",
];
const IS_VALUES: &[&str] = &["fixed", "parallelizable", "allows_parallel", "overdue"];
const HAS_VALUES: &[&str] = &["description", "completed_at", "schedule", "depends"];

pub fn complete<T: Task, H: Habit>(
    input: &str,
    today: Date,
    tasks: &[T],
    habits: &[H],
    limit: Option<usize>,
) -> Vec<Completion> {
    let mut out = Vec::new();
    let max = limit.unwrap_or(usize::MAX);
    let tokens = tokenize(input);
    let (base, token) = last_token_bounds(input, &tokens);

    // Qualifier value completion.
    if let Some(colon) = token.find(':') {
        let key = &token[..colon];
        let val = &token[colon + 1..];
        match key {
            "status" => {
                for v in STATUS_VALUES {
                    if v.starts_with(val) && !push_completion(&mut out, max, &base, key, v, v) {
                        break;
                    }
                }
            }
            "is" => {
                for v in IS_VALUES {
                    if v.starts_with(val) && !push_completion(&mut out, max, &base, key, v, v) {
                        break;
                    }
                }
            }
            "has" => {
                for v in HAS_VALUES {
                    if v.starts_with(val) && !push_completion(&mut out, max, &base, key, v, v) {
                        break;
                    }
                }
            }
            "habit" => {
                for h in habits {
                    let ref_ = format!("h{}", h.display_id());
                    if ref_.starts_with(val)
                        && !push_completion(&mut out, max, &base, key, &ref_, &ref_)
                    {
                        break;
                    }
                }
            }
            "depends" | "dependents" => {
                for t in tasks {
                    let ref_ = format!("#{}", t.display_id());
                    if ref_.starts_with(val)
                        && !push_completion(&mut out, max, &base, key, &ref_, &ref_)
                    {
                        break;
                    }
                }
            }
            "start" | "end" | "scheduled-start" | "scheduled-end" | "from" | "until" => {
                for c in complete_date(val, today) {
                    if !push_completion(&mut out, max, &base, key, &c, &c) {
                        break;
                    }
                }
            }
            _ => {}
        }
        return out;
    }

    // Free word / qualifier name completion.
    let tl = token.to_lowercase();

    // Always show all qualifier names.
    for (q, _desc) in QUALIFIERS {
        let replacement = format!("{q}:");
        if !push_value(&mut out, max, &base, &replacement, &replacement) {
            break;
        }
    }

    // Title matches for free word.
    if !token.is_empty() {
        let mut seen = std::collections::HashSet::new();
        for t in tasks {
            if t.title().to_lowercase().contains(&tl) && seen.insert(t.title().to_string()) {
                let replacement = if t.title().contains(' ') {
                    format!("\"{}\"", t.title())
                } else {
                    t.title().to_string()
                };
                if !push_value(&mut out, max, &base, &replacement, t.title()) {
                    break;
                }
            }
        }
    }

    out
}

fn last_token_bounds<'a>(input: &'a str, tokens: &[Token]) -> (String, &'a str) {
    if input.is_empty() {
        return (String::new(), "");
    }
    if input.ends_with(|c: char| c.is_whitespace()) {
        return (input.to_string(), "");
    }
    if let Some(last) = tokens.last() {
        if matches!(last.kind, TokenKind::LParen | TokenKind::RParen) {
            return (input.to_string(), "");
        }
        let base = &input[..last.start];
        let token = &input[last.start..last.end];
        return (base.to_string(), token);
    }
    (input.to_string(), "")
}

fn push_completion(
    out: &mut Vec<Completion>,
    max: usize,
    base: &str,
    key: &str,
    value: &str,
    label: &str,
) -> bool {
    if out.len() >= max {
        return false;
    }
    let replacement = format!("{key}:{value}");
    push_value(out, max, base, &replacement, label)
}

fn push_value(
    out: &mut Vec<Completion>,
    max: usize,
    base: &str,
    replacement: &str,
    label: &str,
) -> bool {
    if out.len() >= max {
        return false;
    }
    let sep = if base.is_empty() {
        ""
    } else {
        let last = base.chars().last().unwrap();
        if last.is_whitespace() || last == '(' || last == '-' {
            ""
        } else {
            " "
        }
    };
    out.push(Completion {
        value: format!("{base}{sep}{replacement}"),
        label: label.to_string(),
    });
    true
}

fn complete_date(partial: &str, today: Date) -> Vec<String> {
    let mut out = Vec::new();
    let p = partial.trim();

    // Keywords.
    let keywords = ["today", "tomorrow", "yesterday"];
    for kw in &keywords {
        if kw.starts_with(p) {
            out.push(kw.to_string());
        }
    }

    // Full date already.
    if let Ok(d) = p.parse::<Date>() {
        out.push(d.to_string());
    }

    // MM-DD or M-D -> this year
    if let Some(idx) = p.find('-') {
        let month_str = &p[..idx];
        let day_str = &p[idx + 1..];
        if !month_str.is_empty()
            && month_str.chars().all(|c| c.is_ascii_digit())
            && !day_str.is_empty()
            && day_str.chars().all(|c| c.is_ascii_digit())
            && !month_str.contains('-')
            && !day_str.contains('-')
            && let (Ok(month), Ok(day)) = (month_str.parse::<i8>(), day_str.parse::<i8>())
            && let Ok(date) = Date::new(today.year(), month, day)
        {
            out.push(date.to_string());
        }
    }

    // DD or D -> this month
    if !p.is_empty()
        && p.chars().all(|c| c.is_ascii_digit())
        && !p.contains('-')
        && let Ok(day) = p.parse::<i8>()
        && let Ok(date) = Date::new(today.year(), today.month(), day)
    {
        out.push(date.to_string());
    }

    out
}

// ── Tests ───────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[derive(Debug, Clone)]
    struct TestTask {
        id: String,
        display_id: i64,
        title: String,
        description: Option<String>,
        status: String,
        start_at: Option<String>,
        end_at: String,
        depends: String,
        habit_id: Option<String>,
        fixed: bool,
        parallelizable: bool,
        allows_parallel: bool,
        completed_at: Option<String>,
    }

    impl Task for TestTask {
        fn id(&self) -> &str {
            &self.id
        }
        fn display_id(&self) -> i64 {
            self.display_id
        }
        fn title(&self) -> &str {
            &self.title
        }
        fn description(&self) -> Option<&str> {
            self.description.as_deref()
        }
        fn status(&self) -> &str {
            &self.status
        }
        fn start_at(&self) -> Option<&str> {
            self.start_at.as_deref()
        }
        fn end_at(&self) -> &str {
            &self.end_at
        }
        fn depends(&self) -> &str {
            &self.depends
        }
        fn habit_id(&self) -> Option<&str> {
            self.habit_id.as_deref()
        }
        fn fixed(&self) -> bool {
            self.fixed
        }
        fn parallelizable(&self) -> bool {
            self.parallelizable
        }
        fn allows_parallel(&self) -> bool {
            self.allows_parallel
        }
        fn completed_at(&self) -> Option<&str> {
            self.completed_at.as_deref()
        }
    }

    #[derive(Debug)]
    struct TestHabit {
        id: String,
        display_id: i64,
    }
    impl Habit for TestHabit {
        fn id(&self) -> &str {
            &self.id
        }
        fn display_id(&self) -> i64 {
            self.display_id
        }
    }

    fn test_ctx() -> EvalContext {
        let tasks = mk_tasks();
        EvalContext::new(
            TimeZone::UTC,
            Timestamp::from_str("2026-07-25T12:00:00Z").unwrap(),
            [(
                "t1".to_string(),
                (
                    "2026-07-25T09:00:00Z".to_string(),
                    "2026-07-25T12:00:00Z".to_string(),
                ),
            )],
            &tasks,
            &[TestHabit {
                id: "h1".to_string(),
                display_id: 1,
            }],
        )
    }

    fn mk_tasks() -> Vec<TestTask> {
        vec![
            TestTask {
                id: "t1".to_string(),
                display_id: 1,
                title: "朝の散歩".to_string(),
                description: None,
                status: "pending".to_string(),
                start_at: None,
                end_at: "2026-07-25T08:00:00Z".to_string(),
                depends: "[\"t2\"]".to_string(),
                habit_id: Some("h1".to_string()),
                fixed: false,
                parallelizable: false,
                allows_parallel: false,
                completed_at: None,
            },
            TestTask {
                id: "t2".to_string(),
                display_id: 2,
                title: "買い物リスト".to_string(),
                description: Some("卵、牛乳".to_string()),
                status: "pending".to_string(),
                start_at: None,
                end_at: "2026-07-25T18:00:00Z".to_string(),
                depends: "[]".to_string(),
                habit_id: None,
                fixed: false,
                parallelizable: false,
                allows_parallel: false,
                completed_at: None,
            },
            TestTask {
                id: "t3".to_string(),
                display_id: 3,
                title: "レポート".to_string(),
                description: None,
                status: "scheduled".to_string(),
                start_at: Some("2026-07-25T10:00:00Z".to_string()),
                end_at: "2026-07-25T17:00:00Z".to_string(),
                depends: "[\"t1\",\"t2\"]".to_string(),
                habit_id: None,
                fixed: true,
                parallelizable: false,
                allows_parallel: false,
                completed_at: None,
            },
        ]
    }

    #[test]
    fn parse_and_filter_status() {
        let ctx = test_ctx();
        let tasks = mk_tasks();
        let got = filter_tasks(tasks, "status:pending", &ctx).unwrap();
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn parse_or_and_not() {
        let ctx = test_ctx();
        let tasks = mk_tasks();
        let got = filter_tasks(tasks, "status:pending OR status:completed", &ctx).unwrap();
        assert_eq!(got.len(), 2);

        let tasks2 = mk_tasks();
        let got2 = filter_tasks(tasks2, "-status:pending 買い物", &ctx).unwrap();
        assert_eq!(got2.len(), 0); // 買い物 tasks are pending
    }

    #[test]
    fn date_filter_start_end() {
        let ctx = test_ctx();
        let tasks = mk_tasks();
        let got = filter_tasks(tasks, "start:>=2026-07-25", &ctx).unwrap();
        assert_eq!(got.len(), 1); // t3
    }

    #[test]
    fn scheduled_filter() {
        let ctx = test_ctx();
        let tasks = mk_tasks();
        let got = filter_tasks(tasks, "scheduled-start:>=2026-07-25", &ctx).unwrap();
        assert_eq!(got.len(), 1); // t1 has schedule
    }

    #[test]
    fn completion_qualifiers_always_shown() {
        let tasks = mk_tasks();
        let habits: Vec<TestHabit> = vec![TestHabit {
            id: "h1".to_string(),
            display_id: 1,
        }];
        let today = Date::new(2026, 7, 25).unwrap();
        let comps = complete("s", today, &tasks, &habits, None);
        assert!(
            comps
                .iter()
                .any(|c| c.value == "status:" || c.label == "status:")
        );
    }

    #[test]
    fn completion_date_day_and_month_day() {
        let tasks = mk_tasks();
        let habits: Vec<TestHabit> = vec![];
        let today = Date::new(2026, 7, 25).unwrap();
        let comps = complete("start:25", today, &tasks, &habits, None);
        assert!(comps.iter().any(|c| c.value.contains("2026-07-25")));
        let comps2 = complete("start:08-09", today, &tasks, &habits, None);
        assert!(comps2.iter().any(|c| c.value.contains("2026-08-09")));
    }

    #[test]
    fn completion_does_not_suggest_operators() {
        let tasks = mk_tasks();
        let habits: Vec<TestHabit> = vec![];
        let today = Date::new(2026, 7, 25).unwrap();
        let comps = complete("o", today, &tasks, &habits, None);
        assert!(
            !comps
                .iter()
                .any(|c| c.value == "OR" || c.value == "AND" || c.value == "NOT"),
            "operators should not appear as completion"
        );
    }

    #[test]
    fn completion_status_value_has_no_trailing_space() {
        let tasks = mk_tasks();
        let habits: Vec<TestHabit> = vec![];
        let today = Date::new(2026, 7, 25).unwrap();
        let comps = complete("status:p", today, &tasks, &habits, None);
        assert!(comps.iter().any(|c| c.value == "status:pending"));
        assert!(
            !comps.iter().any(|c| c.value.ends_with(' ')),
            "completed values should not end with a space"
        );
    }

    #[test]
    fn completion_always_shows_all_qualifiers() {
        let tasks = mk_tasks();
        let habits: Vec<TestHabit> = vec![];
        let today = Date::new(2026, 7, 25).unwrap();
        let comps = complete("foo", today, &tasks, &habits, None);
        for (q, _desc) in QUALIFIERS {
            assert!(
                comps.iter().any(|c| c.value == format!("{q}:")),
                "qualifier {q}: should always be shown"
            );
        }
    }

    #[test]
    fn filter_now_uses_context_now() {
        let mut ctx = test_ctx();
        // t3 starts at 2026-07-25T10:00:00Z. Set now to one minute after.
        ctx.now = Timestamp::from_str("2026-07-25T10:01:00Z").unwrap();
        let tasks = mk_tasks();
        let got = filter_tasks(tasks, "start:<=now", &ctx).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "t3");
    }

    #[test]
    fn parse_rejects_unmatched_lparenthesis() {
        assert!(parse("(").is_err());
        assert!(parse("(status:pending").is_err());
    }

    #[test]
    fn filter_depends_by_display_id_or_uuid() {
        let ctx = test_ctx();
        let tasks = mk_tasks();
        // t3 depends on t1 and t2; t1 depends on t2.
        let got = filter_tasks(tasks, "depends:1", &ctx).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "t3"); // t3 -> t1 (display_id 1)

        let tasks2 = mk_tasks();
        let got2 = filter_tasks(tasks2, &format!("depends:{}", "t2"), &ctx).unwrap();
        assert_eq!(got2.len(), 2);
    }

    #[test]
    fn filter_dependents_and_deps_count() {
        let ctx = test_ctx();
        let tasks = mk_tasks();
        // t3 depends on t1, so t3 is a dependent of t1.
        let got = filter_tasks(tasks, "dependents:1", &ctx).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "t3");

        let tasks2 = mk_tasks();
        let got2 = filter_tasks(tasks2, "deps_count:>0", &ctx).unwrap();
        assert_eq!(got2.len(), 2); // t1 and t3
    }

    #[test]
    fn has_depends_qualifier() {
        let ctx = test_ctx();
        let tasks = mk_tasks();
        let got = filter_tasks(tasks, "has:depends", &ctx).unwrap();
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn completion_limit_truncates_results() {
        let tasks = mk_tasks();
        let habits: Vec<TestHabit> = vec![];
        let today = Date::new(2026, 7, 25).unwrap();
        let comps = complete("foo", today, &tasks, &habits, Some(3));
        assert_eq!(comps.len(), 3);
    }

    #[test]
    fn completion_limit_truncates_qualifier_results() {
        let tasks = mk_tasks();
        let habits: Vec<TestHabit> = vec![];
        let today = Date::new(2026, 7, 25).unwrap();
        // "status:" matches all six status values, so a limit of 2 should truncate.
        let comps = complete("status:", today, &tasks, &habits, Some(2));
        assert_eq!(comps.len(), 2);
    }
}
