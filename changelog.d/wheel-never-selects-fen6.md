### The wheel never moves a highlight, in any list

##### Changed

- PROJECTS and a channel's HISTORY now scroll under the wheel through
  the same `WheelViews` as every other list, instead of flags of their
  own. The wheel scrolls a list's rows and leaves its highlight, and
  so what is open, alone; the arrows and clicks move it as before.
- A test turns the wheel over every tab of the agents panel, the
  Record panel and the side panel, with and without a channel open, and
  over every dialog, and fails if any highlight moves. A tab or dialog
  added later does not build until it is part of that test.
