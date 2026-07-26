//! Normalize Japanese text for text-to-speech.
//!
//! This module converts dates, times, weekdays and proper nouns into forms
//! that TTS backends (Cartesia, Android system TTS, etc.) can read naturally.
//! It uses Lindera with the embedded IPADIC dictionary for proper-noun
//! readings, and regex-based rules for numeric/date patterns.

use std::borrow::Cow;
use std::sync::OnceLock;

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use lindera::tokenizer::Tokenizer;
use regex::{Captures, Regex};

/// Normalize `text` for the given language before sending it to TTS.
///
/// Currently supports `ja` (Japanese), locale-qualified tags such as
/// `ja-JP`, and the ISO 639-2 code `jpn`. Other languages are returned
/// unchanged (borrowed from `text`).
pub fn normalize_for_tts<'a>(text: &'a str, lang: &str) -> Cow<'a, str> {
    let primary = lang.split('-').next().unwrap_or(lang);
    if primary.eq_ignore_ascii_case("ja") || primary.eq_ignore_ascii_case("jpn") {
        Cow::Owned(normalize_ja(text))
    } else {
        Cow::Borrowed(text)
    }
}

fn normalize_ja(text: &str) -> String {
    let text = normalize_dates_and_times(text);
    normalize_proper_nouns(&text)
}

fn normalize_dates_and_times(text: &str) -> String {
    let text = normalize_iso_dates(text);
    let text = normalize_ja_dates_with_weekday(&text);
    let text = normalize_slash_dates_with_weekday(&text);
    let text = normalize_time_with_seconds(&text);
    normalize_time(&text)
}

/// Returns true when the match at `[start, end)` is not immediately preceded
/// or followed by a numeric character. This avoids matching inside longer
/// numbers without using regex look-around assertions (which the `regex`
/// crate does not support).
fn is_at_digit_boundary(text: &str, start: usize, end: usize) -> bool {
    let prev_is_numeric = text[..start]
        .chars()
        .next_back()
        .is_some_and(char::is_numeric);
    let next_is_numeric = text[end..].chars().next().is_some_and(char::is_numeric);
    !prev_is_numeric && !next_is_numeric
}

fn normalize_iso_dates(text: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?P<year>\d{4})[-/](?P<month>\d{1,2})[-/](?P<day>\d{1,2})").unwrap()
    });
    re.replace_all(text, |caps: &Captures| {
        let m = caps.get(0).unwrap();
        if !is_at_digit_boundary(text, m.start(), m.end()) {
            return caps[0].to_string();
        }
        let Ok(month) = caps["month"].parse::<u32>() else {
            return caps[0].to_string();
        };
        if !(1..=12).contains(&month) {
            return caps[0].to_string();
        }
        let Ok(day) = caps["day"].parse::<u32>() else {
            return caps[0].to_string();
        };
        if !(1..=31).contains(&day) {
            return caps[0].to_string();
        }
        let Ok(year) = caps["year"].parse::<u32>() else {
            return caps[0].to_string();
        };
        if day > days_in_month(month, year) {
            return caps[0].to_string();
        }
        format!("{}月{}日", month, day)
    })
    .into_owned()
}

fn normalize_ja_dates_with_weekday(text: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?P<month>\d{1,2})月(?P<day>\d{1,2})日\s*[(（]\s*(?P<wd>[日月火水木金土])\s*[)）]",
        )
        .unwrap()
    });
    re.replace_all(text, |caps: &Captures| {
        let m = caps.get(0).unwrap();
        if !is_at_digit_boundary(text, m.start(), m.end()) {
            return caps[0].to_string();
        }
        let Ok(month) = caps["month"].parse::<u32>() else {
            return caps[0].to_string();
        };
        if !(1..=12).contains(&month) {
            return caps[0].to_string();
        }
        let Ok(day) = caps["day"].parse::<u32>() else {
            return caps[0].to_string();
        };
        if day > days_in_month_for_unknown_year(month) {
            return caps[0].to_string();
        }
        let Some(wd) = weekday_full(&caps["wd"]) else {
            return caps[0].to_string();
        };
        format!("{}月{}日{}曜日", month, day, wd)
    })
    .into_owned()
}

