//! Streaming text-to-speech block accumulator.
//!
//! [`TtsQueue`] consumes raw `Text` event deltas from a streaming LLM turn and
//! emits speakable TTS blocks. Blocks are flushed at sentence boundaries while
//! streaming, on thinking/tool-call interruptions, at paragraph breaks, and at
//! the end of the turn, so read-aloud starts as soon as the first sentence is
//! ready. Code fences that span multiple flushes are tracked so their contents
//! are not read aloud.

/// Accumulates raw `Text` event deltas from a streaming turn and emits
/// speakable TTS blocks. Blocks are flushed at sentence boundaries while
/// streaming, on thinking/tool-call interruptions, at paragraph breaks, and at
/// the end of the turn, so read-aloud starts as soon as the first sentence is
/// ready. Code fences that span multiple flushes are tracked so their contents
/// are not read aloud.
#[derive(Debug, Default)]
pub struct TtsQueue {
    buffer: String,
    line: String,
    code_state: CodeState,
    buffer_visible_len: usize,
    line_visible_len: usize,
    /// Whether the last completed line was blank, i.e. a paragraph separator.
    prev_line_blank: bool,
}

/// Target visible characters to keep in a single TTS block. When a sentence has
/// no terminator before this limit, clause boundaries (commas, etc.) are used
/// as a soft flush point.
const TTS_BLOCK_TARGET_VISIBLE_LEN: usize = 240;
/// Hard upper bound on visible characters in a single TTS block. The queue
/// tries not to split a short continuation line right at this limit, so a token
/// like "1.5" is not torn apart when the buffer reaches the cap.
const TTS_BLOCK_MAX_VISIBLE_LEN: usize = 480;
/// How far beyond the hard limit the current line may grow before the queue
/// flushes anyway, even without a natural boundary.
const TTS_BLOCK_DEFER_BUDGET: usize = 80;

#[derive(Debug, Default)]
enum CodeState {
    #[default]
    Outside,
    InCodeFence {
        marker: char,
        length: usize,
    },
}

