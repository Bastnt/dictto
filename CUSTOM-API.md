# Custom API and dictation settings

Dictto supports OpenAI-compatible transcription and chat APIs, including a server you host yourself.
Open **Settings > API** to configure:

| Setting | Purpose |
| --- | --- |
| API base URL | Endpoint base including `/v1`, for example `http://127.0.0.1:8000/v1` |
| API Key | Credential required by the selected endpoint |
| Transcription model | Model ID served by `/audio/transcriptions` |
| Cleanup model | Model ID served by `/chat/completions` |
| Text cleanup | Disable to paste the original transcript without a chat request |

The default endpoint and models remain OpenAI's. For a custom server, enter its model IDs
and its API key. Both transcription and cleanup use the configured base URL and key.
Transcription accepts a multipart audio upload and expects a JSON response with `text`.
Cleanup uses an OpenAI-compatible chat response with JSON-schema output support.
If cleanup fails, Dictto pastes the original transcript.

Turning cleanup off retains the selected model for later use. Credentials are stored
in Windows Credential Manager; they are never embedded in the build.

## Change the hotkey

In **Settings > General**, click the hotkey **Change** button. Press your shortcut and
release it to save. Escape or Cancel abandons capture. The change applies immediately
and persists across restarts; the default remains Ctrl+Win.

Supported shortcuts combine Ctrl, Alt, Shift or Win with one letter, digit, function key,
Space, Enter, Tab or Backspace. Two or more modifiers alone and standalone F1 through F24
also work. Avoid shortcuts reserved by Windows or another application.
Capture temporarily suspends dictation activation.

The recording overlay is hidden while idle. It appears during recording and processing.
Request failures display a short error message for 15 seconds or until the next recording,
then the overlay disappears again.

## Build and test

```powershell
pnpm install --frozen-lockfile
pnpm build
node apps/desktop/node_modules/vitest/vitest.mjs run src/components/settings/__tests__/ApiTab.test.tsx src/components/settings/__tests__/HotkeySetting.test.tsx src/components/settings/__tests__/GeneralTab.test.tsx --root apps/desktop
cd apps/desktop/src-tauri
cargo test --lib
```

The fork disables upstream updater endpoints to preserve its custom API features.

Enable **Warm transcription model** to send a silent one-second WAV at app startup and recording start through the configured transcription API. Concurrent warmups are skipped. Recording does not wait, failures are silent, and the response is discarded. Off by default; providers may bill these requests.
