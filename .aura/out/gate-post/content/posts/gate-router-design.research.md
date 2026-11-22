# Research notes: Gate router post

Each source below was fetched on Nov 22, 2026, either through the arXiv API or as a page that returned HTTP 200. Its title, authors and date were checked. The last column says what the post may cite it for.

| Source | Reference | Checked | What the post cites it for |
|---|---|---|---|
| FrugalGPT | Chen, Zaharia, Zou, arXiv 2305.05176, May 2023 | abstract | LLM cascade; matches GPT-4 with up to 98% cost reduction |
| MoT cascades | Yue et al., arXiv 2310.03094, ICLR 2024 | abstract | weak-model answer consistency as the deferral signal; about 40% of the strong model's cost |
| Token-level deferral | Gupta et al., arXiv 2404.10136, Apr 2024 | abstract | deferral rules for generative cascades; sequence uncertainty has a length bias |
| AutoMix | Aggarwal et al., arXiv 2310.12963, Oct 2023 | abstract | few-shot self-verification plus a POMDP router; >50% cost reduction |
| Hybrid LLM | Ding et al., arXiv 2404.14618, Apr 2024 | abstract | router that predicts difficulty; up to 40% fewer large-model calls |
| RouteLLM | Ong et al., arXiv 2406.18665, Jun 2024 | abstract | routers trained on preference data; >2x cost reduction |
| RouteLLM blog | LMSYS, Jul 1, 2024 | page text | >85% cost reduction on MT Bench, 45% on MMLU, 35% on GSM8K, at 95% of GPT-4 |
| RouterBench | Hu et al. (Martian), arXiv 2403.12031, Mar 2024 | abstract | 405k inference outcomes; benchmark of one-shot prompts |
| Routing survey | Varangot-Reille et al., arXiv 2502.00409, Feb 2025 | abstract | survey of routing strategies |
| Router-R1 | Zhang, Feng, You, arXiv 2506.09033, Jun 2025 | abstract | LLM router trained with RL; multi-round routing and aggregation |
| Arch-Router | Tran et al., arXiv 2506.16655, Jun 2025 | abstract | routing aligned with preferences, to domain and action policies the user defines |
| MasRouter | Yue et al., arXiv 2502.11133, Feb 2025 | abstract | routing for multi-agent systems; up to 52% lower overhead on HumanEval |
| TwinRouterBench | Yang et al., arXiv 2605.18859, May 2026 | abstract | agentic routing per step on SWE-bench; downgrade-and-cascade protocol; existing router benchmarks are one-shot |
| Triage | Madeyski, arXiv 2604.07494, Apr 2026 | abstract | light, standard and heavy tiers (Haiku, Sonnet, Opus) behind a verification gate, on SWE-bench Lite |
| ProgRouter | Li, Abdelmoniem, Wang, arXiv 2608.25992, Aug 2026 | abstract | existing cascade routers make one-shot decisions per query; online routing per step |
| Online Pandora's Box | Belloni, Chen, Wei, arXiv 2606.07392, Jun 2026 | abstract | online contextual cascading; UCB-style learning; Õ(√T) regret |
| Cheapest feasible router | Hu et al., arXiv 2611.20417, Nov 2026 | abstract | the cheapest router that passes the audit violates its held-out contract in 32.22% of deployments |
| LLM-as-a-judge | Zheng et al., arXiv 2306.05685, Jun 2023 | abstract | strong LLMs used as judges |
| Reflexion | Shinn et al., arXiv 2303.11366, Mar 2023 | abstract | retrying with verbal feedback |
| Contextual bandits | Li, Chu, Langford, Schapire, arXiv 1003.0146, 2010 | abstract | exploration with logged propensities; offline replay evaluation |
| SWE-Bench Pro | Deng et al., arXiv 2509.16941, Sep 2025 | abstract | 1,865 enterprise-level, long-horizon problems |
| SWE-Bench Pro repo | scaleapi/SWE-bench_Pro-os, GitHub | README | V2 (Sep 22): 642 tasks, the HARD-51 subset, re-grading in a fresh sandbox |
| GPT-5 | OpenAI, "Introducing GPT-5" | page text | "a real-time router that quickly decides which to use based on conversation type, complexity, tool needs, and your explicit intent", trained on real signals |
| Building effective agents | Anthropic | page text | routing workflow: "classifies an input and directs it to a specialized followup task" |
| OpenRouter Auto Code | OpenRouter docs | page text | a fast model rates each turn; the policy weighs pass rate, speed and cost per solved task; it changes effort before switching model, and switches on failure signals |
| OpenRouter routing benchmarks | openrouter.ai/benchmarks/routers | page text | public router benchmark; last run Nov 21, 2026 |
| Not Diamond | notdiamond.ai | page text | commercial router for coding agents; claims 50%+ cost savings |
| Claude pricing | platform.claude.com/docs/en/about-claude/pricing | page text | per MTok, input/output: Opus 5.5 $4/$20, Sonnet 5.5 $2/$10, Haiku 5.5 $0.10 input (up to 100k-token prompts)/$0.50 output; cache reads $0.20 (Opus) and $0.10 (Sonnet) |

Dropped: Martian's homepage. It is now a research lab page with no routing product to cite, and RouterBench covers Martian's work.
