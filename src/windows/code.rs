//! Text behind the Copy links on code blocks and tables. RichEdit reports a clicked link's address,
//! so each Copy link's placeholder address carries its block number; a card needs no window of its own.
use std::cell::RefCell;

const PREFIX: &str = "https://litepilot.invalid/copy/";

thread_local! { static BLOCKS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) }; }

/// Stores the text and returns the link address for it.
pub fn register(text: String) -> String {
    BLOCKS.with_borrow_mut(|b| {
        b.push(text);
        format!("{PREFIX}{}", b.len() - 1)
    })
}

pub fn clear() {
    BLOCKS.with_borrow_mut(|b| *b = Vec::new());
}

pub fn len() -> usize {
    BLOCKS.with_borrow(|b| b.len())
}

/// Drops blocks added since `n`: a streaming reply re-registers its blocks on every redraw.
pub fn truncate(n: usize) {
    BLOCKS.with_borrow_mut(|b| b.truncate(n));
}

/// True for any Copy link, whether or not its block still exists. These are never opened.
pub fn is_copy_link(url: &str) -> bool {
    url.contains(PREFIX)
}

/// The text for a clicked link address.
pub fn lookup(url: &str) -> Option<String> {
    let id: usize = url.split(PREFIX).nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    BLOCKS.with_borrow(|b| b.get(id).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn addresses_map_back_to_their_text() {
        clear();
        let (a, b) = (register("one".into()), register("two".into()));
        assert_eq!(lookup(&a).as_deref(), Some("one"));
        assert_eq!(lookup(&b).as_deref(), Some("two"));
        assert_eq!(lookup("https://example.com"), None);
        truncate(1);
        assert_eq!(lookup(&b), None);
        assert!(is_copy_link(&b));
        clear();
    }
}