impl TtsQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a `Text` event delta to the queue and return any completed TTS
    /// blocks produced while scanning the delta.
    ///
    /// The delta is scanned with a one-character lookahead so boundaries like
    /// `1.5` vs `1. `, `1,000` vs `1, `, and `1:00` vs `Note: ` can be told
    /// apart using the next character in the stream.
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        let mut blocks = Vec::new();
        let mut chars = delta.char_indices().peekable();
        while let Some((_, ch)) = chars.next() {
            if ch == '\n' {
                let line = std::mem::take(&mut self.line);
                let visible = self.line_visible_len;
                self.line_visible_len = 0;
                // Blank lines inside code fences should not be treated as
                // paragraph separators for the text before the fence.
                if matches!(self.code_state, CodeState::Outside) {
                    self.prev_line_blank = line.trim().is_empty();
                }
                self.process_line(line.trim_end_matches('\r'), visible);
                let next = chars.peek().map(|(_, c)| *c);
                if let Some(block) = self.try_flush(next) {
                    blocks.push(block);
                }
            } else {
                self.line.push(ch);
                if !ch.is_whitespace() {
                    self.line_visible_len += 1;
                }
                let next = chars.peek().map(|(_, c)| *c);
                if let Some(block) = self.try_flush(next) {
                    blocks.push(block);
                }
            }
        }
        blocks
    }

    /// Extract accumulated text as a TTS block and clear the buffer.
    ///
    /// Returns `None` if the buffer is empty or the filtered text contains
    /// only whitespace.
    pub fn flush(&mut self) -> Option<String> {
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            let visible = self.line_visible_len;
            self.line_visible_len = 0;
            self.process_line(line.trim_end_matches('\r'), visible);
        }
        let text = std::mem::take(&mut self.buffer);
        self.buffer_visible_len = 0;
        let speech = markdown_to_speech(&text);
        if speech.trim().is_empty() {
            None
        } else {
            Some(speech)
        }
    }

    fn try_flush(&mut self, next: Option<char>) -> Option<String> {
        if !self.should_flush(next) {
            return None;
        }
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            let visible = self.line_visible_len;
            self.line_visible_len = 0;
            self.process_line(line.trim_end_matches('\r'), visible);
        }
        let text = std::mem::take(&mut self.buffer);
        self.buffer_visible_len = 0;
        let speech = markdown_to_speech(&text);
        if speech.trim().is_empty() {
            None
        } else {
            Some(speech)
        }
    }

    fn should_flush(&self, next: Option<char>) -> bool {
        let line_trimmed = self.line.trim_end();
        let has_line_ws = self.line.len() > line_trimmed.len();
        let buf_trimmed = self.buffer.trim_end();
        let has_buf_ws = self.buffer.len() > buf_trimmed.len();
        let total = self.buffer_visible_len + self.line_visible_len;

        if line_trimmed.is_empty() {
            if self.prev_line_blank && self.buffer_visible_len > 0 {
                return true;
            }
            if is_sentence_boundary(buf_trimmed, next, has_buf_ws) {
                return true;
            }
            if total >= TTS_BLOCK_TARGET_VISIBLE_LEN
                && is_clause_boundary(buf_trimmed, next, has_buf_ws)
            {
                return true;
            }
        } else {
            if is_sentence_boundary(line_trimmed, next, has_line_ws) {
                return true;
            }
            if total >= TTS_BLOCK_TARGET_VISIBLE_LEN
                && is_clause_boundary(line_trimmed, next, has_line_ws)
            {
                return true;
            }
        }

        if total < TTS_BLOCK_MAX_VISIBLE_LEN {
            return false;
        }

        // Do not split a number or decimal token right at the hard limit.
        // Defer briefly so "1.5" is not torn into "1" and ".5...".
        let over_max = total - TTS_BLOCK_MAX_VISIBLE_LEN;
        if over_max < TTS_BLOCK_DEFER_BUDGET && line_continues_token(&self.line, next) {
            return false;
        }

        true
    }

    fn process_line(&mut self, line: &str, visible: usize) {
        match self.code_state {
            CodeState::Outside => {
                if let Some((marker, length)) = opening_fence(line) {
                    self.code_state = CodeState::InCodeFence { marker, length };
                } else if line.trim().is_empty() {
                    self.buffer.push('\n');
                } else {
                    self.buffer_visible_len += visible;
                    self.buffer.push_str(line);
                    self.buffer.push('\n');
                }
            }
            CodeState::InCodeFence { marker, length } => {
                if is_code_fence(line, marker, length) {
                    self.code_state = CodeState::Outside;
                }
            }
        }
    }
}

fn is_sentence_end(ch: char) -> bool {
    matches!(
        ch,
        '.' | '?' | '!' | '。' | '！' | '？' | '\u{FF0E}' // ． (fullwidth full stop)
    )
}

fn is_period_like(ch: char) -> bool {
    matches!(ch, '.' | '\u{FF0E}')
}

/// Return true if `c` is a character that can continue a number token when it
/// follows a digit, or when a digit follows it (e.g. decimal points,
/// thousands separators, time separators, Japanese enumeration commas).
fn is_number_continuation(c: char) -> bool {
    matches!(
        c,
        '.' | '\u{FF0E}' // ．
            | ',' | '\u{FF0C}' // ，
            | ':' | '\u{FF1A}' // ：
            | ';' | '\u{FF1B}' // ；
            | '、'
    )
}

