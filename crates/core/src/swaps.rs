//! Layer 2: exact, deterministic swaps applied after cleanup (spec s4).
use crate::dictionary::Dictionary;

/// Word boundary: whitespace, ASCII punctuation, or Devanagari/typographic
/// punctuation. Devanagari vowel signs are not boundaries, so "रिया" never
/// matches inside "रियासत".
pub fn is_boundary(c: char) -> bool {
    c.is_whitespace() || c.is_ascii_punctuation() || "।॥“”‘’…".contains(c)
}

/// One char in, one char out, so indices into the original and the lowered
/// text stay aligned.
fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn at_sentence_start(chars: &[char], i: usize) -> bool {
    let mut j = i;
    while j > 0 && (chars[j - 1] == ' ' || chars[j - 1] == '\t') {
        j -= 1;
    }
    j == 0 || matches!(chars[j - 1], '.' | '!' | '?' | '\n')
}

struct Pattern {
    form: Vec<char>,
    insert: String,
    word: String,
}

/// Replace every `heard_as` form (and every `replace_with` key) found as a
/// whole word or phrase, case-insensitive, longest first. Returns the new text
/// and the dictionary words used, in order of first use.
pub fn apply_swaps(text: &str, dict: &Dictionary) -> (String, Vec<String>) {
    let mut patterns = Vec::new();
    for e in &dict.entries {
        for f in &e.heard_as {
            patterns.push(Pattern { form: f.chars().map(lower).collect(), insert: e.word.clone(), word: e.word.clone() });
        }
        if let Some(r) = &e.replace_with {
            patterns.push(Pattern { form: e.word.chars().map(lower).collect(), insert: r.clone(), word: e.word.clone() });
        }
    }
    patterns.retain(|p| !p.form.is_empty());
    patterns.sort_by(|a, b| b.form.len().cmp(&a.form.len()));

    let chars: Vec<char> = text.chars().collect();
    let low: Vec<char> = chars.iter().copied().map(lower).collect();
    let mut out = String::with_capacity(text.len());
    let mut used: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let starts_word = i == 0 || is_boundary(chars[i - 1]);
        let hit = if starts_word {
            patterns.iter().find(|p| {
                let end = i + p.form.len();
                end <= chars.len() && low[i..end] == p.form[..] && (end == chars.len() || is_boundary(chars[end]))
            })
        } else {
            None
        };
        match hit {
            Some(p) => {
                let mut ins = p.insert.clone();
                if at_sentence_start(&chars, i) && ins.chars().next().is_some_and(char::is_lowercase) {
                    let mut cs = ins.chars();
                    let first = cs.next().unwrap();
                    ins = first.to_uppercase().chain(cs).collect();
                }
                out.push_str(&ins);
                if !used.contains(&p.word) {
                    used.push(p.word.clone());
                }
                i += p.form.len();
            }
            None => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    (out, used)
}
