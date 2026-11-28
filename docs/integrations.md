# Integrations

## Runpod

Runpod rents GPUs by the hour (pods) and by the second (serverless
endpoints). The Runpod tools let agents find GPUs, start and stop pods,
read their logs and run serverless jobs, over the
[Runpod REST API v2](https://docs.runpod.io/api-reference-v2/overview)
and the serverless job API.

### The key

Create an API key on Runpod's
[Credentials page](https://console.runpod.io/user/settings) and add it
under Integrations > Runpod. It is kept in the integration secret storage,
not in a plain file. Without one there, the tools read `RUNPOD_API_KEY`,
the variable Runpod's own CLI uses. The key is only sent as the
`Authorization` header: it never appears in logs, errors or tool output.
The health check calls `GET /v2/account` and shows whether the key works.

### Tools

| Tool | What it does | Kind |
| --- | --- | --- |
| `runpod_list_gpu_types` | GPU types with memory, price per GPU per hour and availability | read |
| `runpod_list_pods` | Your pods with status, GPU, cost per hour and uptime | read |
| `runpod_get_pod` | One pod, with image, ports, SSH command and env variable names | read |
| `runpod_pod_logs` | The latest log lines of a pod (at most 500) | read |
| `runpod_balance` | The account balance | read |
| `runpod_job_status` | Status and output of a serverless job | read |
| `runpod_create_pod` | Create and start a pod | spends |
| `runpod_pod_action` | Start, stop or restart a pod | spends (start) |
| `runpod_run_job` | Run a job on a serverless endpoint, waiting for it or not | spends |
| `runpod_terminate_pod` | Delete a pod and its disk | destroys |

Under the `ask` and `strict` approval levels, the spending and destroying
tools ask before they run. Every request times out (20 s, or the job wait
for `runpod_run_job`), and every result is capped at 32 KiB.

### Spending limits

The tools refuse a call over these limits before they send anything, at
every approval level, `yolo` included. Change them under Integrations >
Runpod.

| Setting | Default | What it bounds |
| --- | --- | --- |
| `max_hourly_usd` | 1.00 | A new or started pod's price per hour (catalog price x GPU count). An unknown price is refused. |
| `max_gpu_count` | 1 | GPUs in one pod. |
| `max_running_pods` | 2 | Pods running at once, counting the one asked for. |
| `max_job_seconds` | 300 | A serverless job's execution timeout, sent with the job. |

`runpod_terminate_pod` also needs the pod id repeated in
`confirm_pod_id`. Stopping a pod is never limited: it ends GPU billing.
