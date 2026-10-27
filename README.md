# MacAi

A small, native macOS chat app for Apple's on-device Foundation Model. Written in
Rust; no Python runtime, browser, web server, account, API key, or cloud service.

## Run

Double-click **`dist/MacAi.app`** in Finder. It opens directly as a normal Mac app,
with its own icon in the Dock and **no Terminal window**. You can also drag it to
Applications or launch it with:

```sh
open dist/MacAi.app
```

Finder displays `MacAi.app` as one movable item. Like other macOS apps, it is
technically a bundle containing the self-contained Rust executable, its icon,
and macOS application metadata. There is no installer or extra runtime.
It uses macOS's `/usr/bin/fm`; the model is supplied by Apple, not bundled in the app.

If you used the earlier bare `MacAi` executable, quit that window first and use
`MacAi.app` from now on. Your existing chats and drafts are preserved automatically.

**Requires:** an Apple Intelligence-capable Mac, macOS 27 with `fm`, and Apple
Intelligence enabled with its model already downloaded. This build targets Apple
Silicon. An initial Apple model download or terms acceptance may need to happen
outside MacAi; chatting afterward is offline.

```sh
./dist/MacAi.app/Contents/MacOS/MacAi --check
```

If needed, enable Apple Intelligence in System Settings. If `fm` asks for terms
acceptance, run `fm license` in Terminal; MacAi does not accept terms on your behalf.

## Using MacAi

- Write a message and press **Enter** to send. **Shift+Enter** adds a new line.
- **Command+N** starts a conversation. Use the sidebar to search and reopen history.
- Replies stream as they are generated. **Stop reply** cancels the local process;
  **Retry reply** retries a stopped or failed message.
- The `...` menu renames or deletes the selected conversation. Deletion asks for
  confirmation. Copy buttons copy messages; replies support Markdown and code blocks.
- Drafts, conversation selection, messages, and model context are saved automatically.
- You can browse other conversations while a reply is being written. Only one
  generation runs at a time.
- **MacAi > About MacAi** shows the creator credit and a clickable GitHub profile link.

## Privacy and storage

MacAi invokes `fm respond --model system` directly, passing prompts through stdin.
It uses `--resume` and `--save-transcript` to retain the same model context between
turns and app launches. There is no network listener, telemetry, remote image
fetching, or automatic link opening.

Everything is stored under:

```text
~/Library/Application Support/MacAi/
    history.json
    history.lock
```

History is **local plaintext, not encrypted**, in an owner-only folder with an
owner-readable/writable history file. Normal macOS backups may include it. Saves
are atomic; another MacAi instance cannot overwrite the same history. Unreadable
or unsupported history is reported rather than silently replaced. Temporary
model transcripts are kept in private subfolders here and removed after each
request. A force-kill or power loss can leave a `.generation-*` folder; it is safe
to remove such folders when MacAi is closed.

Existing `fm chat` sessions in `~/.fm/sessions/` are not imported or modified.
Stopped or failed turns remain visible, but are **not** added to the model's
context; retries use the last successfully completed turn. Apple's model has a
finite context window and safety rules. When a conversation gets too long, MacAi
shows the model's error and asks you to start a new conversation rather than
silently discarding earlier context.

## Build

With Rust 1.86 or newer and Xcode Command Line Tools:

```sh
sh scripts/build.sh
```

The result is the Finder-ready **`dist/MacAi.app`**. The build uses the Swift/AppKit
and `iconutil` tools included with macOS and Xcode Command Line Tools to generate
the blue app icon. The same artwork is embedded in the sidebar, welcome screen,
and replies, so the in-app and Dock icons match. Swift is not needed to run the
resulting app. Cargo needs internet
access to download dependencies on the first build, but the built app does not.
The build script verifies the bundle and applies a local ad-hoc signature, not
Apple Developer ID notarization. A copy downloaded onto another Mac may be subject
to Gatekeeper.

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

For development, `cargo run --locked` opens the app. To capture the rendered window
after its layout settles (and then close it):

```sh
MACAI_SCREENSHOT_TO=/tmp/macai.png cargo run --locked --features screenshot
```

The capture helper is not included in the release build. To exercise real local
inference and conversation resumption as well as the ordinary tests:

```sh
cargo test --locked live_model_remembers_context_after_history_is_reopened -- --ignored --nocapture
```

The New conversation button embeds Ubuntu Bold, Copyright 2011 Canonical Ltd.,
under the Ubuntu Font Licence 1.0. Its license and copyright notice are included
in `assets/fonts/` and the app's `Contents/Resources/licenses/` directory.