fn normalize_slash_dates_with_weekday(text: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?P<month>\d{1,2})\s*/\s*(?P<day>\d{1,2})\s*[(（]\s*(?P<wd>[日月火水木金土])\s*[)）]",
        )
        .unwrap()
    });
    re.replace_all(text, |caps: &Captures| {
        let m = caps.get(0).unwrap();
        if !is_at_digit_boundary(text, m.start(), m.end()) {
            return caps[0].to_string();
        }
        let Ok(month) = caps["month"].parse::<u32>() else {
            return caps[0].to_string();
        };
        if !(1..=12).contains(&month) {
            return caps[0].to_string();
        }
        let Ok(day) = caps["day"].parse::<u32>() else {
            return caps[0].to_string();
        };
        if day > days_in_month_for_unknown_year(month) {
            return caps[0].to_string();
        }
        let Some(wd) = weekday_full(&caps["wd"]) else {
            return caps[0].to_string();
        };
        format!("{}月{}日{}曜日", month, day, wd)
    })
    .into_owned()
}

fn normalize_time_with_seconds(text: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE
        .get_or_init(|| Regex::new(r"T?(?P<hour>\d{1,2}):(?P<min>\d{2}):(?P<sec>\d{2})").unwrap());
    re.replace_all(text, |caps: &Captures| {
        let m = caps.get(0).unwrap();
        if !is_at_digit_boundary(text, m.start(), m.end()) {
            return caps[0].to_string();
        }
        let Ok(hour) = caps["hour"].parse::<u32>() else {
            return caps[0].to_string();
        };
        if hour > 23 {
            return caps[0].to_string();
        }
        let Ok(min) = caps["min"].parse::<u32>() else {
            return caps[0].to_string();
        };
        if min > 59 {
            return caps[0].to_string();
        }
        let Ok(sec) = caps["sec"].parse::<u32>() else {
            return caps[0].to_string();
        };
        if sec > 59 {
            return caps[0].to_string();
        }
        // Preserve the original two-digit seconds (e.g. "00") while
        // dropping leading zeros from hours and minutes.
        format!("{}時{}分{}秒", hour, min, &caps["sec"])
    })
    .into_owned()
}

fn normalize_time(text: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"T?(?P<hour>\d{1,2}):(?P<min>\d{2})").unwrap());
    re.replace_all(text, |caps: &Captures| {
        let m = caps.get(0).unwrap();
        if !is_at_digit_boundary(text, m.start(), m.end()) {
            return caps[0].to_string();
        }
        let Ok(hour) = caps["hour"].parse::<u32>() else {
            return caps[0].to_string();
        };
        if hour > 23 {
            return caps[0].to_string();
        }
        let Ok(min) = caps["min"].parse::<u32>() else {
            return caps[0].to_string();
        };
        if min > 59 {
            return caps[0].to_string();
        }
        format!("{}時{}分", hour, min)
    })
    .into_owned()
}

fn weekday_full(wd: &str) -> Option<&'static str> {
    match wd {
        "日" => Some("日"),
        "月" => Some("月"),
        "火" => Some("火"),
        "水" => Some("水"),
        "木" => Some("木"),
        "金" => Some("金"),
        "土" => Some("土"),
        _ => None,
    }
}

fn days_in_month(month: u32, year: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 31,
    }
}

// When the year is unknown we allow 29 February, because it exists in
// leap years and we cannot know whether the intended year is one.
fn days_in_month_for_unknown_year(month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => 29,
        _ => 31,
    }
}

fn is_leap_year(year: u32) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn normalize_proper_nouns(text: &str) -> String {
    static TOKENIZER: OnceLock<Option<Tokenizer>> = OnceLock::new();
    let tokenizer = TOKENIZER.get_or_init(init_tokenizer);
    let Some(tokenizer) = tokenizer.as_ref() else {
        return text.to_string();
    };
    let Ok(mut tokens) = tokenizer.tokenize(text) else {
        return text.to_string();
    };

    let mut output = String::with_capacity(text.len());
    for token in tokens.iter_mut() {
        let mut replaced = false;
        {
            // IPADIC word details are a comma-separated vector. In the
            // embedded ipadic dictionary the 8th field (index 7) is the
            // katakana reading (yomi). We additionally require the value to
            // consist only of kana so that a future dictionary format change
            // does not silently emit garbage.
            let details = token.details();
            if details.len() >= 8 && details[0] == "名詞" && details[1] == "固有名詞" {
                let reading = details[7];
                if !reading.is_empty() && reading != "*" && reading.chars().all(is_kana) {
                    output.push_str(reading);
                    replaced = true;
                }
            }
        }
        if !replaced {
            output.push_str(token.surface.as_ref());
        }
    }
    output
}

