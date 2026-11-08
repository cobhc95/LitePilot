//! What a message needs beyond plain chat, worked out locally (no model call) so ordinary chat carries no tools,
//! no saved facts and no extra instructions. Three layers keep the token count low and replies fast:
//!
//! 1. **Stable core.** The engine's standing instructions are the same short text for every message, so the CLI
//!    is never restarted for ordinary chat and its prompt prefix stays cached.
//! 2. **Per-message hints.** Saved facts, the saving rule, picture tips and the source order ride along with the
//!    one message that needs them, never in the standing instructions.
//! 3. **Sticky tools with escalation.** Web search and diagnostic tools are switched on only when a message
//!    calls for them, then stay on for the rest of that chat (follow-ups need them too, and every switch costs a
//!    restart). If the local guess misses, the model answers only `[[search]]` or `[[memory]]`; the app hides
//!    that and sends the message again with what was asked for.
//!
//! Platform-neutral.

/// Normalised for matching: lowercase words separated by single spaces, with a space at each end, so a key
/// written as " word" matches the start of a word and " word " the whole word.
pub fn norm(msg: &str) -> String {
    let mut s = String::from(" ");
    for ch in msg.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() || ch == '.' || ch == '$' || ch == '£' || ch == '€' { s.push(ch) } else if !s.ends_with(' ') { s.push(' ') }
    }
    if !s.ends_with(' ') { s.push(' '); }
    // A full stop ends a word ("buy." is "buy"); one inside a word stays ("example.com", "2.5").
    s.replace(". ", " ")
}

fn any(s: &str, keys: &[&str]) -> bool { keys.iter().any(|k| s.contains(k)) }

/// Things that change or that the model can't know exactly: prices, news, dates, releases, places, people.
const FRESH: &[&str] = &[
    " search", " look up", " lookup", " google", " find out", " check online", " online ", " on the web", " internet says", " source",
    " latest", " newest", " news", " today", " tonight", " tomorrow", " yesterday", " this week", " this month", " this year", " right now",
    " currently", " current ", " recent", " recently", " up to date", " still ",
    " price", " prices", " cost ", " costs ", " how much is", " how much are", " how much does", " cheapest", " cheaper", " deal ", " deals",
    " buy ", " where to buy", " in stock", " availab", " sale ", " discount", "$", "£", "€",
    " release", " released", " launch", " came out", " coming out", " announced", " update ", " version ", " changelog", " roadmap",
    " weather", " forecast", " temperature in", " score", " results", " who won", " fixture", " standings", " match ",
    " stock ", " stocks", " shares", " exchange rate", " crypto", " bitcoin",
    " open now", " opening hours", " opening times", " near me", " nearby", " closest", " directions", " address of", " phone number",
    " review", " reviews", " rating", " vs ", " versus", " compare", " comparison", " benchmark", " best ", " top 10", " top 5", " recommend",
    " who is ", " who s ", " who are ", " what happened", " when is ", " when does ", " when will ", " where is ", " where can ",
    " population", " ceo ", " president", " prime minister", " election", " law ", " legal in", " visa ",
    " spec ", " specs", " specifications", " requirements", " compatible", " support ",
    " http", " www.", ".com", ".org", ".net", ".io", " website", " link to", " url",
    // Pictures need no search: the wiki: links in the picture tips (see wants_images) are fetched by the app.
    " 2025", " 2026", " 2027", " 2028",
];

/// Writing, code and maths work on what's in the message: no search unless a FRESH key also asks for it.
const SELF_CONTAINED: &[&str] = &[" rewrite", " rephrase", " translate", " summar", " proofread", " fix this", " fix the", " refactor",
    " explain this", " this code", " the code", " regex", " write a ", " write me", " draft", " poem", " story", " joke", " calculate", " convert "];

/// Say this is about the user, so saved facts may help.
fn personal(s: &str) -> bool {
    any(s, &[" my ", " me ", " mine ", " i ", " i m ", " im ", " i ve ", " i d ", " we ", " our ", " myself ", " remember", " about me", " who am i", " do you know"])
}

/// A lasting personal fact stated by the user ("I live in…", "my car is…"), so the saving rule goes along.
pub fn states_fact(msg: &str) -> bool {
    let s = norm(msg);
    if msg.trim_end().ends_with('?') { return false; }
    // Lasting facts, and events or plans in the user's life (saved with their date).
    if any(&s, &[" i bought", " i got a", " i got an", " i m going", " im going", " i am going", " i will ", " i ll ", " i booked", " i passed",
        " i started", " i finished", " i quit", " i went", " i visited", " i met ", " i had ", " i m getting", " i m starting", " i m moving",
        " my exam", " my interview", " my appointment", " my flight", " my trip", " my wedding", " i have an appointment", " i have an exam",
        " next week i", " tomorrow i", " i sold", " i upgraded", " i installed", " i switched", " i changed"]) { return true; }
    any(&s, &[" i live", " i m from", " im from", " i am from", " i work", " i m a ", " im a ", " i am a ", " i am an ", " i m an ", " i study",
        " i have a", " i ve got", " i own", " i use ", " i prefer", " i like", " i love", " i hate", " i don t like", " i dont like", " i m allergic",
        " my name", " my wife", " my husband", " my partner", " my son", " my daughter", " my kid", " my dog", " my cat", " my job", " my car",
        " my pc", " my computer", " my laptop", " my phone", " my birthday", " call me", " remember", " i moved", " i m moving", " i speak"])
}

