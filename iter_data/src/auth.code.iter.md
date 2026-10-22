---
id: 4d95e9e1-4347-486a-8739-5c10706ef234
name: "Login and tokens"
desc: "Checks passwords against stored argon2 hashes and issues and verifies signed login tokens that carry the user's role and a revocation number, so every request can be tied to a person or engine and old tokens can be cancelled without a session table."
creator: "iter migrate5"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/src/auth.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-02 23:05:35Z", last_modified: "2026-10-22 03:00:00Z", last_tested: ""}
---

# Long Description

## Summary

How iter knows who is asking.

Auth turns a username and password into a token, and a token back into a user and role. It is a small module of pure helpers; the HTTP API calls it. What a known user may do in a given project is decided afterwards by the project access rules (`authz.rs`).

How it works: `iter_data/src/auth.rs: hash_password` stores passwords as argon2id hashes with a random salt, and `verify_password` checks a login attempt against one. `mint_token` issues an HS256 JSON Web Token (a signed string) whose claims are the user name (`sub`), the role (`admin`, `user` or `viewer`; `engine` only in an engine session), a `tokenver` number and an expiry. `verify_token` checks the signature and expiry. Two signing secrets come from `load_secret`: people's tokens use `ITER_JWT_SECRET`, else the secret file (`--secret-file`); engine sessions use `ITER_ENGINE_SESSION_SECRET`, else `engine_session.secret` beside it. Each is generated on first start (owner-only) so restarts keep sessions valid, and neither kind of token verifies with the other's secret.

What goes in and out: the HTTP API's `login` handler calls `verify_password` and `mint_token` (web sessions); `POST /api/users/{user}/token` mints a long-lived token for a person (an admin for anyone, a person for themself, 1–365 days). Engines never sign in as users: `mint_claims` signs their sessions with the engine secret, whose claims name their key (`kid`), and a person's token claiming role `engine` is refused. Passwords (2026-10-17): the first admin and anyone whose password an admin set must choose their own first (`must_change_password`; `authenticate` refuses everything else); `POST /api/users/{user}/password` needs the current password, a new one of 12+ characters, and ends every other session; sign-ins are limited to 5 failures a minute per user and per address. `api::authenticate`, used by the `AuthUser` extractor and the project access layer on every request (MCP included), verifies the token. An engine session (`iter_` + JWT) then goes to Engine keys to re-check its key. For any other token it compares the token's `tokenver` with the user's row. Server startup uses `hash_password` to create the first `admin` user. Nothing here touches the database directly.

Why it matters: raising `tokenver` on a user's row makes every token issued before it fail with "token revoked"; without this module anyone reaching port 8400 could change the queue.

Example: an engine's token leaks; an admin bumps that engine user's `tokenver` from 3 to 4, and the next request made with the old token gets 401 "token revoked".
