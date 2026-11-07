//! AI tidy for the saved facts: the assistant only PROPOSES merges of repeated facts, and every proposal is
//! checked here before it can touch the list. A merge is refused unless the new wording keeps every number
//! and name from each fact it replaces and still says what each of them said. Facts that aren't part of a
//! merge are never touched, so nothing new is ever dropped. Platform-neutral: the front end makes the model call.
use super::memory;
use serde_json::Value;

/// Facts sent to the model per call, so a long list stays within what a model reads well.
pub const CHUNK: usize = 100;

#[derive(Clone, Debug, PartialEq)]
pub struct Merge { pub from: Vec<String>, pub to: String }

pub const SYSTEM: &str = "You tidy a list of short facts a user saved about themselves. Find facts that repeat each other \
(the same thing said twice, or one fact that is just a vaguer version of another) and propose merging each such group \
into ONE fact. Rules: never invent anything; keep every number, name, date and place from every fact in the group; \
do not merge facts that are merely related; leave every other fact alone; when unsure, do not merge. Reply with JSON only, \
no other text: {\"merges\":[{\"ids\":[3,7],\"text\":\"the merged fact\"}]}. Use the numbers the facts are listed with. \
Reply {\"merges\":[]} if nothing repeats.";

/// The user message for one chunk: the facts, numbered from 1.
pub fn prompt(facts: &[String]) -> String {
    let mut s = String::from("Facts:\n");
    for (i, f) in facts.iter().enumerate() { s += &format!("{}. {}\n", i + 1, f.replace('\n', " ")); }
    s
}

/// Splits a long list into chunks; each is tidied on its own.
pub fn chunks(list: &[String]) -> Vec<Vec<String>> { list.chunks(CHUNK).map(|c| c.to_vec()).collect() }

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '.' || c == '\'' || c == '-' || c == ',')).map(|t| t.trim_matches(|c: char| ".,'-".contains(c)).to_string()).filter(|t| !t.is_empty()).collect()
}

/// Numbers and names (capitalised words that don't start the fact): the details a merge must not lose.
fn details(s: &str) -> Vec<String> {
    tokens(s).into_iter().enumerate()
        .filter(|(i, t)| t.chars().any(|c| c.is_ascii_digit()) || (*i > 0 && t.chars().count() >= 2 && t.chars().next().is_some_and(|c| c.is_uppercase())))
        .map(|(_, t)| t.to_lowercase()).collect()
}

/// True when `merged` can safely replace every fact in `from`.
pub fn safe(from: &[&str], merged: &str) -> bool {
    let merged = merged.trim();
    if merged.is_empty() || merged.chars().count() > 400 || merged.contains('\n') || memory::looks_secret(merged) { return false; }
    let m_tokens: Vec<String> = tokens(merged).into_iter().map(|t| t.to_lowercase()).collect();
    let m_stems = memory::stems(merged);
    from.iter().all(|f| {
        if details(f).iter().any(|d| !m_tokens.contains(d)) { return false; }
        // It must still say what this fact said: at least half of its words are in the new wording.
        let s = memory::stems(f);
        s.is_empty() || s.iter().filter(|w| m_stems.iter().any(|x| memory::near(x, w))).count() * 2 >= s.len()
    })
}

/// Reads the model's reply for one chunk and keeps only the merges that pass the checks. Anything odd is ignored.
pub fn proposals(reply: &str, facts: &[String]) -> Vec<Merge> {
    let (Some(a), Some(b)) = (reply.find('{'), reply.rfind('}')) else { return vec![] };
    let Ok(v) = serde_json::from_str::<Value>(&reply[a..=b]) else { return vec![] };
    let mut used = vec![false; facts.len()];
    let mut out = vec![];
    for m in v["merges"].as_array().into_iter().flatten() {
        let mut ids: Vec<usize> = m["ids"].as_array().into_iter().flatten().filter_map(|i| i.as_u64()).map(|i| i as usize).collect();
        ids.sort_unstable();
        ids.dedup();
        let Some(text) = m["text"].as_str() else { continue };
        if ids.len() < 2 || ids.iter().any(|&i| i == 0 || i > facts.len() || used[i - 1]) { continue; }
        let from: Vec<&str> = ids.iter().map(|&i| facts[i - 1].as_str()).collect();
        if !safe(&from, text) { continue; }
        for &i in &ids { used[i - 1] = true; }
        out.push(Merge { from: from.iter().map(|s| s.to_string()).collect(), to: text.trim().to_string() });
    }
    out
}

