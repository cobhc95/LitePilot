# Changelog

Earlier versions are described on the [GitHub releases page](https://github.com/cobhc95/LitePilot/releases).

## 0.1.21

- **Tidier code, same behavior.** The two largest files are split along their existing sections so each part can be read on its own: the popup (`app.rs`, 2,700 lines) into `app/` (RichEdit plumbing, painting, hotkeys, scrolling, input controls, window procedure) and Settings (`settings.rs`, 2,000 lines) into `settings/` (picker, pages, actions, keyboard, painting). Only code moved; nothing was rewritten.
- One copy each of the color blend, clipboard reader and wide-string helpers (there were four, two and three). Literal NUL bytes in the source replaced with `c"..."` strings. Zero clippy warnings (about 50 before).
- Includes everything in 0.1.20, which was not released on its own.

## 0.1.20

- **PC diagnostics on demand.** Eleven read-only tools (`sys`, `lang`, `disk`, `proc`, `net`, `events`, `sec`, `apps`, `file`, `reg`, `dev`), each with a `what` choice, cover about 60 checks, such as installed keyboard layouts, slow-PC causes, port owners, crashes, Defender status and git status.
- **Router.** A local keyword check (no model call) decides which tools a message needs. Ordinary chat sends no tool definitions and no tool instructions at all, which saves about 1,400 input tokens per message compared with 0.1.19, where five tools went with every message. A PC question sends only the matching tools (one tool adds about 500 tokens). Start a message with `/diag` to send all eleven.
- **Token stat.** The line under each reply now also shows the total tokens for that reply (input, cached input and output, summed over every model call including tool rounds), as reported by the provider: `6s · 154 tok/s · 1,842 tokens`. When a provider reports nothing it shows an estimate marked `~`. OpenAI-compatible API requests now ask for usage (`stream_options.include_usage`).
- **Safety model.** The free-form `run_powershell` tool and its script checker are gone. Every check runs one fixed query; the model supplies only values checked by type, range and character set, which reach PowerShell as environment variables, never as command text. Each check is read-only and unelevated (it returns `requires_elevation` instead of asking), stops after 10 s with its whole process tree, and runs one at a time, at most six per message. File and repo paths must be absolute, inside Documents, Downloads or Desktop after links and junctions are resolved, and outside a deny-list of key, credential, browser and password-manager locations. Every result is redacted (passwords, tokens, API keys, Bearer headers, connection strings, product keys, secret-named values), pruned, capped at 4 KB (8 KB for file reads and event logs) and wrapped as data.
- **Audit log.** Every tool call (time, tool, parameters, result size, duration; never the output) is appended to `diag-audit.log` in LitePilot's data folder.
