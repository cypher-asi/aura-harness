# Aura Gateway (TypeScript)

TypeScript gateway for Claude Code reasoner integration with the Aura Swarm.

## Purpose

This gateway wraps the Claude API to provide a "propose-only" reasoning interface. The Rust kernel calls this gateway to get action proposals, which it then authorizes and executes.

**Important:** This gateway does NOT execute tools. It only suggests actions. The kernel handles all authorization and execution.

## Setup

1. Install dependencies:
   ```bash
   npm install
   ```

2. Set environment variables:
   ```bash
   export ANTHROPIC_API_KEY="your-api-key"
   export PORT=3000  # optional, default 3000
   export CLAUDE_MODEL="claude-opus-4-6"  # optional
   export MAX_TOKENS=4096  # optional
   ```

3. Run the server:
   ```bash
   # Development
   npm run dev

   # Production
   npm run build
   npm start
   ```

## API

### Health Check

```
GET /health

Response:
{
  "status": "ok",
  "version": "0.1.0",
  "model": "claude-opus-4-6"
}
```

### Generate Proposals

```
POST /propose

Request:
{
  "agent_id": "hex-encoded-32-bytes",
  "tx": {
    "tx_id": "hex-encoded-32-bytes",
    "agent_id": "hex-encoded-32-bytes",
    "ts_ms": 1704672000000,
    "kind": "user_prompt",
    "payload": "base64-encoded-payload"
  },
  "record_window": [
    {
      "seq": 1,
      "tx_kind": "UserPrompt",
      "action_kinds": ["reason", "delegate"],
      "payload_summary": "..."
    }
  ],
  "limits": {
    "max_proposals": 8
  }
}

Response:
{
  "proposals": [
    {
      "action_kind": "delegate",
      "payload": "base64-encoded-tool-call",
      "rationale": "Need to read the file to understand..."
    }
  ],
  "trace": {
    "model": "claude-opus-4-6",
    "latency_ms": 1234,
    "metadata": {
      "input_tokens": "100",
      "output_tokens": "50"
    }
  }
}
```

## Action Kinds

- `reason` - Think about the problem
- `memorize` - Store information for future reference
- `decide` - Make a decision
- `delegate` - Delegate to a tool (filesystem, commands)

## Available Tools (via delegate)

The gateway can propose these tools, which the kernel will execute:

- `fs.ls` - List directory contents
- `fs.read` - Read file contents
- `fs.stat` - Get file metadata

## Architecture

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│   Rust Kernel   │────>│  TS Gateway     │────>│  Claude API     │
│   (aura-kernel) │<────│  (this service) │<────│                 │
└─────────────────┘     └─────────────────┘     └─────────────────┘
        │
        │ Authorizes & executes
        v
┌─────────────────┐
│  Tool Executor  │
│  (aura-tools)   │
└─────────────────┘
```

The kernel:
1. Calls gateway with transaction context
2. Receives proposals (not executed)
3. Applies policy to authorize proposals
4. Executes authorized actions via ToolExecutor
5. Records everything in the append-only log
