### QUEUE times read relative, and one dot for QUEUE and Integrations

##### Changed

- A channel's QUEUE shows when a merge was queued and finished as
  `just now`, `2 minutes ago` or `5 h ago`, then `yesterday 14:32`,
  `Nov 26, 14:32`, and with the year once it is another year's. The
  times stay current while the tab is open, redrawn only when one next
  reads differently. The pointer on a time shows it in full, to the
  second. The formatting is one shared helper,
  `components::relative_time`, for other panels to use.
- QUEUE's branch link is underlined only while the pointer is on it, as
  the chat's links are.
- The status dot is shared by QUEUE and the Integrations panel: `●` in
  the state's color for a thing that is there, hollow `○` for one not
  set up.
