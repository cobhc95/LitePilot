# LitePilot

A tiny Windows popup for AI chat. Press a shortcut, ask, press Esc.

_Version 0.1 alpha._

<p>
  <img src="screenshots/chat.png" alt="The LitePilot chat popup" width="380">
  <img src="screenshots/settings.png" alt="LitePilot settings" width="380">
</p>

- **Setup:** run `LitePilot-0.1-alpha-Setup.exe` (per-user, no admin rights).
- **Portable:** unzip and run `LitePilot.exe`. Settings and chats stay beside it; API keys stay in Windows Credential Manager on that PC.
- **API key:** Settings > pick a preset > Add profile > enter model and key > Save.
- **Claude or ChatGPT login:** Settings > pick the profile > Sign in. The official CLI downloads on first sign-in, after you confirm.

| Keys | Action |
|---|---|
| Alt+Space (configurable) | Open or hide |
| Enter / Shift+Enter | Send / new line |
| Esc | Stop a reply, otherwise hide |
| Tab | Move between typing and the chat |
| Page Up / Page Down, Ctrl+Up / Ctrl+Down | Scroll the chat |
| Ctrl+H | Previous chats (type to search, arrows, Enter, Del) |
| Ctrl+N | New chat |
| Ctrl+[ and Ctrl+] | Older or newer chat |
| Ctrl+K | Next account |
| Ctrl+M | Account, model, and effort menu |
| Ctrl+T | Keep on top |
| Ctrl+Shift+C | Copy the last code block or table |
| Ctrl+, | Settings (Tab or arrows, Enter, Esc, 1 to 5 on the first page) |
| F1 | List of shortcuts |

Replies show text, tables, code, links, and embedded PNG, JPEG, and GIF images. Web images appear as links.

## Build

```
build-release.cmd
python tools/e2e.py <empty-temp-folder> [--cli]
```

`src/core` is platform-neutral; a future macOS or Linux front end adds a sibling of `src/windows`.

## Performance

About 0.7 MB on disk, about 0.3 MB of RAM hidden in the tray, and about 9 to 14 MB with the window open. The
AI engine for sign-in accounts runs separately and is squeezed when idle. Measured numbers, settings, and how it is
built are in [docs/PERFORMANCE.md](docs/PERFORMANCE.md).
