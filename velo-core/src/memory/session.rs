use std::collections::VecDeque;

#[derive(Debug, Clone)]
struct SessionEntry {
    role: String,
    content: String,
    token_estimate: usize,
}

#[derive(Debug, Clone)]
pub struct SessionBuffer {
    entries: VecDeque<SessionEntry>,
    max_tokens: usize,
    total_tokens: usize,
}

impl SessionBuffer {
    pub fn new(max_tokens: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(128),
            max_tokens,
            total_tokens: 0,
        }
    }

    pub fn push(&mut self, role: String, content: String, token_estimate: usize) {
        self.total_tokens += token_estimate;
        self.entries.push_back(SessionEntry {
            role,
            content,
            token_estimate,
        });

        while self.total_tokens > self.max_tokens {
            if let Some(entry) = self.entries.pop_front() {
                self.total_tokens = self.total_tokens.saturating_sub(entry.token_estimate);
            } else {
                break;
            }
        }
    }

    pub fn token_usage(&self) -> usize {
        self.total_tokens
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn compress_oldest_ratio(&mut self, ratio: f64) -> Option<(String, Vec<(String, String)>)> {
        let compress_count = (self.entries.len() as f64 * ratio).ceil() as usize;
        if compress_count < 2 {
            return None;
        }

        let mut removed: Vec<(String, String)> = Vec::with_capacity(compress_count);
        let mut removed_tokens = 0usize;

        for _ in 0..compress_count {
            if let Some(entry) = self.entries.pop_front() {
                removed_tokens += entry.token_estimate;
                removed.push((entry.role, entry.content));
            } else {
                break;
            }
        }

        if removed.is_empty() {
            return None;
        }

        self.total_tokens = self.total_tokens.saturating_sub(removed_tokens);

        let summary = format!(
            "[Compressed summary of {} prior messages: {}]",
            removed.len(),
            removed
                .iter()
                .map(|(r, c)| format!("{r}: {c}"))
                .collect::<Vec<_>>()
                .join(" | ")
        );

        let summary_tokens = summary.len() / 4;
        self.total_tokens += summary_tokens;
        self.entries.push_front(SessionEntry {
            role: "system".into(),
            content: summary.clone(),
            token_estimate: summary_tokens,
        });

        Some((summary, removed))
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_tokens = 0;
    }

    pub fn context_summary(&self) -> SessionContext {
        let total_entries = self.entries.len();
        let roles: Vec<String> = self.entries.iter().map(|e| e.role.clone()).collect();
        let recent: Vec<String> = self
            .entries
            .iter()
            .rev()
            .take(5)
            .rev()
            .map(|e| format!("{}: {}", e.role, truncate(&e.content, 120)))
            .collect();

        SessionContext {
            total_entries,
            total_tokens: self.total_tokens,
            max_tokens: self.max_tokens,
            roles,
            recent_messages: recent,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionContext {
    pub total_entries: usize,
    pub total_tokens: usize,
    pub max_tokens: usize,
    pub roles: Vec<String>,
    pub recent_messages: Vec<String>,
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    let boundary = s
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i <= max_len)
        .last()
        .unwrap_or(0);
    format!("{}...", &s[..boundary])
}
