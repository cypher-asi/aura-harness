### Runpod is in the Integrations panel, and agents get its tools

##### Added

- Runpod is listed in the Integrations panel. Connect it with a Runpod
  API key (it is checked against your account first, and kept sealed),
  set its spending limits with Configure, and agents get its ten tools
  once it is connected: GPU types and prices, pods, pod logs, the
  balance, serverless jobs, and creating, starting, stopping and
  terminating pods.

##### Changed

- An integration's read-only tools now run without asking, and in plan
  and ask mode too; tools that change, spend or destroy still ask under
  the `ask` and `strict` approval levels and run in agent mode only.
  Before, every integration tool asked and ran in agent mode only.
- An integration connected while aura is running reaches agents on their
  next turn. Before, its tools were only offered after a restart.