/// Applies merges to the current list, matching facts by their text (so a list that changed since the proposal
/// is handled safely: a merge whose facts aren't all still there is skipped). The merged fact takes the place of
/// the last fact it replaces, so it stays as recent as the newest of them. Returns how many merges were applied.
pub fn apply(list: &mut Vec<String>, merges: &[Merge]) -> usize {
    let mut done = 0;
    for m in merges {
        let pos: Vec<Option<usize>> = m.from.iter().map(|f| list.iter().position(|x| x == f)).collect();
        if pos.iter().any(|p| p.is_none()) { continue; }
        let mut idx: Vec<usize> = pos.into_iter().flatten().collect();
        idx.sort_unstable();
        idx.dedup();
        if idx.len() < 2 { continue; }
        let last = *idx.last().unwrap();
        list[last] = m.to.clone();
        for &i in idx[..idx.len() - 1].iter().rev() { list.remove(i); }
        done += 1;
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    fn l(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn merges_true_repeats() {
        let facts = l(&["Likes tea", "Enjoys tea a lot", "Lives in Colchester"]);
        let m = proposals(r#"{"merges":[{"ids":[1,2],"text":"Likes tea a lot"}]}"#, &facts);
        assert_eq!(m.len(), 1);
        let mut list = facts.clone();
        assert_eq!(apply(&mut list, &m), 1);
        assert_eq!(list, l(&["Likes tea a lot", "Lives in Colchester"]));
    }
    #[test]
    fn refuses_a_merge_that_loses_details() {
        let facts = l(&["Works at Acme since 2019", "Works at Acme"]);
        assert!(proposals(r#"{"merges":[{"ids":[1,2],"text":"Works at Acme"}]}"#, &facts).is_empty()); // lost 2019
        assert_eq!(proposals(r#"{"merges":[{"ids":[1,2],"text":"Works at Acme since 2019"}]}"#, &facts).len(), 1);
        let unrelated = l(&["Likes tea", "Has a dog called Rex"]);
        assert!(proposals(r#"{"merges":[{"ids":[1,2],"text":"Likes tea"}]}"#, &unrelated).is_empty());
    }
    #[test]
    fn ignores_bad_replies_and_overlaps() {
        let facts = l(&["Likes tea", "Likes tea", "Likes tea"]);
        assert!(proposals("sorry, no", &facts).is_empty());
        assert!(proposals(r#"{"merges":[{"ids":[1,9],"text":"Likes tea"}]}"#, &facts).is_empty());
        let m = proposals(r#"Here: {"merges":[{"ids":[1,2],"text":"Likes tea"},{"ids":[2,3],"text":"Likes tea"}]}"#, &facts);
        assert_eq!(m.len(), 1); // fact 2 can only be used once
        assert!(proposals(r#"{"merges":[{"ids":[1,2],"text":"Likes tea, password hunter2"}]}"#, &facts).is_empty());
    }
    #[test]
    fn apply_skips_stale_merges_and_keeps_everything_else() {
        let m = vec![Merge { from: l(&["a one", "a two"]), to: "a".into() }];
        let mut list = l(&["x", "a one", "y"]); // "a two" was deleted meanwhile
        assert_eq!(apply(&mut list, &m), 0);
        assert_eq!(list, l(&["x", "a one", "y"]));
    }
}
