# Privacy Policy — Dictto

**Last updated:** November 2026

## Summary

Dictto is a privacy-first voice-to-text app. Dictto sends audio and text to the API endpoint you configure for the actions you invoke. We collect zero telemetry. You bring your own API key — we never see it.

## How Dictto Works

1. You hold a hotkey and speak
2. Audio is captured locally on your device
3. Audio is sent directly from your device to your configured API for transcription and optional text cleanup
4. The transcribed text is pasted at your cursor
5. Dictto discards the dictation audio after processing and stores the dictation text in local history.

In Dictto Native, recording a clone sample saves a WAV locally. Each clone request uploads that sample with the text to synthesize. Speech-to-speech sends the new recording for transcription and sends the resulting text for synthesis. The speech-to-speech transcript stays in memory. Read-aloud uses the selection or clipboard text as input. Dictto caches the synthesized audio from all three actions in the local Speech Audio folder. It saves each complete result as one WAV and keeps up to six results. Replaying a cached result refreshes its recency. Adding a seventh result evicts the least recently used one. The folder stores no transcripts or copies of the input text.

When virtual microphone routing is enabled, Dictto supplies synthesized audio to Windows applications listening to Dictto Microphone. During a temporary switch, Dictto stores the original and temporary communications capture endpoint IDs locally so it can restore the previous input after an interruption.

## Your API Key (BYOK)

Dictto uses a **Bring Your Own Key** model:

- You provide the key for your configured API endpoint in Settings
- Your API key is stored locally in Windows Credential Locker (encrypted by Windows)
- Your API key is sent directly from your device to that endpoint
- If you use OpenAI, its services are governed by [OpenAI's privacy policy](https://openai.com/privacy) and [usage policies](https://openai.com/policies/usage-policies)

## Data We Collect

**None.** Dictto v0.1 collects zero telemetry, zero analytics, zero crash reports. There are no Dictto servers — the app runs entirely on your device.

## Data Stored Locally

Dictto stores the following data on your device only:

| Data | Location | Purpose |
|---|---|---|
| Settings (language, microphone, etc.) | `%LOCALAPPDATA%\com.dictto.app\dictto.db` | App preferences |
| Transcription history | `%LOCALAPPDATA%\com.dictto.app\dictto.db` | Your reference |
| API key | Windows Credential Locker | Authentication with the configured endpoint |
| Native speech output cache | `%LOCALAPPDATA%\com.dictto.app\Speech Audio\` | Up to six synthesized WAVs for reuse and access through **Speech → Open audio folder** |
| Current native clone sample | `%LOCALAPPDATA%\com.dictto.app\voice-sample.wav` | Reference voice for cloned speech |

You can delete all local data by uninstalling Dictto and choosing "Remove all data" during uninstallation, or by manually deleting the `%LOCALAPPDATA%\com.dictto.app\` folder.

## Data Sent to Third Parties

| Recipient | Data sent | When | Why |
|---|---|---|---|
| Configured transcription API | Audio recording (WAV) | Dictation or speech-to-speech | Transcription |
| Configured cleanup API | Transcribed text | Optional dictation cleanup | Text cleanup |
| Configured speech API | Text, voice settings and, for cloning, the current sample WAV | Native speech actions | Speech synthesis |

No data is sent to Dictto or any other party. The app communicates directly with your configured endpoint using your API key.

## No Accounts, No Servers

Dictto does not require an account. There is no Dictto backend, no user database, no authentication system. The app is fully self-contained on your device.

## Future Changes

If a future version of Dictto adds telemetry (e.g., anonymous usage analytics), it will be:

- **Opt-out** — enabled by default but easily disabled in Settings
- **Anonymous** — no personal data, no transcription content, no API keys
- **Documented** — this privacy policy will be updated before any telemetry is added

## Open Source

Dictto's source code is available at [github.com/dictto-app/dictto](https://github.com/dictto-app/dictto) under the AGPL-3.0 license. You can audit exactly what data the app accesses and where it sends it.

## Contact

For privacy questions, open an issue on [GitHub](https://github.com/dictto-app/dictto/issues).