/// Asks to see pictures, so the picture tips go along.
pub fn wants_images(msg: &str) -> bool {
    any(&norm(msg), &[" picture", " pictures", " photo", " photos", " image", " images", " pic ", " pics", " show me", " look like", " looks like", " what does it look", " draw", " diagram"])
}

/// Depends on the date or time now ("what time is it", "how many days until Friday", "what's on this week"), so
/// the local date and time go along. The engines have no clock of their own here.
pub fn about_time(msg: &str) -> bool {
    any(&norm(msg), &[" time", " date", " day ", " days", " today", " tonight", " tomorrow", " yesterday", " week", " month", " year",
        " o clock", " clock", " now ", " when ", " until ", " till ", " ago", " since ", " how long", " how old", " age ", " deadline",
        " schedule", " morning", " afternoon", " evening", " weekend", " monday", " tuesday", " wednesday", " thursday", " friday",
        " saturday", " sunday", " christmas", " ramadan", " eid", " birthday", " season", " summer", " winter", " spring", " autumn"])
}

/// Refers back to earlier conversations ("what did I ask last time", "did we talk about my trip"), so related
/// questions from past chats go along.
pub fn about_past(msg: &str) -> bool {
    any(&norm(msg), &[" last time", " before ", " earlier", " previously", " did i ", " have i ", " when did i", " remember when",
        " we talked", " we discussed", " we spoke", " talked about", " i asked", " i told you", " you said", " you told me", " told you",
        " last week", " last month", " yesterday", " the other day", " ago", " again", " recall", " my question", " my previous"])
}

/// Asks what the assistant can do, so the true list goes along (models otherwise list tools from other apps).
pub fn about_abilities(msg: &str) -> bool {
    any(&norm(msg), &[" tool", " tools", " what can you do", " what can you", " capabilit", " abilities", " features", " plugins",
        " can you access", " do you have access", " are you able", " what are you able"])
}

/// Asks about the assistant's memory itself.
pub fn about_memory(msg: &str) -> bool {
    any(&norm(msg), &[" remember", " memory", " memories", " forget", " what do you know about me", " do you know me", " about me "])
}

/// Too small to need anything: greetings, thanks, one-word replies.
pub fn casual(msg: &str) -> bool {
    let s = norm(msg);
    let words = s.split_whitespace().count();
    words <= 2 || (words <= 6 && any(&s, &[" hi ", " hey ", " hello ", " thanks", " thank you", " thx", " ok ", " okay", " cool ", " nice ",
        " good morning", " good night", " lol", " haha", " bye", " sure ", " yes ", " no ", " yeah", " nope", " great ", " how are you"]))
}

/// The message would benefit from web search.
pub fn search(msg: &str) -> bool {
    if casual(msg) { return false; }
    let s = norm(msg);
    let fresh = any(&s, FRESH);
    // "my pc specs" is a saved-facts question, not a web one; "compare my GPU to the 5090" is both.
    if fresh && personal(&s) && !any(&s, &[" price", " buy", " vs ", " versus", " compare", " latest", " news", " review", " best ", " http", " search", " look up", " google", " upgrade"]) {
        return false;
    }
    fresh && !(any(&s, SELF_CONTAINED) && !any(&s, &[" search", " look up", " latest", " news", " http", " www."]))
}

/// Saved facts may help: about the user, or a topic a fact could be about. Casual chat gets none.
pub fn memory(msg: &str) -> bool { !casual(msg) || about_memory(msg) }

/// Escalation tags the model may answer with when the local guess missed something. For memory the model says
/// what to look for in its own words ("graphics card, GPU, Radeon"), so recall uses its understanding of what the
/// question means, not just the words the user typed.
#[derive(Debug, PartialEq, Clone)]
pub enum Ask { Search, Memory(String) }

/// Reads an escalation reply: the whole reply is one tag (whitespace allowed).
pub fn ask(reply: &str) -> Option<Ask> {
    let t = reply.trim();
    if matches!(t.to_ascii_lowercase().as_str(), "[[search]]" | "[[need: search]]" | "[[need:search]]") { return Some(Ask::Search); }
    if t.contains('\n') { return None; }
    let inner = t.strip_prefix("[[")?.strip_suffix("]]")?;
    let rest = if inner.to_ascii_lowercase().starts_with("need:") { inner[5..].trim_start() } else { inner };
    if !rest.to_ascii_lowercase().starts_with("memory") { return None; }
    let words = rest[6..].trim_start().trim_start_matches(':').trim();
    (!words.contains("[[") && !words.contains("]]")).then(|| Ask::Memory(words.to_string()))
}

