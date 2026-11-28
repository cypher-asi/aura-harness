### One standard way to add an integration

##### Added

- A new crate, `aura-engine-integrations`, is the one way to plug an
  external service into aura. An integration declares a manifest (id,
  name, icon, what it provides, how it signs in, its settings and the
  tools it gives agents) and implements a status, a health check and its
  tools. The registry does the rest the same way for every service:
  connect (only after a passing health check), configure, test,
  disconnect, which agents or channels may use it, and the agent tools,
  named `<id>_<tool>`.
- Credentials are stored sealed with the local store's key, never in
  plain text, never logged, and never sent to a cloud machine unless the
  integration needs them there, and then only as boxd's placeholder.
  Every call has a time limit. `docs/integrations.md` maps how services
  were wired before and says how to add one.

##### Fixed

- The workspace's layer test failed on main because
  `aura-exec-merge-queue` lacked its `//! Layer: exec` tag. It has it now.
