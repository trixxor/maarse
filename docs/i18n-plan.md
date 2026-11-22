# Language support: design record

Status: **built** (2026-11-22). How to use it and how to add a language:
[i18n.md](i18n.md). This page records what was decided and why.

## Decisions

* Languages for now: English and Dansk. A new language is a new file in
  `i18n/`, no code change.
* The language follows the device (Automatic). Settings can choose one by
  hand, and that wins. Anything missing falls back to English.
* The English text in the code is the key. Existing texts are not renamed;
  translations are inserted around them. (A key schema such as
  `chats.send` and generated `strings.xml` was considered and dropped: it
  would have changed about 390 places in the Android app and every text in the
  web client.)
* Host logs, command-line help and the Heltec display stay English.
* Text that people write is never translated.
* System-made text (alerts, receipts, statuses) is translated where it is
  shown; the wire format is unchanged. A check of the code found no place
  that puts human-readable system text into a message sent to others
  (alerts carry a numeric event code, delivered notices are packets).

## What was built

* `i18n/da.json`: one file for all surfaces (about 2,700 texts).
* Web: `maarse-client/i18n.js` (DOM translation by exact text and `{0}`
  patterns), Language button in Settings, host routes, built-in files and
  drop-in files in the data folder. Used by the PC client, the server
  login and dashboard, and the flasher.
* Android: `Translator.kt` (tested), `I18n.kt`, the translating `Text`,
  Language row under Settings > App, files copied into the app at build.
* Windows: tray app (`maarse-desktop`) and installer script.
* `tools/i18n_check.py` and `.github/workflows/i18n.yml`.

## Differences from the first plan

* No generator script and no `strings.xml`; the lookup is at run time.
* No plural forms in the file. English and Danish use the same two forms,
  so a text with a count is written as two entries when needed; a language
  with more forms can add entries per count. Revisit when such a language is
  added.
* Screens translate where the text is shown, so the rule for user-written
  text is `RawText` (Android) and `translate="no"` (web).

## Not done

* A Language list (instead of the cycling button) in the web client.
* Danish versions of `docs/GUIDE.md`, the protocol specification and the
  HTML manual (the user guide in `docs/user/` has a Danish copy).
* The Android system language setting (Android 13 per-app language) is not
  connected; the language is chosen inside the app or follows the device.
* Verification on a physical Android device, and a compiled installer.
