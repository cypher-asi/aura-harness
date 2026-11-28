### Cloud agents never run here while the cloud is down, and refused mail is reported

##### Fixed

- An agent placed on a cloud machine no longer runs on this computer
  when the cloud couldn't be connected at start-up. Without the cloud it
  used to look like a local agent, so its work ran here, a second copy
  of it in the project folder. Its work now connects the cloud first;
  when that fails, it isn't run: the agent shows in red, its chat says
  why, and whoever was owed its answer is told.
- A message one agent sends another on a different cloud machine,
  which that machine refuses (its agent is sealed there, say), is now
  reported to its sender. So is one the machine couldn't be reached
  for. Both used to be lost, with only a line in the log.
