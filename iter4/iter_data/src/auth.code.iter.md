---
id: 4d95e9e1-4347-486a-8739-5c10706ef234
name: "Login and tokens"
description: "Checks passwords against stored argon2 hashes and issues and verifies signed login tokens that carry the user's role and a revocation number, so every request can be tied to a person or engine and old tokens can be cancelled."
simple_description: "How iter knows who is asking and whether they are allowed in."
level: component
owner: bespoke
teststate: inherit
children:
  codedirs:   ["{topdir}/iter_data/src/auth.rs"]
  codenodes:  []
  inputs:     []
  outputs:    []
  bizreqs:    []
  techreqs:   []
  tests:      []
---

# Long Description

Auth turns a username and password into a token, and a token back into a user and role. It is a small module of pure helpers; the HTTP API calls it.

How it works: `iter_data/src/auth.rs: hash_password` stores passwords as argon2id hashes with a random salt, and `verify_password` checks a login attempt against one. `mint_token` issues an HS256 JSON Web Token (a signed string) whose claims are the user name (`sub`), the role (`admin`, `user`, `engine` or `viewer`), a `tokenver` number and an expiry. `verify_token` checks the signature and expiry. The signing secret comes from `load_secret`: the `ITER_JWT_SECRET` environment variable if set, otherwise a secret file beside the server, generated on first start so restarts keep sessions valid.

What goes in and out: the HTTP API's `login` handler calls `verify_password` and `mint_token` (24-hour web sessions); the `user_token` route mints long-lived engine tokens; the `AuthUser` extractor calls `verify_token` on every request and then compares the token's `tokenver` with the user's row. `iter_data/src/main.rs: bootstrap_admin` uses `hash_password` to create the first `admin` user. Nothing here touches the database directly.

Why it matters: tokens are checked without a session table, and revocation still works: raising `tokenver` on a user's row makes every token issued before it fail with "token revoked". Without this module anyone reaching port 8300 could change the queue.

Example: an engine's token leaks; an admin bumps that engine user's `tokenver` from 3 to 4, and the next request made with the old token gets 401 "token revoked".
