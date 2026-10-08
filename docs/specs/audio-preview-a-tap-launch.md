# Spec A: Audio quick-preview via `tap` launch

Status: **implemented** (default key `i`; the spec proposed `p`, which is taken by paste — see Open questions).

## Goal

From elio, one keystroke opens a lightweight TUI audio browser rooted at the
currently selected file/folder. The user scrolls/fuzzy-filters and plays
samples there, then exits back to elio.

## Chosen tool: [timdubbins/tap](https://github.com/timdubbins/tap)

- Rust, cross-platform (Linux / macOS / Windows), ~900 KB binary, actively
  maintained, fzf-style fuzzy finder + minimal player.
- Accepts path arguments:
  - `tap <folder>` — browse that folder
  - `tap <file>` — play directly
- Runner-up considered: `RIZAmohammadkhan/TerminalMusicPlayer` ("Trix") — also
  takes a path arg, but is a heavier full player (gapless, lyrics, MPRIS2,
  ALSA mixer). Keep in mind as a swap-in; the launch-command abstraction below
  makes the tool interchangeable.

## Design

### New action

- `Action::PlayAudio` (or `OpenAudioBrowser` — decide on naming) in
  `src/config/key_bindings/action_bindings.rs`.
- Default binding: `p`.
- Only active when the selected item's `FileClass` is `Audio` or the item is a
  directory (for folder browsing). If bound to a non-audio item: show a status
  message, do nothing.

### Launch mechanics

- Reuse elio's existing command-launch path (same machinery as `Open` /
  `OpenWith` / `ShellHere`), terminal = current terminal.
- Argument selection:
  - selected item is a file → `tap <file>`
  - selected item is a directory → `tap <directory>`
- **Blocking vs detached:** run synchronously in the current terminal (like an
  editor open), so on exit the user is back in elio with state intact. If that
  proves annoying (e.g. no tty handoff), fall back to detaching.

### Configuration

Extend the open-rules config (`src/config/open.rs`) or add a small dedicated
key — preference: a dedicated, explicit setting so the default is discoverable:

```toml
[audio-preview]
command = "tap {path}"   # {path} = selected file or folder
```

- If `tap` is not on `$PATH` (detected at startup, like ffprobe/ffmpeg), the
  binding is a no-op with a status hint: "install `tap` to enable audio
  preview".
- Document the Trix swap: `command = "trix {path}"`.

### Non-goals

- No in-elio playback state, no status-bar "now playing", no stop/next/prev
  keys — tap owns all playback UX while it runs.
- No bundling/packaging of tap.

## Files touched (approx)

- `src/config/key_bindings/action_bindings.rs` — new action + default binding
- `src/input_handling/keyboard.rs` — dispatch
- `src/config/` — `audio-preview` command setting + parsing/tests
- `src/app/` (or wherever `ShellHere` launches) — launch handler
- `docs/` + `CHANGELOG.md`

## Decisions made during implementation

1. **Default key: `i`.** The spec proposed `p` (or `P` if `p` stayed with
   paste). In the shipped default bindings `p` is paste and `P` is
   fullscreen preview, so `i` — a free letter — became the default. It is
   fully configurable via `[keys] play_audio`.
2. **Blocking launch: yes, reused verbatim.** elio already does clean
   terminal suspend/handoff for long-running programs (see
   `run_open_command_in_terminal` and the `PendingTerminalTask` flow in
   `src/terminal_runtime/tui_event_loop.rs`). The new
   `PendingTerminalTask::PlayAudio` reuses that path, so elio state is
   intact when the player exits.
3. **Directories: yes, supported.** The action works on any directory
   (folder browsing) as well as audio files, matching the spec's
   `tap <folder>` use case. Non-audio, non-directory items show a status
   hint and do nothing.