/// Return true if the trimmed text ends at a clause boundary.
fn is_clause_boundary(s: &str, next: Option<char>, trailing_ws: bool) -> bool {
    let mut chars = s.chars().rev();
    let last = chars.next();
    match last {
        Some('、') => {
            let prev = chars.next();
            // 、 between digits is number enumeration ("1、2"); don't split it.
            if prev.is_some_and(|c| c.is_numeric()) && next.is_some_and(|c| c.is_numeric()) {
                return false;
            }
            // Unknown next after digit+、; defer to avoid splitting enumeration.
            if prev.is_some_and(|c| c.is_numeric()) && next.is_none() && !trailing_ws {
                return false;
            }
            true
        }
        Some(',' | '，' | ':' | ';' | '\u{FF1A}' | '\u{FF1B}') => {
            let prev = chars.next();
            if prev.is_some_and(|c| c.is_numeric()) {
                // If it looks like a thousands separator or time, don't flush.
                // "1,000" / "1:00" / "1;30" keep the separator with the number.
                if next.is_some_and(|c| c.is_numeric()) {
                    return false;
                }
                // We don't know what comes after "1," / "1:" yet, so defer
                // to avoid splitting a number across deltas.
                if next.is_none() && !trailing_ws {
                    return false;
                }
            }
            // Only treat as a clause boundary when the separator is actually
            // followed by whitespace (or already has trailing whitespace and
            // the next delta starts a new word). This avoids splitting
            // "foo,bar" or "note:here" at the punctuation.
            trailing_ws || next.is_some_and(|c| c.is_whitespace())
        }
        _ => false,
    }
}

/// Return true if the current line ends in the middle of a number or decimal
/// token, so the queue should defer a hard flush to avoid splitting "1.5" or
/// "1,000".
fn line_continues_token(line: &str, next: Option<char>) -> bool {
    let s = line.trim_end();
    if s.is_empty() {
        return false;
    }
    if is_inside_version_token(s, next) {
        return true;
    }
    let mut chars = s.chars().rev();
    let last = chars.next();
    match last {
        Some(c) if c.is_numeric() => {
            // A number continues if the next char is another digit or a
            // separator that may introduce a decimal/thousands/time/clause.
            next.is_none_or(|n| n.is_numeric() || is_number_continuation(n))
        }
        Some(c) if is_number_continuation(c) => {
            // A separator continues the token only when it is between digits
            // (or at the end of a delta, where we can't tell yet).
            let prev = chars.next();
            prev.is_some_and(|c| c.is_numeric()) && next.is_none_or(|n| n.is_numeric())
        }
        _ => false,
    }
}

/// Return true when `s` ends inside a version/label token like "v1.x" or
/// "1.bc": the last period-like separator is preceded by a digit, and the
/// characters after it (plus the next char, if known) are alphanumeric.
fn is_inside_version_token(s: &str, next: Option<char>) -> bool {
    for (i, c) in s.char_indices().rev() {
        if !is_period_like(c) {
            continue;
        }
        let before = &s[..i];
        let after = &s[i + c.len_utf8()..];
        if before.chars().next_back().is_some_and(|c| c.is_numeric())
            && after.chars().all(|c| c.is_alphanumeric())
            && next.is_none_or(|n| n.is_alphanumeric())
        {
            return true;
        }
        // Only the rightmost period-like separator can end the current token.
        break;
    }
    false
}

