### Startup takes input at once with many agents

##### Fixed

- With many agents, and especially with cloud agents, the app froze
  after it appeared: no key or click did anything until it had
  connected to the cloud and resumed everything the last run cut
  short. It now takes input as soon as the chat is on screen, and does
  that work in the background between your actions. Owed reports,
  restored queues and cut-short turns still come back, in the same
  order.
- The app now opens on the channel or agent you were last in, instead
  of showing another chat first and switching.
