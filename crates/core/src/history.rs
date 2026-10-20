use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const HISTORY_CAP: usize = 100;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryItem {
    pub at_ms: i64,
    pub raw: String,
    pub cleaned: String,
    pub final_text: Option<String>,
    pub app: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct History {
    pub version: u32,
    pub items: VecDeque<HistoryItem>,
}

impl Default for History {
    fn default() -> Self {
        History { version: 1, items: VecDeque::new() }
    }
}

impl History {
    pub fn push(&mut self, item: HistoryItem) {
        self.items.push_back(item);
        while self.items.len() > HISTORY_CAP {
            self.items.pop_front();
        }
    }

    pub fn set_final(&mut self, at_ms: i64, text: String) {
        if let Some(i) = self.items.iter_mut().find(|i| i.at_ms == at_ms) {
            i.final_text = Some(text);
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}
