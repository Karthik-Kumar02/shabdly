use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Manual,
    Learned,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub word: String,
    #[serde(default)]
    pub heard_as: Vec<String>,
    /// Forms used only as hints to the cleanup model (real-word guard, spec s4).
    #[serde(default)]
    pub hint_only: Vec<String>,
    #[serde(default)]
    pub replace_with: Option<String>,
    pub source: Source,
    #[serde(default)]
    pub starred: bool,
    #[serde(default)]
    pub uses: u32,
    #[serde(default)]
    pub last_used: Option<i64>,
    #[serde(default)]
    pub created: i64,
}

impl Entry {
    pub fn new(word: &str, source: Source, now: i64) -> Entry {
        Entry {
            word: word.to_string(),
            heard_as: vec![],
            hint_only: vec![],
            replace_with: None,
            source,
            starred: false,
            uses: 0,
            last_used: None,
            created: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dictionary {
    pub version: u32,
    pub entries: Vec<Entry>,
}

impl Default for Dictionary {
    fn default() -> Self {
        Dictionary { version: 1, entries: vec![] }
    }
}

/// Push `form` unless an equal form (ignoring case) is already there.
pub(crate) fn add_form(list: &mut Vec<String>, form: &str) -> bool {
    let lower = form.to_lowercase();
    if list.iter().any(|f| f.to_lowercase() == lower) {
        return false;
    }
    list.push(form.to_string());
    true
}

impl Dictionary {
    fn index(&self, word: &str) -> Option<usize> {
        let w = word.to_lowercase();
        self.entries.iter().position(|e| e.word.to_lowercase() == w)
    }

    pub fn get(&self, word: &str) -> Option<&Entry> {
        self.index(word).map(|i| &self.entries[i])
    }

    pub fn get_mut(&mut self, word: &str) -> Option<&mut Entry> {
        self.index(word).map(move |i| &mut self.entries[i])
    }

    /// Add or edit a manual entry. Editing replaces `heard_as` and `replace_with`.
    pub fn upsert_manual(&mut self, word: &str, heard_as: Vec<String>, replace_with: Option<String>, now: i64) {
        let word = word.trim();
        match self.get_mut(word) {
            Some(e) => {
                e.word = word.to_string();
                e.heard_as = heard_as;
                e.replace_with = replace_with;
            }
            None => {
                let mut e = Entry::new(word, Source::Manual, now);
                e.heard_as = heard_as;
                e.replace_with = replace_with;
                self.entries.push(e);
            }
        }
    }

    pub fn remove(&mut self, word: &str) -> bool {
        match self.index(word) {
            Some(i) => {
                self.entries.remove(i);
                true
            }
            None => false,
        }
    }

    pub fn set_starred(&mut self, word: &str, starred: bool) -> bool {
        match self.get_mut(word) {
            Some(e) => {
                e.starred = starred;
                true
            }
            None => false,
        }
    }

    pub fn record_use(&mut self, word: &str, now: i64) {
        if let Some(e) = self.get_mut(word) {
            e.uses = e.uses.saturating_add(1);
            e.last_used = Some(now);
        }
    }

    /// Layer 1 hints: starred first, then most used, then alphabetical.
    pub fn hints(&self, limit: usize) -> Vec<(String, Vec<String>)> {
        let mut list: Vec<&Entry> = self.entries.iter().filter(|e| e.replace_with.is_none()).collect();
        list.sort_by(|a, b| {
            b.starred
                .cmp(&a.starred)
                .then(b.uses.cmp(&a.uses))
                .then_with(|| a.word.to_lowercase().cmp(&b.word.to_lowercase()))
        });
        list.into_iter()
            .take(limit)
            .map(|e| (e.word.clone(), e.heard_as.iter().chain(&e.hint_only).cloned().collect()))
            .collect()
    }

    /// Merge another dictionary's entries by word. Returns how many new words were added.
    pub fn import_json(&mut self, json: &str) -> anyhow::Result<usize> {
        let other: Dictionary = serde_json::from_str(json)?;
        let mut added = 0;
        for e in other.entries {
            if e.word.trim().is_empty() {
                continue;
            }
            match self.get_mut(&e.word) {
                Some(mine) => {
                    for f in &e.heard_as {
                        add_form(&mut mine.heard_as, f);
                    }
                    for f in &e.hint_only {
                        add_form(&mut mine.hint_only, f);
                    }
                    if mine.replace_with.is_none() {
                        mine.replace_with = e.replace_with;
                    }
                }
                None => {
                    self.entries.push(e);
                    added += 1;
                }
            }
        }
        Ok(added)
    }

    pub fn export_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("dictionary serializes")
    }
}
