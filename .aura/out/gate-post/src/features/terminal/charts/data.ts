/*
 * The figures behind the charts posts embed, each with where it was published.
 * Dollar amounts are billions of USD; dates are when the figure was reported.
 */

export interface Source {
  label: string;
  href: string;
}

const SRC = {
  anthropicS1: {
    label: 'Anthropic S-1/A, Nov 10, 2026',
    href: 'https://www.sec.gov/Archives/edgar/data/2130357/000162828026072484/anthropicpbc-sx1ax2.htm',
  },
  anthropicRange: {
    label: 'TechCrunch',
    href: 'https://techcrunch.com/2026/11/10/anthropic-sets-ipo-price-range-of-70-to-78-per-share-to-reach-an-implied-valuation-of-1-404-trillion/',
  },
  openaiRound: {
    label: 'Bloomberg, Nov 6, 2026',
    href: 'https://www.bloomberg.com/news/articles/2026-11-06/openai-completes-11-billion-portion-of-funding-at-1-25-trillion-value',
  },
  openaiCommitments: {
    label: 'Bernstein via Investing.com, Nov 7, 2026',
    href: 'https://www.investing.com/news/stock-market-news/what-are-the-ai-lab-compute-commitments-5010113',
  },
  coreweave10q: {
    label: 'CoreWeave 10-Q, Q3 2026',
    href: 'https://www.sec.gov/Archives/edgar/data/1769628/000176962826000465/crwv-20260930.htm',
  },
  coreweaveCap: {
    label: 'S&P Global via Stock Analysis, Nov 13, 2026',
    href: 'https://stockanalysis.com/stocks/crwv/market-cap/',
  },
  nebiusQ3: {
    label: 'Nebius Q3 2026 results',
    href: 'https://evidinvest.com/blog/nbis-q3-2026-read-back-contracts-capex-financing-2026-11-13',
  },
  nebiusCap: {
    label: 'Investing.com, Nov 12, 2026',
    href: 'https://www.investing.com/news/stock-market-news/nebius-group-earnings-analysis-questions-answered-and-next-catalysts-93CH-5023091',
  },
  coreweaveTrace: {
    label: 'FINRA TRACE via Kilambi, Nov 13, 2026',
    href: 'https://news.kilambi.com/leverage/bond/u2069eac4/',
  },
  metaNotes: {
    label: 'Meta 424B5, Nov 3, 2026',
    href: 'https://www.sec.gov/Archives/edgar/data/1326801/000119312526448015/d135326d424b5.htm',
  },
  ftMarks: {
    label: 'FT via Dealroom, Oct 15, 2026',
    href: 'https://dealroom.co/news/other-1ni755j-investors-dump-data-centre-bonds-as-ai-borrowers-face-darwin-moment/',
  },
  siliconData: {
    label: 'Silicon Data B300 Rental Price Index',
    href: 'https://www.silicondata.com/products/silicon-index/b300',
  },
  kernelEvolve: {
    label: 'Meta Engineering',
    href: 'https://engineering.fb.com/2026/04/02/developer-tools/kernelevolve-how-metas-ranking-engineer-agent-optimizes-ai-infrastructure/',
  },
  flashinferContest: { label: 'arXiv 2608.14560', href: 'https://arxiv.org/abs/2608.14560' },
  kernelAgent: {
    label: 'PyTorch',
    href: 'https://pytorch.org/blog/kernelagent-hardware-guided-gpu-kernel-optimization-via-multi-agent-orchestration/',
  },
  kernelPro: { label: 'arXiv 2606.26453', href: 'https://arxiv.org/html/2606.26453' },
} satisfies Record<string, Source>;

/**
 * Compute contracted forward, against what the company is worth. The labs
 * have bought it forward; the neoclouds have sold it forward at fixed rates
 * (remaining performance obligations).
 */
export const COMPUTE_POSITION = {
  long: [
    {
      name: 'Anthropic',
      contracted: 900.9,
      value: 1404,
      note: '$900.9B committed, $1.40T at the top of its IPO range',
    },
    { name: 'OpenAI', contracted: 614, value: 1250, note: '$614B through 2030, $1.25T in its Nov 6 round' },
  ],
  short: [
    { name: 'Nebius', contracted: 38.7, value: 42.4, note: '$38.7B of contracts, $42.4B market cap' },
    { name: 'CoreWeave', contracted: 132.3, value: 29.55, note: '$132.3B of contracts, $29.6B market cap' },
  ],
  sources: [
    SRC.anthropicS1,
    SRC.anthropicRange,
    SRC.openaiRound,
    SRC.openaiCommitments,
    SRC.coreweave10q,
    SRC.coreweaveCap,
    SRC.nebiusQ3,
    SRC.nebiusCap,
  ],
};