/// Return true if the trimmed text ends at a sentence boundary.
///
/// `next` is the character immediately following `s` in the current delta, and
/// `trailing_ws` is true when `s` is already followed by whitespace (e.g. a
/// space was pushed after the period, or a newline sits between `s` and the
/// next delta). This lets us distinguish:
///
/// - "3.5" (decimal) from "3. " (sentence end)
/// - "v1.x" / "a1.b" (version/label) from "3. Next" (sentence end)
fn is_sentence_boundary(s: &str, next: Option<char>, trailing_ws: bool) -> bool {
    let last = match s.chars().next_back() {
        Some(c) => c,
        None => return false,
    };
    if !is_sentence_end(last) {
        return false;
    }
    // Non-period terminators are unambiguous.
    if !is_period_like(last) {
        return true;
    }
    let without_last = s.strip_suffix(last).unwrap_or(s);
    // A trailing period that is part of a decimal/ordinal/version token
    // ("3.5", "v1.x", "a1.") is not a sentence boundary.
    if let Some(prev) = without_last.chars().next_back()
        && prev.is_numeric()
    {
        // A digit immediately followed by another digit (no whitespace) is a
        // decimal. If there is already whitespace after the period, it's not.
        if next.is_some_and(|c| c.is_numeric()) && !trailing_ws {
            return false;
        }
        // A digit followed by a period and an alphabetic char with no
        // intervening whitespace is likely a version/label (v1.x, a1.b).
        if next.is_some_and(|c| c.is_alphabetic()) && !trailing_ws {
            return false;
        }
        // We don't know what follows "3." at the end of a delta, so defer to
        // avoid splitting a decimal like "3.5" across deltas.
        if next.is_none() && !trailing_ws {
            return false;
        }
        // A digit followed by a period and then whitespace/punctuation, or a
        // period already followed by whitespace, ends a sentence.
        return true;
    }
    let token = without_last
        .rsplit_once(char::is_whitespace)
        .map(|(_, w)| w)
        .unwrap_or(without_last)
        .trim_matches(|c: char| c.is_ascii_punctuation() && c != '.');
    if token.is_empty() {
        return false;
    }
    let token = token.to_lowercase();
    const ABBREVIATIONS: &[&str] = &[
        "mr", "mrs", "ms", "miss", "dr", "prof", "sr", "jr", "st", "ave", "blvd", "rd", "no", "vs",
        "etc", "eg", "e.g", "ie", "i.e", "et", "al", "co", "ltd", "inc", "corp", "plc", "llc",
        "llp", "fig", "vol", "ed", "pp", "ph", "phd", "md", "ba", "ma", "esq", "dept", "univ",
        "est", "jan", "feb", "mar", "apr", "jun", "jul", "aug", "sep", "sept", "oct", "nov", "dec",
        "mon", "tue", "wed", "thu", "fri", "sat", "sun", "am", "pm", "a.m", "p.m", "a", "p", "m",
    ];
    !ABBREVIATIONS.contains(&token.as_str())
}

fn opening_fence(line: &str) -> Option<(char, usize)> {
    let spaces = line.bytes().take_while(|&b| b == b' ').count();
    if spaces > 3 {
        return None;
    }
    let rest = &line[spaces..];
    let first = rest.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let run = rest.chars().take_while(|&c| c == first).count();
    if run < 3 {
        return None;
    }
    let after = &rest[run..];
    if after.contains(first) {
        return None;
    }
    Some((first, run))
}

fn is_code_fence(line: &str, marker: char, min_len: usize) -> bool {
    let spaces = line.bytes().take_while(|&b| b == b' ').count();
    if spaces > 3 {
        return false;
    }
    let rest = &line[spaces..];
    let run = rest.chars().take_while(|&c| c == marker).count();
    if run < min_len {
        return false;
    }
    let after = &rest[run..];
    after.chars().all(|c| c == ' ')
}

