use crate::Event;

#[derive(Debug, Clone, Copy)]
pub(crate) struct TextLimits {
    pub max_chars: usize,
    pub max_title_chars: usize,
    pub max_body_chars: usize,
    pub max_tags: usize,
    pub max_tag_key_chars: usize,
    pub max_tag_value_chars: usize,
}

impl Default for TextLimits {
    fn default() -> Self {
        Self {
            max_chars: 16 * 1024,
            max_title_chars: 256,
            max_body_chars: 4 * 1024,
            max_tags: 32,
            max_tag_key_chars: 64,
            max_tag_value_chars: 256,
        }
    }
}

impl TextLimits {
    pub(crate) fn new(max_chars: usize) -> Self {
        Self {
            max_chars,
            ..Self::default()
        }
    }
}

struct LimitedChars {
    max: usize,
    out: Vec<char>,
    truncated: bool,
}

impl LimitedChars {
    fn new(max: usize) -> Self {
        Self {
            max,
            out: Vec::new(),
            truncated: false,
        }
    }

    fn is_empty(&self) -> bool {
        self.out.is_empty()
    }

    fn push_char(&mut self, ch: char) {
        if self.truncated || self.max == 0 {
            return;
        }
        if self.out.len() >= self.max {
            self.truncated = true;
            return;
        }
        self.out.push(ch);
    }

    fn push_str(&mut self, s: &str) {
        if self.truncated || self.max == 0 {
            return;
        }
        for ch in s.chars() {
            if self.out.len() >= self.max {
                self.truncated = true;
                break;
            }
            self.out.push(ch);
        }
    }

    fn finish(mut self) -> String {
        if self.truncated && self.max > 3 {
            self.out.truncate(self.max - 3);
            self.out.extend(['.', '.', '.']);
        }
        self.out.into_iter().collect()
    }
}

fn format_event_text_parts_limited(
    event: &Event,
    limits: TextLimits,
    include_title: bool,
) -> String {
    let mut out = LimitedChars::new(limits.max_chars);

    if include_title {
        let title = truncate_chars(&event.title, limits.max_title_chars);
        out.push_str(&title);
    }

    if let Some(body) = event.body.as_deref() {
        let body = body.trim();
        if !body.is_empty() {
            if !out.is_empty() {
                out.push_char('\n');
            }
            let body = truncate_chars(body, limits.max_body_chars);
            out.push_str(&body);
        }
    }

    for (idx, (k, v)) in event.tags.iter().enumerate() {
        if idx >= limits.max_tags {
            break;
        }
        if !out.is_empty() {
            out.push_char('\n');
        }
        let key = truncate_chars(k, limits.max_tag_key_chars);
        out.push_str(&key);
        out.push_char('=');
        let value = truncate_chars(v, limits.max_tag_value_chars);
        out.push_str(&value);
    }

    out.finish()
}

pub(crate) fn format_event_text_limited(event: &Event, limits: TextLimits) -> String {
    format_event_text_parts_limited(event, limits, true)
}

pub(crate) fn format_event_body_and_tags_limited(event: &Event, limits: TextLimits) -> String {
    format_event_text_parts_limited(event, limits, false)
}

pub(crate) fn truncate_chars(input: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let mut chars = input.chars();
    let mut out: Vec<char> = Vec::with_capacity(max_chars.min(256));
    for _ in 0..max_chars {
        let Some(ch) = chars.next() else {
            return out.into_iter().collect();
        };
        out.push(ch);
    }

    if chars.next().is_none() {
        return out.into_iter().collect();
    }

    if max_chars > 3 {
        out.truncate(max_chars - 3);
        out.extend(['.', '.', '.']);
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Severity;

    #[test]
    fn truncate_chars_is_utf8_safe() {
        let input = "a😀b";
        let out = truncate_chars(input, 3);
        assert_eq!(out, "a😀b");

        let out = truncate_chars(input, 2);
        assert_eq!(out, "a😀");

        let out = truncate_chars(input, 1);
        assert_eq!(out, "a");
    }

    #[test]
    fn truncate_chars_adds_ellipsis() {
        let input = "abcdef";
        let out = truncate_chars(input, 5);
        assert_eq!(out, "ab...");
    }

    #[test]
    fn format_event_text_limited_caps_tags_and_length() {
        let mut event = Event::new("k", Severity::Info, "title").with_body("body");
        for i in 0..100 {
            event = event.with_tag(format!("k{i}"), "v");
        }

        let limits = TextLimits {
            max_chars: 20,
            max_tags: 2,
            ..TextLimits::default()
        };

        let out = format_event_text_limited(&event, limits);
        assert!(out.chars().count() <= 20, "{out}");
        assert!(out.contains("title"), "{out}");
    }
}
