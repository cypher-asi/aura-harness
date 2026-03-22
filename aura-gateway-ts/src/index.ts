import 'dotenv/config';
import express, { Request, Response, NextFunction } from 'express';
import { ClaudeReasoner } from './reasoner';
import { ProposeRequestSchema, ProposalSetSchema } from './types';
import type { GatewayConfig } from './types';

// Load config from environment (dotenv loads .env automatically)
const config: GatewayConfig = {
  port: parseInt(process.env.PORT || '3000', 10),
  anthropicApiKey: process.env.ANTHROPIC_API_KEY || '',
  model: process.env.CLAUDE_MODEL || 'claude-opus-4-6',
  maxTokens: parseInt(process.env.MAX_TOKENS || '4096', 10),
};

// Validate config
if (!config.anthropicApiKey) {
  console.error('ERROR: ANTHROPIC_API_KEY environment variable is required');
  process.exit(1);
}

// Create reasoner
const reasoner = new ClaudeReasoner(config);

// Create Express app
const app = express();
app.use(express.json({ limit: '10mb' }));

// Health endpoint
app.get('/health', (_req: Request, res: Response) => {
  res.json({
    status: 'ok',
    version: '0.1.0',
    model: config.model,
  });
});

// Propose endpoint
app.post('/propose', async (req: Request, res: Response, next: NextFunction) => {
  try {
    // Validate request
    const parseResult = ProposeRequestSchema.safeParse(req.body);
    if (!parseResult.success) {
      res.status(400).json({
        error: 'Invalid request',
        details: parseResult.error.errors,
      });
      return;
    }

    const request = parseResult.data;
    console.log(`[propose] agent=${request.agent_id.slice(0, 16)}... tx_kind=${request.tx.kind}`);

    // Call reasoner
    const result = await reasoner.propose(request);

    console.log(`[propose] proposals=${result.proposals.length} latency=${result.trace?.latency_ms}ms`);

    res.json(result);
  } catch (error) {
    next(error);
  }
});

// Error handler
app.use((err: Error, _req: Request, res: Response, _next: NextFunction) => {
  console.error('Unhandled error:', err);
  res.status(500).json({
    error: 'Internal server error',
    message: err.message,
  });
});

// Start server
app.listen(config.port, () => {
  console.log(`Aura Gateway listening on port ${config.port}`);
  console.log(`Using model: ${config.model}`);
  console.log('Endpoints:');
  console.log(`  GET  /health  - Health check`);
  console.log(`  POST /propose - Generate proposals`);
});
