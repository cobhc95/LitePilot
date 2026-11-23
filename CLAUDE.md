# Rules for every agent working on LitePilot

## Always verify internally first (added 2026-11-23)
Never report a fix as done without proving it inside this machine first: run the relevant path (test, e2e, or a live probe) and check the output. In particular, don't ship a version bump whose only evidence is that it compiled.

## After every fix or change
Do **not** commit, tag, push, or create a GitHub release unless the user explicitly says so (e.g. "push", "release", "upload"). Building locally is fine; publishing is not. The user owns the publish decision — an approved build is not approval to push it.
1. **Bump the version** (patch) in `Cargo.toml`. It is the only place the version lives.
2. **Build** with `build-release.cmd` (runs the tests, then makes `dist\LitePilot-<version>-Setup.exe` and `-Portable.zip`).
3. **Only if the user explicitly asks to push/release**: commit, tag `v<version>` (always a new tag, never move one), push, then `gh release create` with both the Setup EXE and the Portable ZIP.
4. **Wipe old versions**: delete the older local builds in `dist`. Do not delete old GitHub releases or force-move tags unless the user asks.
5. **Launch the new version**: quit the running LitePilot, install the new Setup EXE, and start it again. Start it **outside the Claude app's process job**, or closing the Claude desktop app kills LitePilot too: `Start-Process explorer.exe '"<full path to LitePilot.exe>"'` (explorer becomes the parent, like a normal user launch). Never `Start-Process` the exe directly, and do not leave the installer's own relaunch running (it inherits the job).

Quit the real LitePilot before running e2e tests (only one instance can run).
