### Restarts keep reports and resume agents

##### Fixed

- After the app restarts, channel members whose turns it cut short carry on by themselves, and their reports still reach the coordinator. Before, they sat idle until the stall check nudged them about two minutes later.
- Cloud turns that ran on while the app was closed are followed again on reopen. A turn that finished meanwhile is reported, and one cut short carries on. Before, those reports never arrived.
- Everything queued for an agent comes back after a restart: typed messages, `!` commands, `/build`, `/compact`, and images up to 4 MiB in all. A larger image is left out and the chat says so. Before, only inbox mail came back; the rest vanished without a word.
- Mail an agent sends is saved before the sender is told it went, so a crash no longer loses it. How many messages each thread has carried, and which agents the user parked, now survive a restart too.