fn is_kana(c: char) -> bool {
    matches!(
        c,
        '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
        | '\u{31F0}'..='\u{31FF}' // Katakana phonetic extensions
    )
}

fn init_tokenizer() -> Option<Tokenizer> {
    let dictionary = match load_dictionary("embedded://ipadic") {
        Ok(d) => d,
        Err(err) => {
            tracing::warn!("failed to load embedded IPADIC dictionary: {err}");
            return None;
        }
    };
    let segmenter = Segmenter::new(Mode::Normal, dictionary, None).keep_whitespace(true);
    Some(Tokenizer::new(segmenter))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_date_omits_year() {
        assert_eq!(normalize_ja("2025-07-08"), "7月8日");
        assert_eq!(normalize_ja("2025/12/25"), "12月25日");
    }

    #[test]
    fn iso_datetime_with_t_separator() {
        assert_eq!(normalize_ja("2025-07-08T12:30:00"), "7月8日12時30分00秒");
    }

    #[test]
    fn iso_date_does_not_match_partial_numbers() {
        assert_eq!(normalize_ja("12025-07-08"), "12025-07-08");
        assert_eq!(normalize_ja("2025-07-080"), "2025-07-080");
    }

    #[test]
    fn slash_date_with_weekday() {
        assert_eq!(normalize_ja("7/8(火)"), "7月8日火曜日");
        assert_eq!(normalize_ja("7 / 8 （ 水 ）"), "7月8日水曜日");
    }

    #[test]
    fn invalid_slash_date_left_unchanged() {
        assert_eq!(normalize_ja("2/30(火)"), "2/30(火)");
    }

    #[test]
    fn slash_date_allows_leap_day_without_year() {
        assert_eq!(normalize_ja("2/29(火)"), "2月29日火曜日");
    }

    #[test]
    fn ja_date_with_weekday() {
        assert_eq!(normalize_ja("7月8日(火)"), "7月8日火曜日");
        assert_eq!(normalize_ja("7月8日（ 木 ）"), "7月8日木曜日");
    }

    #[test]
    fn invalid_ja_date_left_unchanged() {
        assert_eq!(normalize_ja("2月30日(火)"), "2月30日(火)");
    }

    #[test]
    fn ja_date_allows_leap_day_without_year() {
        assert_eq!(normalize_ja("2月29日(火)"), "2月29日火曜日");
    }

    #[test]
    fn fullwidth_digit_boundary() {
        // １ is a full-width digit; the half-width time after it must not be
        // matched as part of a longer number.
        assert_eq!(normalize_ja("１12:30"), "１12:30");
    }

    #[test]
    fn time_conversion() {
        assert_eq!(normalize_ja("12:30"), "12時30分");
        assert_eq!(normalize_ja("09:05:45"), "9時5分45秒");
    }

    #[test]
    fn time_does_not_match_partial_numbers() {
        assert_eq!(normalize_ja("123:30"), "123:30");
        assert_eq!(normalize_ja("12:300"), "12:300");
    }

    #[test]
    fn proper_noun_replacement() {
        let normalized = normalize_ja("名古屋の予定");
        assert!(normalized.contains("ナゴヤ"), "got {normalized}");
    }

    #[test]
    fn invalid_date_left_unchanged() {
        assert_eq!(normalize_ja("2025-02-30"), "2025-02-30");
    }

    #[test]
    fn locale_tag_ja_jp() {
        assert_eq!(normalize_for_tts("2025-07-08", "ja-JP"), "7月8日");
    }

    #[test]
    fn non_ja_text_is_borrowed_unchanged() {
        assert_eq!(normalize_for_tts("hello world", "en"), "hello world");
    }
}
