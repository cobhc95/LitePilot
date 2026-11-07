/// The release name shown in the app, the installer and the file properties.
pub const VERSION: &str = "0.1 alpha";
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
