---
id: 26b76d8d-b7b6-467b-acf6-3a7853cb1c35
name: "Engine keys"
desc: "Registers engine public keys, signs engines in by checking a signature made with the private key that never leaves their machine, and rotates and revokes keys, so an admin can cut off one machine — and every agent it started — at once without touching the others."
creator: "iter engine fleet 2026-10-11"
teststate: inherit
level: component
owner: bespoke
children:
  codedirs:  ["{topdir}/iter_data/src/keys.rs", "{topdir}/iter_core/src/enginekey.rs"]
  codenodes: []
  tests:     []
  reqs:      []
timestamps: {create: "2026-10-11 00:00:00Z", last_modified: "2026-10-11 00:00:00Z", last_tested: ""}
---

# Long Description

## Summary

How a fleet of engine machines proves who they are.

An engine machine makes its own Ed25519 key pair with `iter_engine key new`. The private key stays in `~/.iter/engine.key` on that machine. An admin pastes only the public key (`iter_` + 64 hex) into the webui's Engine keys screen (the 🔑 button in the header) and names the engine. Nothing secret is copied. An engine is a machine, not a person: it signs in as the engine itself (`iter_engine:<engine>`, role `engine`), with no user behind it and no `owns` edge, and acts only for its own engine (`settings::machine_engine`).

How it works: `iter_core/src/enginekey.rs` holds the shapes. They are the public key format, the key id (the first 12 hex of the key's SHA-256), the login message and the session prefix. `iter_data/src/keys.rs` keeps one `engine_key` row per key: the engine, a note, the public key, who added it and when, and `revoked` (the time it stops working; "" means never). Sign-in is `POST /auth/engine`, with no bearer. The engine signs its key id, its clock and a one-time nonce. A live key, a clock within 5 minutes, an unused nonce and a valid signature earn a session token: `iter_` + a JWT naming the key (`kid`). `check_session`, called from `api::authenticate`, re-checks on every request that the session's key is still registered, unrevoked and the engine's. Revoking a key therefore stops the engine and its agents at once, and the session's 24-hour expiry only limits a copied session nobody revoked. The time a key was last used goes in a separate row (sk `used`). The admin API is `GET /api/keys`, which returns the keys and the older engine users still on record. `POST /api/keys` adds a key; `POST /api/keys/{id}/rotate` takes the new public key and `grace_minutes`. `POST /api/keys/{id}/revoke` and `DELETE /api/keys/{id}` (revoked keys only) complete it.

What goes in and out: the webui's `keysScreen` (index.html) is the only caller of the admin API. The engine's client (`iter_engine/src/client.rs`) signs in, renews its session, and moves to a rotated key once it is accepted. The `iter_` prefix on a session lets a proxy in front of iter_data, such as an AWS load-balancer rule, send engine requests past a browser login.

Why it matters: each machine of a fleet has its own credential and nothing secret ever travels. One lost laptop is revoked without touching the others, and a rotation needs no restart.

Example: a laptop is lost. An admin opens Engine keys and presses revoke on that laptop's row. Its next request, and every request of the agents it started, gets 401 "engine key revoked"; every other engine keeps working.