/// Strip markdown markup that should not be read aloud (code blocks, HTML,
/// thematic breaks, images, etc.) while keeping inline text and inline code.
fn markdown_to_speech(text: &str) -> String {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};

    let parser = Parser::new(text);
    let mut parts = Vec::new();
    let mut in_code_block = false;
    for event in parser {
        match event {
            Event::Start(Tag::CodeBlock(_)) => in_code_block = true,
            Event::End(TagEnd::CodeBlock) => in_code_block = false,
            Event::Text(s) if !in_code_block => parts.push(s.to_string()),
            Event::Code(s) => parts.push(s.to_string()),
            _ => {}
        }
    }
    parts
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tts_queue_accumulates_and_flushes_text() {
        let mut q = TtsQueue::new();
        q.push("hello ");
        q.push("world");
        assert_eq!(q.flush(), Some("hello world".to_string()));
        assert_eq!(q.flush(), None);
    }

    #[test]
    fn tts_queue_filters_code_blocks_for_speech() {
        let mut q = TtsQueue::new();
        q.push("hello \n```\nsecret\n```\n world");
        assert_eq!(q.flush(), Some("hello world".to_string()));
    }

    #[test]
    fn tts_queue_tracks_code_fence_across_flushes() {
        let mut q = TtsQueue::new();
        q.push("hello \n```\ncode");
        assert_eq!(q.flush(), Some("hello".to_string()));
        q.push("more\n```\nworld");
        assert_eq!(q.flush(), Some("world".to_string()));
    }

    #[test]
    fn tts_queue_keeps_inline_text_and_code() {
        let mut q = TtsQueue::new();
        q.push("use `foo` for *bar* and **baz**");
        assert_eq!(q.flush(), Some("use foo for bar and baz".to_string()));
    }

    #[test]
    fn tts_queue_handles_crlf_newlines() {
        let mut q = TtsQueue::new();
        q.push("hello \r\nworld");
        assert_eq!(q.flush(), Some("hello world".to_string()));
    }

    #[test]
    fn tts_queue_strips_list_and_quote_markers() {
        let mut q = TtsQueue::new();
        q.push("items:\n- one\n- two\n> quoted");
        assert_eq!(q.flush(), Some("items: one two quoted".to_string()));
    }

    #[test]
    fn tts_queue_flushes_at_sentence_boundaries() {
        let mut q = TtsQueue::new();
        let blocks = q.push("Hello. World! Foo");
        assert_eq!(blocks, vec!["Hello.", "World!"]);
        assert_eq!(q.flush(), Some("Foo".to_string()));
    }

    #[test]
    fn tts_queue_flushes_japanese_sentence_boundaries() {
        let mut q = TtsQueue::new();
        let blocks = q.push("こんにちは。明日は晴れですか？はい");
        assert_eq!(blocks, vec!["こんにちは。", "明日は晴れですか？"]);
        assert_eq!(q.flush(), Some("はい".to_string()));
    }

    #[test]
    fn tts_queue_ignores_abbreviation_dots() {
        let mut q = TtsQueue::new();
        let blocks = q.push("See Dr. Smith at 3 p.m. today. Bye!");
        assert_eq!(blocks, vec!["See Dr. Smith at 3 p.m. today.", "Bye!"]);
    }

    #[test]
    fn tts_queue_enforces_max_length_across_lines() {
        let mut q = TtsQueue::new();
        let first = "a".repeat(TTS_BLOCK_MAX_VISIBLE_LEN - 1);
        let blocks = q.push(&format!("{}\nb", first));
        assert_eq!(blocks.len(), 1);
        // markdown_to_speech collapses the newline to a space, so the flushed
        // string is one byte longer than the visible-char budget.
        assert_eq!(blocks[0].len(), TTS_BLOCK_MAX_VISIBLE_LEN + 1);
    }

    #[test]
    fn tts_queue_defers_hard_flush_for_number_token() {
        // The hard cap is reached at the start of a number, but the queue
        // should defer until the number (or its continuation) finishes instead
        // of splitting it.
        let mut q = TtsQueue::new();
        let prefix = "a".repeat(TTS_BLOCK_MAX_VISIBLE_LEN - 1);
        let tail = "1".to_string() + &"2".repeat(TTS_BLOCK_DEFER_BUDGET);
        let blocks = q.push(&format!("{}{}", prefix, tail));
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].len() > TTS_BLOCK_MAX_VISIBLE_LEN);
        assert!(blocks[0].len() <= TTS_BLOCK_MAX_VISIBLE_LEN + TTS_BLOCK_DEFER_BUDGET);
        assert!(q.flush().is_none());
    }

    #[test]
    fn tts_queue_flushes_at_max_length_even_without_terminator() {
        let mut q = TtsQueue::new();
        let long = "a".repeat(TTS_BLOCK_MAX_VISIBLE_LEN + 10);
        let blocks = q.push(&long);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].len(), TTS_BLOCK_MAX_VISIBLE_LEN);
        let remaining = q.flush().unwrap_or_default();
        assert_eq!(remaining.len(), 10);
    }

    #[test]
    fn tts_queue_does_not_treat_decimal_point_as_sentence_boundary() {
        let mut q = TtsQueue::new();
        let blocks = q.push("1.5時間かかります");
        // No sentence terminator and well under the max length, so the whole
        // number should stay in one block.
        assert!(blocks.is_empty());
        assert_eq!(q.flush(), Some("1.5時間かかります".to_string()));
    }

    #[test]
    fn tts_queue_keeps_decimal_number_after_hard_length_cap() {
        // Fill the buffer to just before the hard cap, then append a decimal
        // number followed by more text. The cap should not split "1.5" into
        // "1" and ".5...".
        let mut q = TtsQueue::new();
        let prefix = "a".repeat(TTS_BLOCK_MAX_VISIBLE_LEN - 1);
        let tail = "1.5".to_string() + &"x".repeat(TTS_BLOCK_DEFER_BUDGET);
        let blocks = q.push(&format!("{}{}", prefix, tail));
        assert_eq!(blocks.len(), 1);
        // The number "1.5" stays in the same flushed block, and the defer
        // budget keeps the block within the allowed extension.
        assert!(blocks[0].contains("1.5"));
        assert!(blocks[0].len() > TTS_BLOCK_MAX_VISIBLE_LEN);
        assert!(blocks[0].len() <= TTS_BLOCK_MAX_VISIBLE_LEN + TTS_BLOCK_DEFER_BUDGET);
        // Remaining text is not a fragment of the decimal like ".5" or "5x".
        let remaining = q.flush().unwrap_or_default();
        assert!(remaining.starts_with('x'));
    }

    #[test]
    fn tts_queue_flushes_at_clause_boundaries() {
        let mut q = TtsQueue::new();
        let head = "a".repeat(TTS_BLOCK_TARGET_VISIBLE_LEN - 1);
        let blocks = q.push(&format!("{}, world", head));
        assert_eq!(blocks.len(), 1);
        // The flushed block should contain the clause boundary (comma) and the
        // remainder should start with the next word.
        assert!(blocks[0].contains(','));
        assert_eq!(q.flush(), Some("world".to_string()));
    }

    #[test]
    fn tts_queue_flushes_at_paragraph_breaks() {
        let mut q = TtsQueue::new();
        let blocks = q.push("first paragraph\n\nsecond paragraph");
        assert_eq!(blocks, vec!["first paragraph".to_string()]);
        assert_eq!(q.flush(), Some("second paragraph".to_string()));
    }

    #[test]
    fn tts_queue_ignores_blank_lines_inside_code_fences() {
        let mut q = TtsQueue::new();
        let blocks = q.push("intro\n```\n\n\n```\noutro");
        // The blank lines inside the fence should not flush "intro" early.
        assert!(blocks.is_empty());
        assert_eq!(q.flush(), Some("intro outro".to_string()));
    }

    #[test]
    fn tts_queue_does_not_split_fullwidth_decimal_numbers() {
        let mut q = TtsQueue::new();
        let blocks = q.push("１.５時間かかります");
        assert!(blocks.is_empty());
        assert_eq!(q.flush(), Some("１.５時間かかります".to_string()));
    }

    #[test]
    fn tts_queue_does_not_split_thousands_separator() {
        // The comma in "1,000" is right at the soft-flush target, but it
        // should not be treated as a clause boundary and split the number.
        let mut q = TtsQueue::new();
        let head = "a".repeat(TTS_BLOCK_TARGET_VISIBLE_LEN - 1);
        let blocks = q.push(&format!("{}1,000 more text", head));
        assert!(blocks.is_empty());
        assert!(q.flush().unwrap_or_default().contains("1,000"));
    }

    #[test]
    fn tts_queue_flushes_list_commas_as_clause_boundaries() {
        // A comma followed by a space is a clause boundary, even if the comma
        // is preceded by a digit. Only commas followed directly by digits are
        // treated as thousands separators.
        let mut q = TtsQueue::new();
        let head = "a".repeat(TTS_BLOCK_TARGET_VISIBLE_LEN - 1);
        let blocks = q.push(&format!("{}1, 2, 3", head));
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].ends_with("1,"));
        assert_eq!(q.flush(), Some("2, 3".to_string()));
    }

    #[test]
    fn tts_queue_does_not_split_time_notation() {
        let mut q = TtsQueue::new();
        let head = "a".repeat(TTS_BLOCK_TARGET_VISIBLE_LEN - 1);
        let blocks = q.push(&format!("{}1:00 later", head));
        assert!(blocks.is_empty());
        assert!(q.flush().unwrap_or_default().contains("1:00"));
    }

    #[test]
    fn tts_queue_flushes_number_ending_sentence_with_lookahead() {
        // "The answer is 3." should flush at the period because the next
        // character is whitespace, not a digit that would continue a decimal.
        let mut q = TtsQueue::new();
        let blocks = q.push("The answer is 3. Next");
        assert_eq!(blocks, vec!["The answer is 3.".to_string()]);
        assert_eq!(q.flush(), Some("Next".to_string()));
    }

    #[test]
    fn tts_queue_defers_ambiguous_decimal_across_deltas() {
        // A number + period at the end of a delta is ambiguous. It should be
        // deferred until the next delta arrives, so "Value 3." + "5hours"
        // becomes "Value 3.5hours" (decimal), while "Value 3." + " Next"
        // flushes "Value 3." as a sentence and leaves "Next" for the next block.
        let mut q = TtsQueue::new();
        let blocks = q.push("Value 3.");
        assert!(blocks.is_empty());
        let blocks = q.push("5hours");
        assert!(blocks.is_empty());
        assert_eq!(q.flush(), Some("Value 3.5hours".to_string()));

        let mut q = TtsQueue::new();
        let blocks = q.push("Value 3.");
        assert!(blocks.is_empty());
        let blocks = q.push(" Next");
        assert_eq!(blocks, vec!["Value 3.".to_string()]);
        assert_eq!(q.flush(), Some("Next".to_string()));
    }

    #[test]
    fn tts_queue_does_not_split_version_label_at_hard_limit() {
        // A period preceded by a digit and followed by letters is a version/
        // label token; the hard-limit defer should keep "1.bc" together.
        let mut q = TtsQueue::new();
        let prefix = "a".repeat(TTS_BLOCK_MAX_VISIBLE_LEN - 1);
        let tail = "1.bc".to_string() + &"x".repeat(TTS_BLOCK_DEFER_BUDGET);
        let blocks = q.push(&format!("{}{}", prefix, tail));
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].contains("1.bc"));
        assert!(!blocks[0].ends_with("1."));
        assert!(q.flush().unwrap_or_default().starts_with('x'));
    }

    #[test]
    fn tts_queue_does_not_split_japanese_number_enumeration() {
        // 、 between digits is enumeration, not a clause boundary.
        let mut q = TtsQueue::new();
        let head = "a".repeat(TTS_BLOCK_TARGET_VISIBLE_LEN - 1);
        let blocks = q.push(&format!("{}１、２、３", head));
        assert!(blocks.is_empty());
        assert!(q.flush().unwrap_or_default().contains("１、２、３"));
    }

    #[test]
    fn tts_queue_does_not_split_clause_chars_without_whitespace() {
        // Without whitespace after a clause char, it is part of the current
        // token rather than a boundary.
        let mut q = TtsQueue::new();
        let head = "a".repeat(TTS_BLOCK_TARGET_VISIBLE_LEN - 1);
        let blocks = q.push(&format!("{}foo,bar note:here", head));
        assert!(blocks.is_empty());
        let remaining = q.flush().unwrap_or_default();
        assert!(remaining.contains("foo,bar"));
        assert!(remaining.contains("note:here"));
    }

    #[test]
    fn markdown_to_speech_strips_code_blocks_and_html() {
        assert_eq!(
            markdown_to_speech("hello \n```\nsecret\n```\n world"),
            "hello world",
        );
        assert_eq!(markdown_to_speech("text <br> more"), "text more",);
    }

    #[test]
    fn markdown_to_speech_strips_heading_list_and_quote_markers() {
        assert_eq!(markdown_to_speech("# heading\ntext"), "heading text");
        assert_eq!(
            markdown_to_speech("- item one\n- item two"),
            "item one item two"
        );
        assert_eq!(markdown_to_speech("> quoted"), "quoted");
    }
}
