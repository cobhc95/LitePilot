# Changelog

Earlier versions are described on the [GitHub releases page](https://github.com/cobhc95/LitePilot/releases).

## 0.1.45 to 0.1.59

### 0.1.59: scrolling a long chat no longer stalls the window
- **Cause:** two painters that run on every chat paint were slow on long chats. The Copy-link painter and the bubble-corner painter asked the text control (RichEdit) where every reply and bubble marker was, including ones far off screen, and each position query makes RichEdit walk the text in between. One paint could block the UI thread for up to 2 s, wheel input piled up behind it, and Windows showed its busy pointer (arrow with a ring) while you scrolled.
- **Fix:** reply and bubble markers are read once per text change and kept; only markers in the visible range are positioned; the cache key no longer reads the whole text; no scroll path repaints synchronously any more (RichEdit's own scroll-bar path included). Wheel notches share one paint per frame.
- **Measured** (400-message chat with tables and code, 1920x1160 window, 150% DPI, 80 wheel notches posted 8 ms apart, timed inside the app):

| | Before (0.1.58) | After (0.1.59) |
|---|---|---|
| Paint frame | 193 to 1,990 ms | median 5.8 to 6.1 ms, p90 7.4 ms, max 8.8 ms |
| Paints for 80 notches | 1 to 5 | 79 to 80 |
| Copy-link painter | 186 to 1,968 ms | 0.2 ms |
| Bubble painter | up to 243 ms | 1.8 ms |
| Message queue left after the last notch | 139 to 1,309 ms to drain | 0 to 2 ms |
| Scroll handler (move, then queue a repaint) | 0.2 ms | 0.2 ms |

- Not verified: chats with pictures, replies streaming in live, a real wheel or touchpad (the test posted wheel messages). `LITEPILOT_PERF=<file>` writes per-frame timings; it does nothing unless set.
- Tests: 110 pass.

### 0.1.58 and 0.1.57: scroll bar and pointer
- The scroll-bar thumb jittered because setting the scroll position made RichEdit redraw the moved text straight to the screen, over the thumb. The position is now set with drawing held, the chat height is measured once per layout, and one function derives the thumb from the same numbers the scrolling uses.
- The chat uses its own copy of the arrow on the scroll strip and during thumb drags, which Windows does not swap for its "app starting" pointer. The helper programs started for Claude and ChatGPT chats also signal readiness at startup.
- Zoom (Ctrl + wheel or keys) lays the chat out once after the zoom settles instead of on every step, which also removed a stall on each step (0.1.55).

### 0.1.54: Copy links, typing flicker, resize
- A small underlined **Copy** link sits under every reply (after the speed line on the newest). Drawn over the chat, never inserted, so no selection or copy of the chat can include it.
- The typing box no longer sets the text direction on every key press, which made it lay out and repaint (the flicker).
- Scroll steps mark the chat for repainting and share one paint instead of each forcing one.
- After maximise, restore, Snap or any resize, the chat is laid out again once the size settles, so text, tables and code cards fit the new width.

### 0.1.52 and 0.1.50: typing box
- With one line it stays a single row. Once the text wraps, the text takes the full width and the model chip, menu and Send move to their own row beneath it, with a hairline between. It grows up to half the window's height and scrolls inside after that (0.1.50 sized it from the real line height; it was capped at six lines of a fixed height, which buried text).

### 0.1.51: Copy conversation
- The quick menu's Copy tile is now **Copy conversation** (two lines) and copies the whole chat with each turn labelled `User:` or `AI:`. The "copy last reply" key still copies only the last reply.

### 0.1.48: Esc and stopped questions
- Esc only hides the window; a reply keeps being written in the background (the send button or the Stop key still stops it).
- A question that was stopped before it was answered is now sent along with your next message. Before, it was dropped, so the model saw only "?". Questions that failed with an error are still dropped.

### 0.1.47: arrow keys scroll the chat
- Up and Down scroll the chat when the typing box is empty or has one line (Shift, Ctrl and Alt keep their meaning). Setting "Up and Down scroll the chat", on by default, in Settings > Startup.

### 0.1.46: spare engine
- Claude and ChatGPT keep a second, empty process ready with web search on, so a message that needs search no longer waits 0.5 to 2 s for a restart.

### 0.1.45: Gemini plan usage
- The model menu shows Gemini's 5-hour and weekly limits, read from `agy -p /usage` (refreshed when older than 60 s).

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
