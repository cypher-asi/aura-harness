### The wheel scrolls the Integrations list

##### Fixed

- Turning the wheel over INTEGRATIONS now scrolls its rows and leaves
  the highlight where it was, as in every other list. It used to do
  nothing, so a list taller than the panel could not be scrolled. A key
  that moves the highlight brings it back into view.
- The file-size check failed on main because `app/agent_views.rs` had
  grown past 500 lines with the Integrations panel; it is back under.
