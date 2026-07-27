use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

pub const MAX_KEY_SCALARS: usize = 256;
pub const MAX_CONTENT_SCALARS: usize = 4096;
pub const MAX_QUERY_SCALARS: usize = 256;

/// Worst-case cap on candidate rows fetched from SQL for similar-task search.
/// The bigram pre-filter already narrows candidates sharply; this only guards
/// against a very common bigram transferring an unbounded row set. Far above
/// any personal-scale completed-task count, so it never drops a relevant match
/// in practice (#942).
pub const SIMILAR_TASK_CANDIDATE_CAP: usize = 10000;

/// Shared Unicode normalization for memory keys, content, and search queries.
///
/// 1. NFKC
/// 2. ASCII uppercase → lowercase (per scalar value)
/// 3. Unicode whitespace → ASCII space
/// 4. collapse consecutive spaces and trim ends
/// 5. reject empty / control-only values and values exceeding `max_scalars`
pub fn normalize_text(input: &str, max_scalars: Option<usize>) -> Result<String, String> {
    let mut buf = Vec::with_capacity(input.chars().count().max(1));
    for c in input.nfkc() {
        let c = if c.is_ascii_uppercase() {
            c.to_ascii_lowercase()
        } else {
            c
        };
        let c = if c.is_whitespace() && c != ' ' {
            ' '
        } else {
            c
        };
        buf.push(c);
    }

    let mut out = String::with_capacity(buf.len());
    let mut prev_space = true;
    for c in buf {
        if c == ' ' {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }

    if out.is_empty() {
        return Err("empty value".to_string());
    }
    if out.chars().all(|c| c.is_control()) {
        return Err("control characters only".to_string());
    }

    if let Some(max) = max_scalars {
        let count = out.chars().count();
        if count > max {
            return Err(format!("value exceeds {max} unicode scalar values"));
        }
    }

    Ok(out)
}

pub fn normalize_key(input: &str) -> Result<String, String> {
    normalize_text(input, Some(MAX_KEY_SCALARS))
}

pub fn normalize_content(input: &str) -> Result<String, String> {
    normalize_text(input, Some(MAX_CONTENT_SCALARS))
}

pub fn normalize_query(input: &str) -> Result<String, String> {
    normalize_text(input, Some(MAX_QUERY_SCALARS))
}

/// Build a set of distinct bigrams from a normalized string of scalar values.
pub fn bigrams(s: &str) -> HashSet<(char, char)> {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 2 {
        return HashSet::new();
    }
    chars
        .windows(2)
        .filter_map(|w| {
            let [a, b] = w else { return None };
            Some((*a, *b))
        })
        .collect()
}

/// Sørensen–Dice coefficient of two sets of bigrams, plus a substring bonus.
pub fn dice_coefficient(a: &HashSet<(char, char)>, b: &HashSet<(char, char)>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let intersection: f64 = a.intersection(b).count() as f64;
    (2.0 * intersection) / (a.len() + b.len()) as f64
}

fn similar_task_score_inner(q: &str, t: &str) -> Option<f64> {
    if t.is_empty() {
        return None;
    }

    let t_chars: Vec<char> = t.chars().collect();
    if t_chars.len() == 1 {
        return if t == q { Some(1.0) } else { None };
    }

    let q_bigrams = bigrams(q);
    let t_bigrams = bigrams(t);
    let mut score = dice_coefficient(&q_bigrams, &t_bigrams);
    if t.contains(q) {
        score += 0.25;
    }
    if score > 1.0 {
        score = 1.0;
    }
    if score <= 0.0 {
        return None;
    }
    Some(score)
}

/// Score a completed-task title against a query title.
/// Returns `None` when the title should be excluded from results.
pub fn similar_task_score(query: &str, title: &str) -> Option<f64> {
    let q = normalize_text(query, Some(MAX_QUERY_SCALARS)).ok()?;
    let t = normalize_text(title, Some(MAX_CONTENT_SCALARS)).ok()?;
    similar_task_score_inner(&q, &t)
}

/// Like [`similar_task_score`], but the `query` is already normalized so it
/// is not re-normalized for every title in a loop.
pub fn similar_task_score_pre_normalized(query: &str, title: &str) -> Option<f64> {
    let t = normalize_text(title, Some(MAX_CONTENT_SCALARS)).ok()?;
    similar_task_score_inner(query, &t)
}

/// Trait for items that can be ranked by `rank_memories`.
pub trait MemoryRankable {
    fn id(&self) -> &str;
    fn normalized_key(&self) -> &str;
    fn normalized_content(&self) -> &str;
    fn updated_at(&self) -> String;
}

/// Rank key match quality; lower is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchRank {
    Exact = 0,
    Prefix = 1,
    KeySubstring = 2,
    ContentSubstring = 3,
    NoMatch = 4,
}

