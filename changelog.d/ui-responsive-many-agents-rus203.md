### The app stays responsive with many agents

##### Fixed

- With many agents at work, such as a channel of 200 members streaming and finishing turns, the UI no longer stutters behind the event loop.
  - Before, the loop rebuilt every agent list and resaved its state after each turn start, turn end and message, and was busy two thirds of the time.
  - Now the lists are rebuilt at most four times a second, and the loop's own saves at most five times a second. In the load test that cut the loop's busy time from 68% to 15%, and the worst wait for an event from 32 ms to under 1 ms.
- Nothing is saved later than before where someone waits on it: mail an agent sends, and the last save at shutdown, are still written at once.

##### Added

- A warning in the log whenever one event holds up the event loop for over 100 ms, naming what it was.
- A load harness (`docs/performance.md`) that runs the real app with 10 to 200 simulated agents and prints where the time goes.
