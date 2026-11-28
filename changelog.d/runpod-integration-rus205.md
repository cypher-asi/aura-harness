### Agents can use Runpod GPUs

##### Added

- A Runpod integration: agents can list GPU types with prices, list,
  create, start, stop and terminate pods, read pod logs, check the
  balance, and run serverless jobs. Add a Runpod API key under
  Integrations > Runpod (or set `RUNPOD_API_KEY`).
- Spending limits guard it at every approval level: by default a pod may
  cost at most $1.00 an hour and have one GPU, at most two pods run at
  once, and a serverless job runs at most 300 seconds. Terminating a pod
  needs its id repeated. See `docs/integrations.md`.