fn term_parts(term: &str) -> Vec<&str> {
    term.split('*').filter(|p| !p.is_empty()).collect()
}

/// Check that the literal parts of a (possibly wildcard) term appear in `text`
/// in the same order. `*` is the only wildcard and matches any sequence.
fn term_matches(term: &str, text: &str) -> bool {
    if !term.contains('*') {
        return text.contains(term);
    }
    let parts = term_parts(term);
    if parts.is_empty() {
        return true; // term is just "*"
    }
    let mut start = 0usize;
    for part in parts {
        if let Some(pos) = text[start..].find(part) {
            start += pos + part.len();
        } else {
            return false;
        }
    }
    true
}

fn key_equals_term(term: &str, key: &str) -> bool {
    if !term.contains('*') {
        return key == term;
    }
    let parts = term_parts(term);
    if parts.is_empty() {
        return true;
    }
    if !key.starts_with(parts[0]) {
        return false;
    }
    let mut start = parts[0].len();
    for part in &parts[1..] {
        if let Some(pos) = key[start..].find(part) {
            start += pos + part.len();
        } else {
            return false;
        }
    }
    if term.ends_with('*') {
        true
    } else {
        key.ends_with(parts.last().unwrap())
    }
}

fn key_starts_with_term(term: &str, key: &str) -> bool {
    if !term.contains('*') {
        return key.starts_with(term);
    }
    let parts = term_parts(term);
    if parts.is_empty() {
        return true;
    }
    if !key.starts_with(parts[0]) {
        return false;
    }
    let mut start = parts[0].len();
    for part in &parts[1..] {
        if let Some(pos) = key[start..].find(part) {
            start += pos + part.len();
        } else {
            return false;
        }
    }
    true
}

fn match_rank(term: &str, item: &dyn MemoryRankable) -> MatchRank {
    let key = item.normalized_key();
    let content = item.normalized_content();
    if key_equals_term(term, key) {
        MatchRank::Exact
    } else if key_starts_with_term(term, key) {
        MatchRank::Prefix
    } else if term_matches(term, key) {
        MatchRank::KeySubstring
    } else if term_matches(term, content) {
        MatchRank::ContentSubstring
    } else {
        MatchRank::NoMatch
    }
}

fn item_score(terms: &[String], item: &dyn MemoryRankable) -> (u8, usize, usize) {
    let mut sum = 0u8;
    let mut key_hits = 0usize;
    let mut content_hits = 0usize;
    for term in terms {
        let rank = match_rank(term, item);
        sum += rank as u8;
        if rank <= MatchRank::KeySubstring {
            key_hits += 1;
        } else if rank == MatchRank::ContentSubstring {
            content_hits += 1;
        }
    }
    (sum, key_hits, content_hits)
}

/// Tokenize `query` into normalized whitespace-separated terms. Multiple
/// terms are combined with AND semantics by the caller (e.g. every term must
/// match either the key or the content). `*` in a term acts as a wildcard
/// matching any sequence of characters.
pub fn tokenize_query(query: &str) -> Result<Vec<String>, String> {
    let q = normalize_query(query)?;
    Ok(q.split_whitespace().map(|s| s.to_string()).collect())
}

/// Build SQL `LIKE` patterns for each term. `*` is converted to `%` and other
/// LIKE metacharacters (`%`, `_`, `\`) are escaped. The resulting patterns are
/// always wrapped with leading/trailing `%` so they form a superset of the
/// strings matched by [`term_matches`]: any text that `term_matches` accepts is
/// guaranteed to satisfy the generated `LIKE` pattern.
pub fn memory_like_patterns(terms: &[String]) -> Vec<String> {
    terms
        .iter()
        .map(|term| {
            if term.contains('*') {
                let parts: Vec<String> = term
                    .split('*')
                    .filter(|seg| !seg.is_empty())
                    .map(escape_like_pattern)
                    .collect();
                if parts.is_empty() {
                    "%".to_string()
                } else {
                    format!("%{}%", parts.join("%"))
                }
            } else {
                format!("%{}%", escape_like_pattern(term))
            }
        })
        .collect()
}

