### Integrations can declare numbers, advanced settings and costly tools

##### Added

- An integration's settings can be numbers with bounds (`FieldKind::Number`),
  checked when set, so the panel refuses "abc" for a spending limit.
- A setting can be advanced (an API URL override): the panel hides it
  until it is set. GitHub's API URL and boxd's console URL are advanced.
- A tool can say it spends money or destroys something
  (`ToolEffect::Spends`, `ToolEffect::Destroys`), besides reading or
  changing; a health check can add a detail line; and an integration can
  refuse a call itself (`IntegrationError::Refused`), told apart from a
  failure.
