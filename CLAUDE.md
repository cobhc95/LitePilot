# Rules for every agent working on LitePilot

## After every fix or change (do this without asking)
1. **Bump the version** (patch) in `Cargo.toml`. It is the only place the version lives.
2. **Build** with `build-release.cmd` (runs the tests, then makes `dist\LitePilot-<version>-Setup.exe` and `-Portable.zip`).
3. **Upload to GitHub**: commit, tag `v<version>` (always a new tag, never move one), push, then `gh release create` with both the Setup EXE and the Portable ZIP.
4. **Wipe old versions**: delete the older local builds in `dist`. Do not delete old GitHub releases or force-move tags unless the user asks.
5. **Launch the new version**: quit the running LitePilot, install the new Setup EXE, and start it again.

Quit the real LitePilot before running e2e tests (only one instance can run).