/// Sort `items` by the deterministic memory search ranking in place.
/// After sorting, the caller should truncate to the desired `limit`.
pub fn sort_memories<T: MemoryRankable>(query: &str, items: &mut [T]) {
    let terms = match tokenize_query(query) {
        Ok(t) => t,
        Err(_) => {
            items.sort_by(|a, b| {
                b.updated_at()
                    .cmp(&a.updated_at())
                    .then_with(|| a.id().cmp(b.id()))
            });
            return;
        }
    };
    if terms.is_empty() {
        items.sort_by(|a, b| {
            b.updated_at()
                .cmp(&a.updated_at())
                .then_with(|| a.id().cmp(b.id()))
        });
        return;
    }
    items.sort_by(|a, b| {
        let (sa, ka, ca) = item_score(&terms, a);
        let (sb, kb, cb) = item_score(&terms, b);
        sa.cmp(&sb)
            .then_with(|| kb.cmp(&ka))
            .then_with(|| cb.cmp(&ca))
            .then_with(|| b.updated_at().cmp(&a.updated_at()))
            .then_with(|| a.id().cmp(b.id()))
    });
}

/// Compare two optional strings in descending order (`None` is "largest").
pub fn compare_optional_desc(a: &Option<String>, b: &Option<String>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(x), Some(y)) => y.cmp(x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

/// Escape a normalized query so it can be used as a literal inside a SQL
/// `LIKE` pattern. The escape character is `\`.
pub fn escape_like_pattern(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '%' | '_' | '\\' => vec!['\\', c],
            other => vec![other],
        })
        .collect()
}

