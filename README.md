# MacAi

A small, native macOS chat app for Apple's Foundation Models. It uses the
on-device model by default, so chats stay on your Mac and work offline. Written in
Rust. You don't need Python, a browser, an account or an API key, and the model
isn't bundled: MacAi talks to Apple's built-in `/usr/bin/fm` command.

## Build it yourself (on a brand-new Mac)

These steps take you from a fresh Mac to a working `MacAi.app`. Run the commands
in **Terminal** (Applications > Utilities).

1. **Check your Mac.** You need:
   - an Apple Silicon Mac,
   - macOS 27 or newer, which includes Apple's `fm` command,
   - Apple Intelligence turned on in **System Settings**, with its model fully
     downloaded.

   This should list `system` with a ✓:

   ```sh
   /usr/bin/fm models
   ```

2. **Install Apple's Command Line Tools.** These provide `git`, `swift`,
   `iconutil`, `codesign` and `plutil`, which the build uses. Then follow the
   dialog that appears:

   ```sh
   xcode-select --install
   ```

3. **Install Rust** with the official installer, [rustup](https://rustup.rs),
   and accept the defaults:

   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

   Open a **new** Terminal window, then check that the version is 1.86 or newer:

   ```sh
   cargo --version
   ```

4. **Get the code:**

   ```sh
   git clone https://github.com/mrxrinc/MacAi.git
   cd MacAi
   ```

5. **Build the app.** The first build downloads Rust packages, so it needs
   internet and takes a few minutes. Later builds are faster.

   ```sh
   sh scripts/build.sh
   ```

6. **Open it:**

   ```sh
   open dist/MacAi.app
   ```

   You can also double-click `dist/MacAi.app` in Finder or drag it to
   Applications. On first launch, MacAi shows Apple's terms for the `fm` tool.
   Read and accept them (see [First launch](#first-launch)), then start chatting.

> **This build works only on the Mac that built it.** `scripts/build.sh` signs the
> app ad hoc and doesn't notarize it, so macOS may block it if you copy it to
> another Mac. To share MacAi, see [Distribute to other Macs](#distribute-to-other-macs).

### Troubleshooting

- **`swift`, `iconutil` or `git`: command not found**, or a prompt asks to install
  developer tools. Finish step 2 (`xcode-select --install`), then run the build
  again.
- **`cargo: command not found`.** Open a new Terminal window after installing
  Rust, or run `. "$HOME/.cargo/env"`.
- **Rust is too old** (an error mentions `rust-version` or 1.86). Run
  `rustup update`.
- **`fm` is missing, or MacAi says Apple Intelligence isn't ready.** Make sure
  you're on macOS 27, turn on Apple Intelligence in System Settings, and wait for
  its model to download. Then choose **Check again** in MacAi, or run:

  ```sh
  ./dist/MacAi.app/Contents/MacOS/MacAi --check
  ```

- **macOS says MacAi "can't be opened" or can't be verified.** This happens
  when the app was built on a different Mac. Build it on this Mac with the steps
  above, or use a notarized release.

## First launch

MacAi checks whether Apple's `fm` terms have been accepted. If not, it shows the
current terms from `fm license --show`. Tick the checkbox and choose **Accept and
continue**. macOS then asks for **administrator approval**, because the agreement
applies to every user on the Mac. MacAi never sees or stores your password.
**Not now** keeps chat turned off, but you can still browse saved conversations.

MacAi records consent only through that button: it never accepts on startup or
treats a chat message as consent. It checks that the terms haven't changed, answers Apple's
prompt through a private pseudo-terminal using macOS's built-in `expect`, and confirms the recorded agreement before turning chat on. If the terms change,
it asks again. If something goes wrong or you cancel the administrator prompt,
you can try again from the setup window.

MacAi only needs the on-device `system` model. On macOS 27.2, `fm models` also
lists Private Cloud Compute (`pcc`). If `pcc` is unavailable, local chat still
works; only the optional Apple Cloud model is affected (see below).

## Using MacAi

- Write a message and press **Enter** to send. **Shift+Enter** adds a new line.
- **Command+N** starts a conversation. Use the sidebar to search and reopen history.
  The chevron at the top left hides or shows the sidebar.
- After a conversation's first reply, MacAi asks the on-device model in the
  background for a short 3-6 word title. It runs as a separate `fm respond` call
  without the conversation's transcript, so the chat's context is untouched. The
  first message stays as the title until the summary arrives or if it fails, and a
  title you renamed is never replaced.
- Replies stream as they are generated. **Stop reply** cancels the local process;
  **Retry reply** retries a stopped or failed message.
- Each conversation in the sidebar has a `...` button, shown on hover and on the
  selected chat, to rename or delete it. The header's `...` menu does the same for
  the open conversation. Deleting asks for confirmation.
- Copy buttons copy messages. Replies support Markdown and code blocks.
- MacAi follows the macOS light or dark appearance by default. Open
  **MacAi > Settings…** (Command+,) to choose System, Light or Dark; the choice is saved.
- Drafts, conversation selection, messages, and model context are saved automatically.
- You can browse other conversations while a reply is being written. Only one
  generation runs at a time.
- Chats use the on-device model (`fm --model system`). MacAi also supports
  Apple Cloud, but its switch is hidden for now because Apple only allows Private
  Cloud Compute from Terminal (see below). When it is shown again, the
  **On-device / Apple Cloud** switch under the message box chooses the model. Apple Cloud uses Apple's
  Private Cloud Compute (`fm --model pcc`) and needs internet. MacAi remembers your
  choice for new conversations. A conversation's context belongs to the model that
  started it, so switching models in a conversation that already has messages
  starts a fresh conversation and moves your unsent draft into it.
- Apple currently reports Private Cloud Compute as *"not available in this context.
  Please use the Terminal app."* for apps other than Terminal. When that happens,
  MacAi hides the Apple Cloud switch and its notice (the code is commented out in
  `src/app.rs`, ready to re-enable) and keeps everything on-device. Conversations
  saved on Apple Cloud stay readable; sending in one continues in a new on-device
  conversation.
- To keep Apple's small on-device model focused, MacAi sends it only the 6 most
  recent exchanges (plus any instructions) when a conversation continues. The full
  conversation stays saved and visible.
- In longer chats the on-device model can get stuck repeating an earlier answer.
  When a reply matches an earlier one word for word, MacAi shows a notice offering
  **Answer without earlier context** (asks again from a clean slate, replacing the
  repeated reply) or **Start new chat with this question**.
- **MacAi > About MacAi** shows the creator credit and a clickable GitHub profile link.

## Privacy and storage

MacAi runs `fm respond --model system` (or `--model pcc` for Apple Cloud) and
passes your prompt through stdin. It adds no extra instructions, so replies match
what `fm respond` gives you in Terminal. It uses `--resume` and `--save-transcript`
to keep the model's context between turns and app launches. On-device chats stay on
your Mac; Apple Cloud chats are processed by Apple's Private Cloud Compute. There is
no network listener, telemetry, remote image loading or automatic link opening.

Everything is stored under:

```text
~/Library/Application Support/MacAi/
    history.json
    history.lock
```

History is **plain text, not encrypted**. The folder is owner-only and the history
file is readable and writable only by you. Normal macOS backups may include it.
Saves are atomic, and a second MacAi instance can't overwrite the same history.
Unreadable or unsupported history is reported, never silently replaced. Temporary
model transcripts live in private subfolders here and are removed after each
request. A force-quit or power loss can leave a `.generation-*` folder behind; it's
safe to delete while MacAi is closed.

MacAi doesn't import or change `fm chat` sessions in `~/.fm/sessions/`. Stopped or
failed replies stay visible but are **not** added to the model's context; retries
continue from the last completed turn. Apple's model has a limited context window
and safety rules. If a conversation gets too long, MacAi shows the model's error and
suggests a new conversation rather than silently dropping earlier context.

## How the build works

`scripts/build.sh` generates the app icon with `swift` and `iconutil`, builds the
Rust app in release mode, and assembles and ad-hoc signs **`dist/MacAi.app`**. The
same artwork is used for the Dock icon and inside the app. Swift is needed only to
build MacAi, not to run it.

The result is a **local development build**. It isn't notarized, so macOS can block
it on other Macs. Neither an ad-hoc signature nor an Apple Development or App Store
certificate fixes that.

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
