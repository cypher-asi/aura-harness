### The terminal's local database is encrypted

##### Security

- The terminal, `aura -p` and the desktop app now keep their database
  (channel posts, prompts, History summaries, the agent list) encrypted
  on disk with AES-256-GCM, as `aura-node` does in sealed mode. Before,
  all of it sat on disk as plain text.
- The key is kept in the OS keychain, one per data directory. On a
  machine without a keychain it is kept in `<data dir>/.aura-store-key`,
  readable by you only. Set `AURA_STATE_KEY_FILE` to keep it in a file of
  your choosing instead. Without the key the database can't be read: the
  app stops with an error that says where it looked.
- On the first start, an unencrypted database is copied into an
  encrypted one, checked key by key, and swapped in; the unencrypted
  copy is then deleted. If that fails, the app keeps working on the
  unencrypted data and tries again on the next start.
