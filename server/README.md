# PeetMail sync server

Keeps PeetMail's settings (signatures, snippets, splits and so on) the same on all your devices. You run it yourself. Devices encrypt everything before uploading it, so the server only holds ciphertext it can't read. People sign in with a mail account they already use in PeetMail, so the server stores no passwords and never sends email. The design is in [docs/SYNC.md](../docs/SYNC.md).

> PeetMail itself can't connect to a sync server yet. That's the next stage of the work.

## Running it

With Docker, from the repository root:

```sh
docker build -f server/Dockerfile -t peetmail-sync .
docker run -d --name peetmail-sync --restart unless-stopped \
  -p 3000:3000 -v peetmail-sync:/data \
  -e ORIGIN=https://sync.example.com \
  -e ALLOW_REGISTER=@example.com \
  peetmail-sync
```

Without Docker (Node 22.18 or later, and pnpm):

```sh
pnpm install --filter peetmail-sync
pnpm --filter peetmail-sync build
cd server && ALLOW_REGISTER=@example.com node build
```

Put it behind a reverse proxy that terminates HTTPS (Caddy, nginx, Traefik…). PeetMail will only connect over plain http to `localhost`.

## Settings

| Variable | Meaning |
|---|---|
| `ALLOW_REGISTER` | **Required.** Who may create a sync account, separated by commas or spaces: `priya@example.com` (one address), `@example.com` (a domain), `tenant:<id>` (everyone in a Microsoft Entra tenant), `*` (anyone) or `none` (nobody new). Linking another mail account to an existing sync account is always allowed. |
| `ORIGIN` | The public URL, e.g. `https://sync.example.com`. Needed behind a proxy. |
| `PORT` | Port to listen on (default `3000`). |
| `DATABASE_PATH` | The SQLite file (Docker: `/data/sync.db`; otherwise `data/sync.db`). |
| `MICROSOFT_CLIENT_IDS` | Microsoft apps whose sign-ins are accepted. Default: PeetMail's own. If you use [your own Entra app](../docs/user/your-own-app.md), list both. |
| `GOOGLE_CLIENT_IDS` | Google apps whose sign-ins are accepted. Empty by default, which turns Google accounts off; set it to your Google app's client ID. |
| `MICROSOFT_DISCOVERY_URL`, `GOOGLE_DISCOVERY_URL` | The OpenID configuration URLs. Leave them alone; the tests point them at a local stand-in. |
| `BODY_SIZE_LIMIT` | Largest request (Docker default `4M`). |

### Matching addresses

- **Microsoft:** address and domain rules match the account's sign-in name: the UPN for work accounts, the address for personal ones. Entra's `email` claim isn't verified, so it's never used. For a whole organisation, `tenant:<id>` is the most precise rule.
- **Google:** rules match the account's address, and only if Google has verified it.

## Looking after it

- **Backups:** copy the SQLite file. WAL mode is on, so use `sqlite3 sync.db ".backup copy.db"`, or stop the server and copy the files.
- **Upgrades:** pull, rebuild and restart. The server migrates its database when it starts.
- **Logs:** the server logs when an account is created (with its address) or deleted, and any errors. It never logs tokens or item contents.
- **Removing someone:** they can delete their sync account from PeetMail. To remove them yourself, delete their row from `users` (the rest goes with it): `DELETE FROM users WHERE id = (SELECT user_id FROM identities WHERE email_hint = 'priya@example.com');`

## Development

```sh
pnpm --filter peetmail-sync dev     # http://localhost:5173 (needs ALLOW_REGISTER)
pnpm server:check                   # svelte-check
pnpm server:test                    # Vitest
pnpm --filter peetmail-sync db:generate   # after changing src/lib/server/schema.ts
```

- `src/lib/server/service.ts` holds every call's logic, independent of HTTP.
- `src/lib/server/oidc.ts` checks id_tokens.
- `src/routes/v1/` contains thin route handlers.
- `src/lib/protocol.ts` has the request and response types.
- `protocol-fixtures/examples.json` holds one example of each request and response. The server's tests check against it, and the Rust client's tests will too.