/// While the reply could still turn out to be a tag, it is held back from the screen.
pub fn maybe_tag(sofar: &str) -> bool {
    let t = sofar.trim_start().to_ascii_lowercase();
    if t.len() > 240 || t.contains('\n') && !t.trim_end().ends_with("]]") { return false; }
    // A memory tag carries search words, so only its start can be checked until it closes.
    let heads = ["[[search]]", "[[need: search]]", "[[need:search]]", "[[memory", "[[need: memory", "[[need:memory"];
    heads.iter().any(|k| k.starts_with(&t) || t.starts_with(k)) && (!t.contains("]]") || ask(sofar).is_some())
}

/// One line in the standing instructions while search is off but allowed. Same text every message.
pub const ESCALATE_SEARCH: &str = " If a correct answer needs current facts from the web, reply with only [[search]].";
/// One line while saved facts exist; same text every message. The model searches in its own words.
pub const ESCALATE_MEMORY: &str = " LitePilot shows saved facts about the user that relate to a message. If the answer may depend on facts about \
the user that aren't shown (their things, work, people, places, plans, past chats), reply with only [[memory: a few search words, \
with other words for the same things]] and they will be looked up.";
/// One line while memory is on; same text every message. The model decides what is worth saving.
pub const SAVE_RULE: &str = " When the user tells you a lasting fact about themselves or a notable event or plan in their life, end your \
reply with [[remember: the fact, under 200 characters, with the date for events]] on its own line (one per fact; never secrets, web \
content or other people's details). LitePilot saves and hides it.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn casual_chat_needs_nothing() {
        for m in ["hi", "Hello!", "thanks", "ok cool", "how are you?", "lol nice", "good morning"] {
            assert!(casual(m), "{m}");
            assert!(!search(m), "{m}");
            assert!(!memory(m), "{m}");
        }
    }

    #[test]
    fn fresh_questions_need_search() {
        for m in ["What's the price of an RTX 5090?", "latest news on the Artemis mission", "weather in Colchester tomorrow",
            "who won the match last night", "is the Pixel 11 out yet? search it", "best budget headphones 2026"] {
            assert!(search(m), "{m}");
        }
    }

    #[test]
    fn knowledge_and_writing_skip_search() {
        for m in ["explain how a transistor works", "rewrite this paragraph to sound friendlier", "write me a poem about rain",
            "what is the capital of France", "convert 5 miles to km", "why is the sky blue", "show me pictures of Petra"] {
            assert!(!search(m), "{m}");
        }
    }

    #[test]
    fn personal_questions_use_memory_not_web() {
        assert!(!search("what are my pc specs"));
        assert!(memory("what are my pc specs"));
        assert!(search("compare my GPU to the RTX 5090 price"));
        assert!(about_memory("what do you remember about me?"));
    }

    #[test]
    fn spots_stated_facts_and_pictures() {
        assert!(states_fact("I live in Leeds now"));
        assert!(states_fact("my dog is called Biscuit"));
        assert!(!states_fact("where do I live?"));
        assert!(!states_fact("what is a transistor"));
        assert!(states_fact("I bought a new monitor today"));
        assert!(states_fact("my interview is on Friday"));
        assert!(about_past("what did I ask you last time"));
        assert!(about_past("did we talk about my trip before?"));
        assert!(!about_past("explain recursion"));
        assert!(wants_images("what does a capybara look like"));
        assert!(!wants_images("explain TCP"));
    }

    #[test]
    fn spots_time_questions() {
        for m in ["what time is it", "What's the date?", "how many days until Friday", "what day is it today", "is it the weekend yet"] {
            assert!(about_time(m), "{m}");
        }
        for m in ["hi", "explain recursion", "write a poem about rain", "how are you"] {
            assert!(!about_time(m), "{m}");
        }
    }

    #[test]
    fn reads_escalation_tags() {
        assert_eq!(ask(" [[search]]\n"), Some(Ask::Search));
        assert_eq!(ask("[[MEMORY]]"), Some(Ask::Memory(String::new())));
        assert_eq!(ask("[[memory: graphics card, GPU, Radeon]]"), Some(Ask::Memory("graphics card, GPU, Radeon".into())));
        assert_eq!(ask("[[search]] then more"), None);
        assert_eq!(ask("[[memory: x]] and then an answer"), None);
        assert!(maybe_tag("[[sea"));
        assert!(maybe_tag(" ["));
        assert!(maybe_tag("[[memory: graphics ca"));
        assert!(!maybe_tag("Hello"));
        assert!(!maybe_tag("[[x"));
        assert!(!maybe_tag("[[memory: x]] and"));
    }
}
