//! System prompt for the cleanup model (spec s5, spike-tested copy).
use crate::settings::Language;

fn script_rule(lang: Language) -> &'static str {
    match lang {
        Language::Auto | Language::Hinglish => {
            "Write any Hindi in Roman script as natural Hinglish (e.g. \"main kal aaunga\"), never Devanagari."
        }
        Language::Hindi => "Write Hindi in Devanagari script. Keep English words in Latin script.",
        Language::English => "The speaker is speaking English. Fix obvious mis-hearings of English words.",
    }
}

pub fn system_prompt(lang: Language, hints: &[(String, Vec<String>)]) -> String {
    let mut p = String::from("You clean up raw speech-to-text transcripts for a bilingual Hindi/English speaker.\nRules:\n");
    p += &format!("- {}\n", script_rule(lang));
    p += "- Keep English words as English. Do not translate between languages.\n";
    p += "- Remove filler words (um, uh, like, basically when filler). Apply self-corrections (\"5, no wait, 6\" -> \"6\").\n";
    p += "- Fix punctuation and capitalization.\n";
    if !hints.is_empty() {
        let list: Vec<String> = hints
            .iter()
            .map(|(w, heard)| if heard.is_empty() { w.clone() } else { format!("{w} (heard as {})", heard.join("/")) })
            .collect();
        p += &format!(
            "- These dictionary words must be spelled exactly like this when they are what was meant: {}.\n",
            list.join(", ")
        );
    }
    p += "- Output only the cleaned text. No quotes, no explanations.";
    p
}
