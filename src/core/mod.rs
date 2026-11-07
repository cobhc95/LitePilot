/// The one app version: Cargo.toml's `version`. Shown in Settings, the uninstall entry and HTTP user agents.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub mod config;
pub mod attach;
pub mod backup;
pub mod history;
pub mod keys;
pub mod mcp;
pub mod memory;
pub mod protocol;
pub mod resident;
pub mod tidy;
pub mod update;

#[derive(Clone, Debug, PartialEq)]
pub struct Msg {
    pub role: String, // user | assistant | error | note (shown only, never sent)
    pub text: String,
}
impl Msg {
    pub fn new(role: &str, text: impl Into<String>) -> Self {
        Msg { role: role.into(), text: text.into() }
    }
}
