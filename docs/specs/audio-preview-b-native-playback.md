# Spec B: Native in-place audio playback (no TUI handoff)

Status: **proposal** — being A/B'd against Spec A (`tap` launch).

## Goal

Press a key on an audio file in elio and it starts playing **inside elio** —
no leaving the TUI, no new process UI. Scroll to another file and hit the key
again: previous track stops, new one plays. The preview pane stays the source
of truth (metadata + cover art already exist in `src/preview/audio.rs`).

This is the "audition while scrolling" UX. No existing TUI does this well
inside a file manager; the ecosystem convention (`mpv --no-video`
fire-and-forget) is exactly the primitive we formalize.

## Design

### Player backend

- Spawn one managed child at a time:
  - Linux/BSD/Windows: `mpv --no-video --really-quiet --no-terminal {path}`
  - macOS fallback: `afplay {path}` (mpv may also be present)
- Tool detection at startup, same pattern as the existing `ffprobe_available`
  / `ffmpeg_available` checks in the preview pipeline.
- Fallback chain: `mpv` → `ffplay` → `afplay` (macOS) → none. If none,
  binding is a no-op with a status hint.

### State

New module `src/app/audio_player.rs` (or `src/preview/playback.rs` if we want
it preview-scoped — lean app-scoped: playback outlives preview rebuilds):

```rust
struct AudioPlayer {
    child: Option<Child>,
    path: Option<PathBuf>,   // currently playing file
}
```

- `play(path)`: if a child for `path` is already running → kill it and restart
  from the beginning (re-audition). If a different file is playing → kill,
  spawn new.
- `stop()`: kill + reap.
- Kill semantics reuse the cancel/kill/reap pattern from
  `src/preview/external_commands.rs` (children are already killed+reaped there
  on cancel — extract or mirror that helper).
- SIGTERM first; no zombie tolerance (elio already reaps children explicitly).

### Triggers

- `Action::PlayAudio` (default `p`), active only when selection is
  `FileClass::Audio`:
  - no track playing → play selected file
  - selected file already playing → stop (toggle) — simpler mental model than
    restart-on-press; document either choice
- Implicit stop on:
  - `Action::Quit` / `Action::QuitWithoutCd`
  - selection moves to a non-audio item? — **no**, keep playing (user may want
    to navigate while listening); only explicit toggle or quit stops it
- Optional second action `Action::StopAudio` (default `P` or unbound) if the
  toggle behavior feels ambiguous.

### UI feedback

- Preview header/status bar: `▶ Title — Artist (0:34)` while playing, using
  metadata already extracted by `src/preview/audio.rs` (don't re-probe).
- Progress: v1 = static "▶" marker only. Later: poll
  `mpv --input-ipc-server=/tmp/...` JSON IPC every ~500 ms for position/duration
  and render a mini progress bar in the preview pane. IPC is **out of scope
  for v1**.
- Theme: reuse existing accent/progress-bar palette tokens; no new theme keys
  for v1 (add `[preview.playback]` later if the marker needs styling).

### Configuration

```toml
[audio]
player = "mpv"              # "mpv" | "ffplay" | "afplay" | "auto" (default)
command = "mpv --no-video --really-quiet {path}"   # full override, optional
```

- `auto` = detect first available in the fallback chain.
- Keep the setting optional; defaults must work with mpv alone.

### Cancellation & edge cases

- Player must not block the event loop: spawn detached, poll `try_wait()`
  in the existing tick loop; when the process exits naturally (track ended),
  clear state and drop the ▶ marker.
- File deleted/unreadable mid-session: mpv exits with error → same natural
  cleanup path, no elio-side error surface.
- Terminal exit (SIGTERM to elio): kill the child (add to whatever signal/cleanup
  path elio already uses for preview subprocesses).
- Long paths / spaces: spawn via `Command` args (no shell), path as a single
  argument.

## Files touched (approx)

- `src/config/key_bindings/action_bindings.rs` — `Action::PlayAudio` (+ `StopAudio`?)
- `src/input_handling/keyboard.rs` — dispatch
- `src/app/audio_player.rs` (new) — child lifecycle, try_wait polling hook
- `src/app/` tick loop — poll player state
- `src/config/` — `[audio]` section parsing + tests
- `src/ui/status_bar.rs` or preview header — ▶ marker
- `src/preview/external_commands.rs` — possibly extract shared kill/reap helper
- `docs/` + `CHANGELOG.md`

## Non-goals (v1)

- No seek, no next/prev, no queue/playlist, no volume control, no waveform,
  no IPC progress bar.
- No audio format decoding in-process (no rodio/symphonia dependency) —
  delegate to mpv so format support is free.

## Open questions

1. Toggle (`p` stops the same file) vs restart (`p` always plays selected)?
2. Does playback continue when selection moves within the same folder? (spec
   says yes — confirm)
3. Status-bar marker vs preview-header marker — which surface does elio use
   for ephemeral state today?
4. Windows: is `mpv` the expected baseline, or do we need `potplayer`/`afplay`-
   equivalent detection? (mpv ships Windows builds; likely fine.)
