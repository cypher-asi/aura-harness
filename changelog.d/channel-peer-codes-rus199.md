### Channel members reach each other by code

##### Fixed

- Members of a channel see each other in `list_agents` and can message
  each other with `send_to_agent` by code (`RUS198`) or agent id,
  locally and from cloud machines. Before, a member the coordinator had
  spawned was left out of every other member's list and couldn't reach
  them, and sending by code from a cloud machine always failed as an
  unknown agent.
- Sending to an unknown agent now names the agents you can reach; a code
  two agents share asks for the agent id.