/// Build SQL `LIKE` patterns (wrapped in `%…%`, escaped for the `\` escape
/// character) that pre-filter candidate titles for similar-task search.
///
/// The patterns form a strict superset of the titles that would score non-zero
/// under [`similar_task_score_pre_normalized`], so using them as an SQL
/// pre-filter never drops a true match:
/// - a non-zero Dice score requires at least one shared bigram, i.e. the title
///   contains that bigram as a substring;
/// - the substring bonus for a query of length ≥ 2 likewise implies the title
///   contains a query bigram;
/// - a single-scalar query has no bigrams, so we fall back to a single-character
///   containment pattern (the substring bonus is its only scoring path).
///
/// Callers join the patterns with `OR` and bind each one as a SQL parameter
/// (never interpolate them into the SQL text). The result is sorted and
/// deduplicated for deterministic SQL.
pub fn similar_task_filter_patterns(normalized_query: &str) -> Vec<String> {
    let bg = bigrams(normalized_query);
    let mut out: Vec<String> = if bg.is_empty() {
        if normalized_query.is_empty() {
            return Vec::new();
        }
        vec![format!("%{}%", escape_like_pattern(normalized_query))]
    } else {
        bg.into_iter()
            .map(|(a, b)| {
                format!(
                    "%{}%",
                    escape_like_pattern(&[a, b].iter().collect::<String>())
                )
            })
            .collect()
    };
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_nfkc_case_and_whitespace() {
        let s = "  ＡＢＣ　  研究室  \t\n  ";
        let out = normalize_text(s, Some(100)).unwrap();
        assert_eq!(out, "abc 研究室");
    }

    #[test]
    fn normalize_rejects_empty() {
        assert!(normalize_text("   ", None).is_err());
        assert!(normalize_text("\t\n", None).is_err());
    }

    #[test]
    fn normalize_enforces_max() {
        let s = "a".repeat(300);
        assert!(normalize_text(&s, Some(256)).is_err());
        assert!(normalize_text(&s, Some(300)).is_ok());
    }

    #[test]
    fn bigram_basic() {
        let s = "abc";
        let b = bigrams(s);
        assert!(b.contains(&('a', 'b')));
        assert!(b.contains(&('b', 'c')));
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn dice_identical() {
        let a = bigrams("abc");
        let b = bigrams("abc");
        assert!((dice_coefficient(&a, &b) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn similar_task_one_scalar_exact() {
        assert_eq!(similar_task_score("a", "a"), Some(1.0));
        assert_eq!(similar_task_score("a", "b"), None);
    }

    #[test]
    fn similar_task_substring_bonus() {
        let score = similar_task_score("数学の演習", "数学の演習問題を解く").unwrap();
        assert!(score > 0.25);
    }

    #[test]
    fn similar_task_excludes_empty_title() {
        assert_eq!(similar_task_score("x", "!!!"), None);
    }

    #[test]
    fn filter_patterns_are_superset_of_dice_matches() {
        // A reordered title shares bigrams but no substring with the query; the
        // patterns must still match it (recall safety).
        let q = normalize_query("数学演習").unwrap();
        let title = normalize_text("演習数学", Some(MAX_CONTENT_SCALARS)).unwrap();
        assert!(similar_task_score_pre_normalized(&q, &title).is_some());
        let pats = similar_task_filter_patterns(&q);
        assert!(
            pats.iter()
                .any(|p| title.contains(p.trim_start_matches('%').trim_end_matches('%')))
        );
    }

    #[test]
    fn filter_patterns_single_scalar_fallback() {
        let pats = similar_task_filter_patterns("x");
        assert_eq!(pats, vec!["%x%".to_string()]);
    }

    #[test]
    fn filter_patterns_empty_query() {
        assert!(similar_task_filter_patterns("").is_empty());
    }

    #[test]
    fn filter_patterns_escape_like_metachars() {
        // A bigram containing '%' must be escaped so it is matched literally.
        let pats = similar_task_filter_patterns("a%b");
        assert!(pats.contains(&"%a\\%%".to_string()));
        assert!(pats.contains(&"%\\%b%".to_string()));
    }

    #[test]
    fn filter_patterns_sorted_and_deduped() {
        let pats = similar_task_filter_patterns("aaaa");
        // Only one distinct bigram ("aa").
        assert_eq!(pats, vec!["%aa%".to_string()]);
    }

    #[test]
    fn memory_ranking() {
        struct M {
            id: String,
            key: String,
            content: String,
            updated: String,
        }
        impl MemoryRankable for M {
            fn id(&self) -> &str {
                &self.id
            }
            fn normalized_key(&self) -> &str {
                &self.key
            }
            fn normalized_content(&self) -> &str {
                &self.content
            }
            fn updated_at(&self) -> String {
                self.updated.clone()
            }
        }
        let mut items = vec![
            M {
                id: "1".into(),
                key: "研究室".into(),
                content: "...".into(),
                updated: "2025-01-02T00:00:00Z".into(),
            },
            M {
                id: "2".into(),
                key: "研究室".into(),
                content: "...".into(),
                updated: "2025-01-01T00:00:00Z".into(),
            },
            M {
                id: "3".into(),
                key: "研究室棟".into(),
                content: "...".into(),
                updated: "2025-01-03T00:00:00Z".into(),
            },
            M {
                id: "4".into(),
                key: "foo".into(),
                content: "研究室について".into(),
                updated: "2025-01-04T00:00:00Z".into(),
            },
        ];
        sort_memories("研究室", &mut items);
        assert_eq!(items[0].id, "1"); // exact, newer tie
        assert_eq!(items[1].id, "2"); // exact, older
        assert_eq!(items[2].id, "3"); // prefix
        assert_eq!(items[3].id, "4"); // content substring
    }

    #[test]
    fn tokenize_query_splits_keywords_and_keeps_wildcard() {
        let terms = tokenize_query("  研究室 *大学  ").unwrap();
        assert_eq!(terms, vec!["研究室", "*大学"]);
    }

    #[test]
    fn memory_like_patterns_handle_wildcards() {
        let patterns = memory_like_patterns(&[
            "研究室".to_string(),
            "大学*".to_string(),
            "*foo".to_string(),
            "a*b".to_string(),
        ]);
        assert_eq!(patterns[0], "%研究室%");
        assert_eq!(patterns[1], "%大学%");
        assert_eq!(patterns[2], "%foo%");
        assert_eq!(patterns[3], "%a%b%");
    }

    #[test]
    fn memory_like_patterns_escape_metacharacters() {
        let patterns = memory_like_patterns(&["a%b".to_string(), "c_d".to_string()]);
        assert_eq!(patterns[0], "%a\\%b%");
        assert_eq!(patterns[1], "%c\\_d%");
    }

    #[test]
    fn memory_ranking_multi_term_and_wildcard() {
        struct M {
            id: String,
            key: String,
            content: String,
            updated: String,
        }
        impl MemoryRankable for M {
            fn id(&self) -> &str {
                &self.id
            }
            fn normalized_key(&self) -> &str {
                &self.key
            }
            fn normalized_content(&self) -> &str {
                &self.content
            }
            fn updated_at(&self) -> String {
                self.updated.clone()
            }
        }
        let mut items = vec![
            M {
                id: "1".into(),
                key: "大学の研究室".into(),
                content: "".into(),
                updated: "2025-01-01T00:00:00Z".into(),
            },
            M {
                id: "2".into(),
                key: "研究室".into(),
                content: "大学".into(),
                updated: "2025-01-02T00:00:00Z".into(),
            },
            M {
                id: "3".into(),
                key: "研究".into(),
                content: "".into(),
                updated: "2025-01-03T00:00:00Z".into(),
            },
        ];
        sort_memories("研究* 大学", &mut items);
        // Both terms match in key of id 1; one term key one content for id 2; id 3 only first term.
        assert_eq!(items[0].id, "1");
        assert_eq!(items[1].id, "2");
        assert_eq!(items[2].id, "3");
    }

    #[test]
    fn memory_like_patterns_are_superset_of_term_matches() {
        // Parse a SQL LIKE pattern using the same rules as the worker/storage
        // queries (escape character is '\', wildcard is '%').
        fn like_matches(pattern: &str, text: &str) -> bool {
            #[derive(Debug, Clone)]
            enum Token {
                Wildcard,
                Literal(String),
            }

            let mut tokens = Vec::new();
            let mut current = String::new();
            let mut chars = pattern.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    } else {
                        current.push('\\');
                    }
                } else if c == '%' {
                    if !current.is_empty() {
                        tokens.push(Token::Literal(current.clone()));
                        current.clear();
                    }
                    tokens.push(Token::Wildcard);
                } else {
                    current.push(c);
                }
            }
            if !current.is_empty() {
                tokens.push(Token::Literal(current));
            }

            // Collapse consecutive wildcards.
            let mut collapsed = Vec::new();
            for t in tokens {
                if matches!(t, Token::Wildcard) && matches!(collapsed.last(), Some(Token::Wildcard))
                {
                    continue;
                }
                collapsed.push(t);
            }

            fn match_tokens(tokens: &[Token], text: &str) -> bool {
                if tokens.is_empty() {
                    return text.is_empty();
                }
                match &tokens[0] {
                    Token::Literal(lit) => {
                        text.starts_with(lit) && match_tokens(&tokens[1..], &text[lit.len()..])
                    }
                    Token::Wildcard => {
                        if tokens.len() == 1 {
                            return true;
                        }
                        let lit = match &tokens[1] {
                            Token::Literal(l) => l,
                            Token::Wildcard => return match_tokens(&tokens[1..], text),
                        };
                        for i in 0..=text.len() {
                            let suffix = &text[i..];
                            if suffix.starts_with(lit)
                                && match_tokens(&tokens[2..], &suffix[lit.len()..])
                            {
                                return true;
                            }
                        }
                        false
                    }
                }
            }

            match_tokens(&collapsed, text)
        }

        fn generate_strings(chars: &[char], max_len: usize) -> Vec<String> {
            let mut out = Vec::new();
            fn rec(chars: &[char], max_len: usize, prefix: &mut String, out: &mut Vec<String>) {
                if prefix.len() == max_len {
                    out.push(prefix.clone());
                    return;
                }
                for &c in chars {
                    prefix.push(c);
                    rec(chars, max_len, prefix, out);
                    prefix.pop();
                }
            }
            rec(chars, max_len, &mut String::new(), &mut out);
            out
        }

        let term_chars = ['a', 'b', '*'];
        let text_chars = ['a', 'b'];
        let mut term_count = 0;
        let mut text_count = 0;

        for term_len in 1..=4 {
            for term in generate_strings(&term_chars, term_len) {
                term_count += 1;
                let patterns = memory_like_patterns(std::slice::from_ref(&term));
                let pat = &patterns[0];
                for text_len in 0..=5 {
                    for text in generate_strings(&text_chars, text_len) {
                        text_count += 1;
                        if term_matches(&term, &text) {
                            assert!(
                                like_matches(pat, &text),
                                "term {term:?} matches text {text:?} but pattern {pat:?} does not"
                            );
                        }
                    }
                }
            }
        }

        // Guard against accidental empty enumeration; we should have exercised
        // a non-trivial search space.
        assert!(term_count > 100, "expected many terms, got {term_count}");
        assert!(text_count > 100, "expected many texts, got {text_count}");
    }
}
