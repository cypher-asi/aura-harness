### GitHub and boxd are integrations

##### Added

- GitHub and boxd are the first two integrations on the new model. Each
  has a status (a `gh` login or token for GitHub, noting a GitHub App for
  cloud pushes; a key for boxd), a Test that makes one real call (GitHub's
  `/user`; boxd's key exchange), and a sealed place for a token or key
  entered in the Integrations panel. How they work today is unchanged:
  `gh`, `GH_TOKEN`, `GITHUB_TOKEN` and `BOXD_API_KEY` in `.env` still work
  as before.
- The REPO tab also uses a GitHub token stored that way, after
  `GH_TOKEN` and `GITHUB_TOKEN`.
- An integration's agent tools are given to the agents allowed to use it,
  as `<id>_<tool>`, while it is connected.