/** Price in cents on the dollar, and the yield where it was reported. */
export const CREDIT_REPRICING = {
  bonds: [
    { name: 'Meta 5.85% notes due 2031', detail: 'Aa3, priced Nov 3', price: 99.998, yieldPct: 5.85 },
    { name: 'Hyperion SPV, leased to Meta', detail: 'Above par in July', price: 92, below: true },
    { name: 'QTS Excalibur, leased to Microsoft', detail: 'Oct 15', price: 88, below: true },
    { name: 'Oracle Michigan campus', detail: 'Par in July', price: 87 },
    { name: 'CoreWeave 9.75% notes due 2031', detail: 'Nov 13', price: 82.62, yieldPct: 14.87 },
  ],
  sources: [SRC.metaNotes, SRC.ftMarks, SRC.coreweaveTrace],
};

/** Daily readings, USD per GPU-hour, neocloud segment. */
export const B300_INDEX = {
  readings: [
    ['2026-10-15', 6.77],
    ['2026-10-16', 6.96],
    ['2026-10-17', 7.01],
    ['2026-10-18', 7.01],
    ['2026-10-19', 7.02],
    ['2026-10-20', 7.02],
    ['2026-10-21', 7.02],
    ['2026-10-22', 7.35],
    ['2026-10-23', 7.53],
    ['2026-10-24', 7.53],
    ['2026-10-25', 7.53],
    ['2026-10-26', 7.52],
    ['2026-10-27', 7.56],
    ['2026-10-28', 7.56],
    ['2026-10-29', 7.56],
    ['2026-10-30', 7.56],
    ['2026-10-31', 7.56],
    ['2026-11-01', 7.56],
    ['2026-11-02', 7.55],
    ['2026-11-03', 7.55],
    ['2026-11-04', 7.55],
    ['2026-11-05', 7.56],
    ['2026-11-06', 7.56],
    ['2026-11-07', 7.69],
    ['2026-11-08', 7.69],
    ['2026-11-09', 7.68],
    ['2026-11-10', 7.68],
    ['2026-11-11', 7.67],
    ['2026-11-12', 7.67],
    ['2026-11-13', 7.67],
  ] as const satisfies readonly (readonly [string, number])[],
  sources: [SRC.siliconData],
  /** The runs where the index stepped up, first day before the move to the day it landed. */
  steps: [
    { from: '2026-10-21', to: '2026-10-23' },
    { from: '2026-11-06', to: '2026-11-07' },
  ],
};

/**
 * One MW of critical IT as the deck's project-finance slide sizes it
 * (features/deck/slides.full.ts): 64 B300 nodes, 512 GPUs, fully utilized.
 * Rates are USD per GPU-hour; cost and value are millions of USD. Preliminary
 * ZODE estimates, not results.
 */
export const PER_MW = {
  gpus: 512,
  nodes: 64,
  rates: [
    { name: 'Contracted', rate: 5, detail: '5-year offtake at a fixed rate' },
    { name: 'Merchant', rate: 6.5, detail: 'GPU-hours sold on the open market' },
    { name: 'Inference', rate: 16.9, detail: 'Tokens on open models, net per GPU-hour' },
  ],
  /** GPUs $35.2M, data center conversion $5.0M, land and power $1.0M. */
  cost: { total: 41.2, gpus: 35.2, building: 5, land: 1 },
  /** Enterprise value per MW of CoreWeave, Nebius and IREN on October 13, 2026. */
  value: [50, 75] as const,
};

/** GPU-hours one MW sells in a year, fully utilized. */
export const GPU_HOURS_PER_MW = PER_MW.gpus * 24 * 365;

/** Revenue a year from one MW at a GPU-hour rate, in millions of USD. */
export const revenuePerMw = (rate: number): number => (GPU_HOURS_PER_MW * rate) / 1e6;

/** Each kernel's speed against the baseline it was measured on, as a multiple. */
export const AGENT_KERNELS = {
  results: [
    {
      name: 'Fused MoE, B200',
      detail: 'MLSys 2026 FlashInfer contest, vs FlashInfer',
      speedup: 1.71,
    },
    {
      name: 'Ads model inference, NVIDIA',
      detail: 'Meta KernelEvolve in production, vs torch.compile and vendor libraries',
      speedup: 1.6,
    },
    {
      name: 'KernelBench level 1, H100',
      detail: 'PyTorch KernelAgent, vs torch.compile',
      speedup: 1.56,
    },
    {
      name: 'MoE training kernel, Hopper',
      detail: 'KernelPro, vs hand-tuned Triton',
      speedup: 1.23,
    },
  ],
  sources: [SRC.flashinferContest, SRC.kernelEvolve, SRC.kernelAgent, SRC.kernelPro],
};

/** The stages ZODE has to prove, in megawatts of critical IT. */
export const ROADMAP = [
  {
    mw: 1,
    label: '1 MW',
    stage: 'First site',
    detail: 'Merchant compute and inference',
    proves: [
      'Merchant rates hold well above our cost per GPU-hour',
      'Inference earns more per GPU-hour than renting the GPU',
      'The cluster stays busy without an anchor customer',
    ],
  },
  {
    mw: 30,
    label: '30 MW',
    stage: 'Own site',
    detail: 'Bought and built with ZODE modules',
    proves: [
      "Powered land costs less than colocation over a GPU's life",
      'ZODE modules deploy faster and cheaper per MW',
      'Power routing yields more compute per MW',
      'GPUs are financeable without an offtaker',
    ],
  },
  {
    mw: 100,
    label: '100 MW+',
    stage: 'Production',
    detail: 'A repeatable build, financed without an offtaker',
    proves: [
      'Each site repeats the last at a falling cost per MW',
      'Demand absorbs 100 MW without selling it forward',
      'Each GPU generation finances the next',
    ],
  },
  {
    mw: 1000,
    label: '1 GW+',
    stage: 'Full stack',
    detail: 'Our own models, harness and wafers',
    proves: [
      'Open models stay close to the frontier',
      "Our own models earn more per MW than serving others'",
      'Volume justifies custom silicon and wafer supply',
    ],
  },
];

