# Mycel Desktop

The graphical app, built with [Tauri](https://tauri.app) (Rust backend +
React/TypeScript frontend) on top of `mycel-core`.

**Not started yet: this is Phase 1.** See [../../docs/roadmap.md](../../docs/roadmap.md).

Planned shape:

```
apps/desktop/
  src/            React + TypeScript UI (servers, channels, chat, settings)
  src-tauri/      Rust side: runs a mycel-core node, exposes commands/events to the UI
```

The UI never talks to the network itself. It sends commands to the Rust side
(`send_message`, `create_invite`, `kick_member`, ...) and renders the events it
receives, the same split the CLI already uses.
