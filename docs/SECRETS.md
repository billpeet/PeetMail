# Secrets and credentials

Everything the app and its releases depend on: where the live copy is, where the backup is, what breaks if it's lost or leaks, and how to rotate it. No values here. GitHub secrets and variables are under the repo's **Settings → Secrets and variables → Actions**; only repo admins can change them, and secrets can't be read back.

Several of these end up inside the app (marked *in the binary*). The repo is public and the installer is downloadable, so treat those as public: they're safe only because of what they can and can't do.

| Credential | Where | Backup | If lost | If leaked |
|---|---|---|---|---|
| **Updater signing key** + password | Secrets `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Public half in `src-tauri/tauri.conf.json` | Password manager (Martin) | **No installed copy can update again.** Everyone reinstalls by hand from the next release | Someone could sign an update. Rotate (below) |
| **Android signing key** + password | Secrets `PEETMAIL_ANDROID_KEYSTORE_BASE64`, `PEETMAIL_ANDROID_KEYSTORE_PASSWORD`. Certificate SHA-256 in `RELEASING.md` | Password manager (Martin) | **No phone can update PeetMail again.** Each uninstalls it, losing its local data, and installs the new build | Someone could build an APK that installs over PeetMail, given a way to get it onto a phone. Can't be rotated in place (below) |
| Sentry DSN (*in the binary*) | Variable `PEETMAIL_SENTRY_DSN`; Sentry project `metroid/peetmail` → Client Keys | Sentry | Get it from Sentry again | Fake crash reports. Rotate the client key |
| Sentry auth token | Secret `SENTRY_AUTH_TOKEN` (CI only: releases, symbols, source maps) | None needed: make a new one | Uploads are skipped with a warning; stack traces stop being readable | Rotate in Sentry → Settings → Auth Tokens |
| Telemetry gateway credential (*in the binary*) | Secret `PEETMAIL_OTLP_HEADERS` (`Authorization=Basic …`), variable `PEETMAIL_OTLP_ENDPOINT`. Server side: user `peetmail` in the OTLP basic-auth label of the `futura-observability` compose app (Dokploy, futura) | None needed: make a new one | Builds stop sending telemetry | Can only write telemetry (the route reaches Alloy's OTLP receiver only). Rotate, see `RELEASING.md` |
| Microsoft client ID (*in the binary*) | Secret `PEETMAIL_MS_CLIENT_ID`; app registration `PeetMail` in the `metroid.net.au` Entra tenant | Entra | Look it up in Entra | Not a secret: a public client has no password. If the registration is deleted, every account on it must sign in again |
| Google client ID + secret (*in the binary*) | Secrets `PEETMAIL_GOOGLE_CLIENT_ID`, `PEETMAIL_GOOGLE_CLIENT_SECRET`; Google Cloud project → Clients | Google Cloud console | Look them up in the console | A desktop client's secret isn't confidential (Google says so). Rotate the secret if it's abused |
| Feedback address | Variable `PEETMAIL_FEEDBACK_EMAIL` | n/a | The app hides the email option | Not secret |
| `GITHUB_TOKEN` | Made by GitHub for each run | n/a | n/a | Expires with the run |

Not secrets, though they look like them:

- `tests/harness/updater/test.key`: a test signing key, committed on purpose. Only harness builds trust it (`src-tauri/tauri.harness.conf.json`); release builds use the real public key.
- `PEETMAIL_*` overrides in `src-tauri/src/testenv.rs`: debug builds only.

Developer logins (`gh`, `az`, `sentry-cli` in `~/.sentryclirc`, SSH to futura) belong to the developer, not the project, and aren't needed to release.

## Rotating

- **Updater signing key.** Installed copies trust only the key they were built with, so a new key has to arrive in an update signed by the old one. Generate a new pair (`pnpm tauri signer generate -w <path> -p <password>`), put the new public key in `tauri.conf.json`, and release that build **signed with the old key**. Once installs have taken it, switch the two secrets to the new key and save it in the password manager. If the old key has leaked, do this at once and tell users to update; if it's lost, this isn't possible and users reinstall.
- **Android signing key.** Android has key rotation (APK signature scheme v3: the new key signed by the old), but neither Tauri nor this repo's Gradle setup uses it. If the key leaks or is lost, make a new keystore, set the secrets, release, and every phone uninstalls and reinstalls.
- **Sentry DSN.** Add a new client key in the Sentry project, set the variable, release. Disable the old key once installs have updated (disabling it first cuts their crash reports off).
- **Telemetry credential and Google secret.** Same pattern: add the new credential alongside the old, release, then remove the old once installs have updated. Steps for the gateway are in `docs/RELEASING.md`.
- **Sentry auth token.** Create a new token, set the secret, revoke the old one. Nothing installed uses it.
