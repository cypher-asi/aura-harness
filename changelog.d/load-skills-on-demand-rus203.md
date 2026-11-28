### Skills load on demand

##### Changed

- An agent's skills no longer go into every turn's system prompt in full. The prompt now lists each skill's name, description and folder, and the agent calls the new `load_skill` tool to get a skill's instructions when a task needs them.
  - This applies to local agents from a permanent agent with skills, and to turns on a cloud node with skills installed for the agent.
  - For three typical skills, the system prompt falls from about 3,100 tokens to about 200.
- Cloud turns asked from the app still carry their skills in full, because the cloud machine doesn't have this computer's skill folders.
