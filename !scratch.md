1. Engine ↔ iter_data

Today:
- Each engine has its own key pair... Nothing secret is ever pasted.
> agreed, this is good as-is, don't change.  Please only add:
> - an iter_engine CLI function that prints the public key on demand, for easy setup
> - an iter_engine CLI function to generate a new private/public key, for cycling keys

The JWT proves who is calling, but it doesn't encrypt anything
> this is bad; we need to encrypt traffic.
Proposal:
- iter_data serves HTTPS itself. On first start it creates a self-signed certificate and prints its fingerprint. You give that fingerprint to each engine once (the installers ask for it), and the engine trusts only that certificate. 
> we already have an ED25519 public/private key setup already; can't we have:
> - new iter_engine setup, engine generates public/private keypair
> - user registers engine and public key with the iter_data server
> - new iter_engine signs it's initial message to iter_data proving itself
> - iter_data encrypts it's own "self-signed certificate and prints its fingerprint" 
>   or ED25519 public key or JWT or whatever, and returns to iter_engine (encrypted)
> - iter_engine decrypts and saves the iter_data public key
> - from there, they can setup JWT or whatever
> - iter_data should maintain one key for each engine, so it can revoke one engine without cycling keys for all others

- The engine refuses http:// to anything except localhost.
> yes
- Smaller fixes: a switch to turn off the old token logins, and owner-only permissions on the secret file.
> if this is for backwards compatibility, no need; everything is decommissioned
> if this is for key revocation, yes

There are also some smaller issues:
- The old 365-day ITER_ENGINE_TOKEN logins are still accepted.
> no backwards compatibility is needed; these are all now obsolete
> we will need a way to cycle new secrets between the iter_engine and iter_data server, i.e.
> both iter_engine and iter_data need a way to send an encrypted message to the other saying
> "replace the old public_key with this new public_key" for standard key rotation.
> if a key is revoked, assume it's compremised and do not fire this message; this requires a user to manually setup again.

- The server's signing-secret file is written without restricted permissions.
> we should allow the iter_engine CLI to expose the public key (locally to the admin), no private key access should be allowed.  Whatever we can do to restrict access is wise.  
- Engine and user tokens share one signing secret.
> no; we should split this into different things.  Users have passwords, engines have keys.  iter_data can validate /manage users and passwords. 



2. Claude Code tokens on the engine
> this is all ok... these are claude keys, so user will have to cycle these manually.


Gaps:
- Every token reaches every process. At startup the engine exports everything in .env, so every Claude session and script it starts inherits every account's token, not just the one it needs.
- The plaintext file is the only source. There's no support for a secret store.
> we should probably add a secret store, but make it optional; either environment variable injection on start (for things like AWS), or an ~/.iter/.env file, or a key management system, if configured. 

On servers, and KMS: KMS itself only holds encryption keys, not values. The AWS s are Secrets Manager and SSM Parameter Store (SecureString). Both encrypt withKMS, and the instance reads them through its IAM role, so no AWS keys sit on disk.

Proposal:
- Keep .env as the default. 
> agreed
Add an --env-command "<cmd>" option: a command the engine runs at start and every few minutes, which prints KEY=value lines. 
> prints it where?  what good does that do? 
That one hook covers Secrets Manager, Parameter Store, 1Password (op), Vault and the macOS Keychain, with a documented recipe for each (e.g. aws ssm get-parameters-by-path --path /iter/engine/<name>/ --with-decryption). 
> not clear on this... are you saying the `env-command "<cmd>"` executes in the shell, and replaces envars every few minutes?  so if you change a claude key in say 1Password, it will refresh within a few minutes?
It doesn't bloat the engine with an AWS SDK.
> agreed, i don't like bloat.
- Pass each Claude session only its own account's token.
> agreed
- Missing tokens: keep today's flags, and also log the missing variables clearly at startup.
> agreed, just ensure no keys in logs (only first or last X characters if absolutely needed)


3. Users in iter_data

Proposal:
- Forced first change: 
> agreed
- Changing a password: require the current password, apply a minimum length of 12, and log out other sessions.
> agreed
- Login attempts: about 5 failures per minute per user and IP.
> agreed
- Cleanup: remove the first-boot password from the env file and SSM once it's been changed.
> agreed
