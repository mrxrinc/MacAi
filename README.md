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
before chatting; chatting afterward is offline.

On first launch, MacAi checks Apple's CLI license status. If it has not been
accepted, a setup window displays the current terms from `fm license --show`.
Read them, tick the agreement checkbox, and choose **Accept and continue**.
macOS then asks for **administrator approval** because Apple's agreement applies
to every user on the Mac. You do not need to open Terminal, and MacAi never sees
or stores the administrator password. Choosing **Not now** leaves chat disabled
but still allows browsing existing conversations.

MacAi verifies that the terms have not changed, answers Apple's consent prompt
through a private pseudo-terminal using macOS's built-in `expect` utility, and
checks the recorded agreement before enabling chat. It never accepts on startup
or sends a chat message as an agreement. Changed terms require fresh consent.
An error or cancelled administrator prompt can be retried from the setup window.

```sh
./dist/MacAi.app/Contents/MacOS/MacAi --check
```

If needed, enable Apple Intelligence in System Settings and wait for its model
to download, then choose **Check again** in MacAi.

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
This command creates a **local development build only**, with an ad-hoc signature.
It is **not notarized** and macOS can block it when downloaded onto another Mac.
An ad-hoc signature, Apple Development certificate, or App Store distribution
certificate does not fix that warning.

## Distribute to other Macs

A normal first launch on another Mac requires **Developer ID Application signing
and Apple notarization**. These require an Apple Developer Program account and a
valid Developer ID Application certificate with its private key in your keychain.
The app cannot create that identity or bypass Apple's verification itself.

After installing the certificate, store notarization credentials in Keychain
using Apple's interactive command (do not put passwords or API keys in this repo):

```sh
xcrun notarytool store-credentials MacAi-notary
```

Then create the distributable release:

```sh
MACAI_SIGNING_IDENTITY='Developer ID Application: Your Name (TEAMID)' \
MACAI_NOTARY_PROFILE='MacAi-notary' \
sh scripts/release.sh
```

The script requires the correct certificate, signs with hardened runtime and a
secure timestamp, submits the app to Apple, waits for an **Accepted** result,
staples and validates the notarization ticket, and runs a Gatekeeper assessment.
Only after all steps succeed does it publish **`dist/MacAi.zip`**, containing the
stapled `MacAi.app`. The stapled ticket allows offline verification on other Macs.
Notarization logs are kept under `target/notarization/`. Distribute this ZIP,
not the ad-hoc app produced by `scripts/build.sh`. Do not modify the app bundle
after signing and notarization.

Building the next local development version replaces `dist/MacAi.app` with an
ad-hoc build again; the previously published ZIP is not changed. No script disables
Gatekeeper or removes quarantine attributes.

## Development checks

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

For development, `cargo run --locked` opens the app. To capture the rendered window
after its layout settles (and then close it):

```sh
MACAI_SCREENSHOT_TO=/tmp/macai.png cargo run --locked --features screenshot
```

For a read-only preview of the first-run terms screen on a Mac where they have
already been accepted, also set `MACAI_PREVIEW_LICENSE=1`. The preview cannot
submit an agreement or request administrator approval.

The capture helper is not included in the release build. To exercise real local
inference and conversation resumption as well as the ordinary tests:

```sh
cargo test --locked live_model_remembers_context_after_history_is_reopened -- --ignored --nocapture
```

The New conversation button embeds Ubuntu Bold, Copyright 2011 Canonical Ltd.,
under the Ubuntu Font Licence 1.0. Its license and copyright notice are included
in `assets/fonts/` and the app's `Contents/Resources/licenses/` directory.
