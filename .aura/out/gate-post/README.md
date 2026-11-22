# Gate post: staged files for cypher-website-vercel

The website repo is outside this session's writable sandbox, so its files are staged here with the paths they have in the site. They were type-checked and tested in a scratch copy of the site. Run `sh apply.sh` to copy them over.

| Staged file | Goes to | What it is |
|---|---|---|
| `content/posts/gate-router-design.md` | same path | The post. Front matter as in the research fixture, with `[n](url)` citations and a `## Sources` list (28 sources, which the site's `extractSources` parses). |
| `content/posts/gate-router-design.research.md` | same path | The checked sources and what each one is cited for |
| `src/features/terminal/charts/gate-data.ts` | same path | Generated benchmark data. Do not edit by hand. |
| `src/features/terminal/charts/data.ts` | same path | Adds `GATE_FLOW` and `GATE_SOURCES` |
| `src/features/terminal/charts/PostChart.tsx` / `.module.css` | same path | Five figures: `gate-architecture`, `gate-savings`, `gate-swe-tasks`, `gate-spend` and `gate-iterations`. Tooltips now render `tip.list`, which `Roadmap` already set but nothing drew. `Bar.id` keys a bar's tooltip when two bars share a label. |
| `src/features/terminal/charts/gate.test.tsx` | same path | Checks that the charts render. Also checks that every `chart` block in the post is registered, and that citations and the Sources list match. |

`*.orig` are the site's files as they were before these changes, kept for diffing.

**Already in the site.** Another agent copied an earlier version of these files into `cypher-website-vercel` at 02:31–02:33. It then made two fixes of its own in `PostChart.tsx`/`.module.css`: container queries for the narrow reader panel, and taking `list` back off the Roadmap tooltips. Both fixes are merged into the staged copies here. The site's copy still has the 16-task numbers and lacks the accuracy fixes, so re-run `apply.sh` (or copy the post, `gate-data.ts`, `data.ts` and `PostChart.tsx`) once that agent has finished in those files.

## Refreshing the numbers

Compare #37 was still running more tasks when the snapshot in `data/` was taken (see `data/SNAPSHOT_AT`). Only pairs where both arms had finished are counted. To refresh:

```sh
cd ~/code/aura-harness
python3 scripts/compare_summary.py --compare 28-37 --suite swe_bench_pro_hard > .aura/out/gate-post/data/compares.json
python3 .aura/out/gate-post/build_data.py .aura/out/gate-post/data/compares.json \
  > .aura/out/gate-post/src/features/terminal/charts/gate-data.ts
```

The figures read everything from `gate-data.ts`. The prose quotes the 16-task numbers (for example $6.54 against $13.23), so update the post if the counts change.

## Publishing

Posts live in the database, not in files. Create a draft through `POST /api/v1/sites/{site}/posts`, then `PATCH` it with the front matter fields and the body. The Aura company has no `siteId` in `src/lib/site.ts` yet, so a site has to be created first (`npm run site:create -- aura AURA https://aura.ai`), or the post published under another site.