/* Gate, the router behind Aura's `gate` model id. Results are in gate-data.ts. */

const GATE_SRC = {
  sweBenchPro: { label: 'SWE-Bench Pro HARD-51', href: 'https://github.com/scaleapi/SWE-bench_Pro-os' },
  claudePricing: { label: 'Claude list prices', href: 'https://platform.claude.com/docs/en/about-claude/pricing' },
} satisfies Record<string, Source>;

export const GATE_SOURCES = [GATE_SRC.sweBenchPro, GATE_SRC.claudePricing];

/** One stage of a turn under Gate, in the order a turn meets them. */
export interface GateStage {
  id: string;
  name: string;
  /** What the stage does, in a few words, shown under its name. */
  does: string;
  /** Where it lives in aura-harness. */
  module: string;
  /** The model it ran on in Compare #37. */
  ranOn?: string;
  /** Its key in GATE_RESULTS.spend and .calls, for what it cost in Compare #37. */
  spend?: 'gate:classify' | 'gate:criteria' | 'gate:judge' | 'work' | 'escalated';
  points: string[];
}

export const GATE_FLOW: { stages: GateStage[]; learn: string; layers: { name: string; crate: string; holds: string }[] } = {
  stages: [
    {
      id: 'classify',
      name: 'Classify',
      does: 'Label the work',
      module: 'classify.rs',
      ranOn: 'Haiku 5.5',
      spend: 'gate:classify',
      points: [
        'One call to the cheapest fast model you can use',
        'One of 8 categories, and easy, medium or hard',
        'Answers through a label tool, with a 20 second limit',
      ],
    },
    {
      id: 'pick',
      name: 'Pick',
      does: 'Choose model and effort',
      module: 'router.rs',
      points: [
        'The cheapest model proven on this kind of work',
        'Otherwise the start tier: fast for easy work, standard for the rest',
        'Effort from difficulty: low, medium or high, never max to start',
      ],
    },
    {
      id: 'criteria',
      name: 'Criteria',
      does: 'Write down what done means',
      module: 'criteria.rs',
      ranOn: 'Sonnet 5.5',
      spend: 'gate:criteria',
      points: [
        'Acceptance criteria for code turns, written before the work starts',
        'Shown to the worker; reviews check the work against them',
      ],
    },
    {
      id: 'work',
      name: 'Work',
      does: 'Run the turn',
      module: 'aura-app event loop',
      ranOn: 'Sonnet 5.5',
      spend: 'work',
      points: [
        'The agent loop on the picked model, with its usual tools',
        'Large changes and planning are led: the lead hands edits to subtasks',
      ],
    },
    {
      id: 'verify',
      name: 'Verify',
      does: 'Read the evidence',
      module: 'checks.rs · followup.rs · evidence.rs',
      points: [
        'Did the last test or build pass, by its exit status and its output?',
        'Work that checked nothing is asked to check, or tested by Gate',
        'Signs of a weak pass: tests only of its own, a reply that disowns it',
      ],
    },
    {
      id: 'judge',
      name: 'Judge',
      does: 'Decide whose fault',
      module: 'turn_judge.rs',
      ranOn: 'Opus 5.5 for failed checks, Sonnet 5.5 for the rest',
      spend: 'gate:judge',
      points: [
        'When the work failed, checked nothing, or is due a review',
        'Model at fault: the work moves up. Excused: it stands',
      ],
    },
    {
      id: 'escalate',
      name: 'Escalate',
      does: 'Move up a tier',
      module: 'router::escalate',
      ranOn: 'Opus 5.5',
      spend: 'escalated',
      points: [
        'Same conversation, files as the work left them, with the findings',
        'More effort only on the strongest model, and only after a judged failure',
      ],
    },
  ],
  learn:
    'Every prompt and outcome is a transaction in the record. The table of what each model has proven per kind of work is a fold over it, updated before the next pick.',
  layers: [
    {
      name: 'Decide',
      crate: 'aura-agent-gate',
      holds: 'Pure functions: candidates, routing, the table, request builders, outcome rules. No I/O.',
    },
    {
      name: 'Run',
      crate: 'aura-app event loop',
      holds: 'Sends the classifier and judge calls through the kernel, runs checks, restores files.',
    },
    {
      name: 'Record',
      crate: 'kernel and record',
      holds: 'Every call, route and outcome is appended. Replaying it rebuilds the same table.',
    },
  ],
};
