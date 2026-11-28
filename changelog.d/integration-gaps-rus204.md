### Cloud mode uses the boxd key from the Integrations panel, and plan tokens are sealed

##### Fixed

- A boxd key saved in the Integrations panel passed Test, but cloud mode
  went on reading only `.env` and the environment. Now cloud mode uses
  the stored key first, then `BOXD_TOKEN` and `BOXD_API_KEY` as before,
  so existing setups keep working. The log names the source (never the
  key), and so does `aura cloud status`. `aura cloud setup` and
  `/cloud setup` say the panel's key is in use instead of minting
  another into `.env`.
- Without an OS keychain, linked plans' tokens and the zOS session used
  to be written in plain text. They are now sealed with the local
  store's cipher, under `AURA_STATE_KEY_FILE` or
  `~/.aura/auth/vault.key`.
  - A plain file from before is sealed on its first read, only after
    its sealed copy reads back the same. Until then the plain file is
    kept, so no sign-in is lost.
  - A sealed file whose key is missing or wrong is a clear error naming
    the key file, never a sign-out.
