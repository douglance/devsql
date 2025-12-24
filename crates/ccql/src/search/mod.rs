use crate::error::Result;
use regex::Regex;

pub struct SearchEngine {
    pattern: Regex,
    _case_sensitive: bool,
}

impl SearchEngine {
    pub fn new(pattern: &str, case_sensitive: bool, is_regex: bool) -> Result<Self> {
        let regex_pattern = if is_regex {
            if case_sensitive {
                pattern.to_string()
            } else {
                format!("(?i){}", pattern)
            }
        } else {
            let escaped = regex::escape(pattern);
            if case_sensitive {
                escaped
            } else {
                format!("(?i){}", escaped)
            }
        };

        let regex = Regex::new(&regex_pattern)?;
        Ok(Self {
            pattern: regex,
            _case_sensitive: case_sensitive,
        })
    }

    pub fn matches(&self, text: &str) -> bool {
        self.pattern.is_match(text)
    }

    pub fn find_in_json(&self, value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::String(s) => self.matches(s),
            serde_json::Value::Array(arr) => arr.iter().any(|v| self.find_in_json(v)),
            serde_json::Value::Object(obj) => obj.values().any(|v| self.find_in_json(v)),
            _ => {
                let s = value.to_string();
                self.matches(&s)
            }
        }
    }

    pub fn highlight(&self, text: &str) -> String {
        use colored::Colorize;
        self.pattern
            .replace_all(text, |caps: &regex::Captures| {
                caps[0].red().bold().to_string()
            })
            .to_string()
    }
}

#[derive(Debug, Clone)]
pub struct SearchMatch {
    pub source: String,
    pub line_number: Option<usize>,
    pub content: String,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

impl SearchMatch {
    pub fn new(source: String, content: String) -> Self {
        Self {
            source,
            line_number: None,
            content,
            context_before: vec![],
            context_after: vec![],
        }
    }

    pub fn with_line(mut self, line: usize) -> Self {
        self.line_number = Some(line);
        self
    }

    pub fn with_context(mut self, before: Vec<String>, after: Vec<String>) -> Self {
        self.context_before = before;
        self.context_after = after;
        self
    }
}
