'use client';

import {
  createContext,
  Fragment,
  useContext,
  useEffect,
  useId,
  useRef,
  useState,
  type CSSProperties,
  type FocusEvent,
  type KeyboardEvent,
  type PointerEvent,
  type ReactElement,
  type ReactNode,
} from 'react';
import {
  AGENT_KERNELS,
  B300_INDEX,
  COMPUTE_POSITION,
  CREDIT_REPRICING,
  GATE_FLOW,
  GATE_SOURCES,
  GPU_HOURS_PER_MW,
  PER_MW,
  ROADMAP,
  revenuePerMw,
  type Source,
} from './data';
import { GATE_RESULTS } from './gate-data';
import styles from './PostChart.module.css';

/*
 * Charts a post embeds with a fenced block in its Markdown:
 *
 *   ```chart
 *   compute-position
 *   ```
 *
 * Drawn in HTML and CSS rather than a scaled SVG, so the type stays at the
 * article's size from a phone to the full column. Every mark has a tooltip:
 * hover it with a mouse, tap it on a touch screen, or tab to it and (on the
 * line chart) move along it with the arrow keys.
 */

/** What a tooltip says: a title, label/value rows, and an optional line of context. */
interface Tip {
  title: string;
  rows: (readonly [string, string])[];
  note?: string;
  /** Points under a heading, e.g. what a stage has to prove. */
  list?: { heading: string; items: string[] };
}

/** Where a tooltip points, in client coordinates: a point on top of the mark, and the mark's bottom edge. */
interface Anchor {
  x: number;
  top: number;
  bottom: number;
}

interface Shown {
  key: string;
  x: number;
  y: number;
  below: boolean;
  side: 'start' | 'center' | 'end';
  tip: Tip;
}

interface Tips {
  /** The tooltip's element id, for the active mark's `aria-describedby`. */
  id: string;
  /** Key of the mark whose tooltip is showing, unique within its figure. */
  active: string | null;
  show: (key: string, anchor: Anchor, tip: Tip) => void;
  /** Hides the tooltip, or only the given mark's when a key is passed. */
  hide: (key?: string) => void;
}

const TipContext = createContext<Tips>({ id: '', active: null, show: () => {}, hide: () => {} });

/** Room a tooltip needs above its mark before it flips below it. */
const ROOM_ABOVE = 110;

function Figure({
  title,
  subtitle,
  sources,
  children,
}: {
  title: string;
  subtitle?: string;
  sources: Source[];
  children: ReactNode;
}): ReactElement {
  const figure = useRef<HTMLElement>(null);
  const [shown, setShown] = useState<Shown | null>(null);
  const id = useId();
  const open = shown !== null;

  // A tap outside the figure closes a tooltip opened by touch.
  useEffect(() => {
    if (!open) return;
    const onDown = (event: globalThis.PointerEvent) => {
      if (!figure.current?.contains(event.target as Node)) setShown(null);
    };
    document.addEventListener('pointerdown', onDown);
    return () => document.removeEventListener('pointerdown', onDown);
  }, [open]);

  const tips: Tips = {
    id,
    active: shown?.key ?? null,
    show(key, anchor, tip) {
      const box = figure.current?.getBoundingClientRect();
      if (!box) return;
      const x = anchor.x - box.left;
      const below = anchor.top - box.top < ROOM_ABOVE;
      const side = x > box.width * 0.62 ? 'end' : x < box.width * 0.38 ? 'start' : 'center';
      setShown({ key, x, y: (below ? anchor.bottom : anchor.top) - box.top, below, side, tip });
    },
    hide(key) {
      setShown((current) => (key === undefined || current?.key === key ? null : current));
    },
  };

  return (
    <TipContext.Provider value={tips}>
      <figure ref={figure} className={styles.figure}>
        <div className={styles.head}>
          <p className={styles.title}>{title}</p>
          {subtitle ? <p className={styles.subtitle}>{subtitle}</p> : null}
        </div>
        {children}
        {sources.length ? (
          <figcaption className={styles.sources}>
            Sources:{' '}
            {sources.map((source, i) => (
              <Fragment key={source.href}>
                {i ? '; ' : ''}
                <a href={source.href} target="_blank" rel="noreferrer noopener">
                  {source.label}
                </a>
              </Fragment>
            ))}
          </figcaption>
        ) : null}
        {shown ? (
          <div
            id={id}
            role="tooltip"
            className={styles.tip}
            data-side={shown.side}
            data-below={shown.below || undefined}
            style={{ left: shown.x, top: shown.y }}
          >
            <p className={styles.tipTitle}>{shown.tip.title}</p>
            {shown.tip.rows.map(([label, value]) => (
              <p key={label} className={styles.tipRow}>
                <span>{label}</span>
                <span>{value}</span>
              </p>
            ))}
            {shown.tip.note ? <p className={styles.tipNote}>{shown.tip.note}</p> : null}
            {shown.tip.list ? (
              <div className={styles.tipList}>
                <p className={styles.tipListHeading}>{shown.tip.list.heading}</p>
                <ul>
                  {shown.tip.list.items.map((item) => (
                    <li key={item}>{item}</li>
                  ))}
                </ul>
              </div>
            ) : null}
          </div>
        ) : null}
      </figure>
    </TipContext.Provider>
  );
}

/**
 * Props that give a mark its tooltip. The tooltip points at the mark's
 * `[data-anchor]` child: its end for "end", its middle otherwise.
 */
function useTipTarget(): (key: string, tip: Tip) => Record<string, unknown> {
  const tips = useContext(TipContext);
  return (key, tip) => {
    const show = (target: HTMLElement) => {
      const anchor = target.querySelector<HTMLElement>('[data-anchor]') ?? target;
      const box = anchor.getBoundingClientRect();
      const x = anchor.dataset.anchor === 'end' ? box.right : box.left + box.width / 2;
      tips.show(key, { x, top: box.top, bottom: box.bottom }, tip);
    };
    const active = tips.active === key;
    return {
      tabIndex: 0,
      'data-active': active || undefined,
      'aria-describedby': active ? tips.id : undefined,
      onPointerEnter: (event: PointerEvent<HTMLElement>) => {
        if (event.pointerType === 'mouse') show(event.currentTarget);
      },
      onPointerLeave: (event: PointerEvent<HTMLElement>) => {
        if (event.pointerType === 'mouse') tips.hide(key);
      },
      // Touch and pen have no hover, so a tap toggles the tooltip.
      onPointerUp: (event: PointerEvent<HTMLElement>) => {
        if (event.pointerType === 'mouse') return;
        if (active) tips.hide(key);
        else show(event.currentTarget);
      },
      onFocus: (event: FocusEvent<HTMLElement>) => {
        if (event.currentTarget.matches(':focus-visible')) show(event.currentTarget);
      },
      onBlur: () => tips.hide(key),
      onKeyDown: (event: KeyboardEvent<HTMLElement>) => {
        if (event.key === 'Escape') tips.hide(key);
      },
    };
  };
}

interface Bar {
  /** Keys the bar's tooltip when the label repeats within a figure. */
  id?: string;
  label: string;
  detail?: string;
  value: number;
  shown: string;
  accent?: boolean;
  tip: Tip;
}

/** Horizontal bars from zero to `max`, with an optional marker line across them. */
function Bars({
  rows,
  max,
  marker,
}: {
  rows: Bar[];
  max: number;
  marker?: { at: number; label: string };
}): ReactElement {
  const { active } = useContext(TipContext);
  const target = useTipTarget();
  const pct = (n: number) => `${Math.min(Math.max(n / max, 0), 1) * 100}%`;
  return (
    <ul
      className={styles.bars}
      data-has-active={rows.some((row) => (row.id ?? row.label) === active) || undefined}
      style={marker ? ({ '--marker': pct(marker.at) } as CSSProperties) : undefined}
    >
      {rows.map((row) => (
        <li key={row.id ?? row.label} className={styles.row} {...target(row.id ?? row.label, row.tip)}>
          <span className={styles.rowLabel}>
            {row.label}
            {row.detail ? <span className={styles.rowDetail}>{row.detail}</span> : null}
          </span>
          <span className={styles.track} data-marker={marker ? '' : undefined}>
            <span
              className={styles.bar}
              data-anchor="end"
              data-accent={row.accent || undefined}
              style={{ width: pct(row.value) }}
            />
          </span>
          <span className={styles.rowValue}>{row.shown}</span>
        </li>
      ))}
      {marker ? (
        <li className={styles.markerRow} aria-hidden="true">
          <span />
          <span className={styles.markerLabel}>{marker.label}</span>
          <span />
        </li>
      ) : null}
    </ul>
  );
}

const percent = (n: number) => `${Math.round(n * 100)}%`;
/** Billions of USD, e.g. "$38.7B" or "$1.40T". */
const usd = (billions: number) =>
  billions >= 1000 ? `$${(billions / 1000).toFixed(2)}T` : `$${+billions.toFixed(1)}B`;
const signed = (n: number, text: string) => `${n > 0 ? '+' : n < 0 ? '−' : ''}${text}`;

function ComputePosition(): ReactElement {
  const rows = (side: 'long' | 'short'): Bar[] =>
    COMPUTE_POSITION[side].map((c) => ({
      label: c.name,
      detail: c.note,
      value: c.contracted / c.value,
      shown: percent(c.contracted / c.value),
      accent: side === 'long',
      tip: {
        title: `${c.name}: ${side === 'long' ? 'bought forward' : 'sold forward'}`,
        rows: [
          [side === 'long' ? 'Compute committed' : 'Contracts sold', usd(c.contracted)],
          [side === 'long' ? 'Valuation' : 'Market cap', usd(c.value)],
          ['Share of value', percent(c.contracted / c.value)],
        ],
        note:
          side === 'long'
            ? 'Keeps whatever the contracted hours turn out to be worth.'
            : 'Has sold the hours at a fixed rate and carries the GPUs and debt.',
      },
    }));
  return (
    <Figure
      title="Compute contracted forward, as a share of what each company is worth"
      subtitle="The labs have bought compute forward; the neoclouds have sold it forward at fixed rates."
      sources={COMPUTE_POSITION.sources}
    >
      <p className={styles.group}>Bought forward: long compute</p>
      <Bars rows={rows('long')} max={5} />
      <p className={styles.group}>Sold forward: short compute</p>
      <Bars rows={rows('short')} max={5} />
    </Figure>
  );
}

function CreditRepricing(): ReactElement {
  return (
    <Figure
      title="The debt behind the short side is repricing"
      subtitle="Cents below par. Investment-grade paper from the long side still prices at par."
      sources={CREDIT_REPRICING.sources}
    >
      <Bars
        max={20}
        rows={CREDIT_REPRICING.bonds.map((bond) => {
          const rows: [string, string][] = [
            ['Price', `${bond.below ? 'under ' : ''}${+bond.price.toFixed(3)}c`],
            ['Below par', `${bond.below ? 'over ' : ''}${+(100 - bond.price).toFixed(2)}c`],
          ];
          if (bond.yieldPct) rows.push(['Yield', `${bond.yieldPct.toFixed(2)}%`]);
          return {
            label: bond.name,
            detail: bond.yieldPct ? `${bond.detail}, ${bond.yieldPct.toFixed(2)}% yield` : bond.detail,
            value: 100 - bond.price,
            shown: bond.price >= 99.99 ? 'par' : `${bond.below ? 'under ' : ''}${+bond.price.toFixed(1)}c`,
            accent: bond.name.startsWith('CoreWeave'),
            tip: { title: bond.name, rows, note: bond.detail },
          };
        })}
      />
    </Figure>
  );
}

const monthDay = new Intl.DateTimeFormat('en-US', { month: 'short', day: 'numeric', timeZone: 'UTC' });
const fullDate = new Intl.DateTimeFormat('en-US', {
  weekday: 'short',
  month: 'short',
  day: 'numeric',
  year: 'numeric',
  timeZone: 'UTC',
});

function B300Index(): ReactElement {
  const readings = B300_INDEX.readings;
  const change = readings[readings.length - 1][1] / readings[0][1] - 1;
  const contract = PER_MW.rates[0].rate;
  return (
    <Figure
      title="Meanwhile, the asset keeps getting more valuable"
      subtitle={`B300 rental price, USD per GPU-hour: up ${percent(change)} in a month, against the $${contract.toFixed(2)} a long-term contract locks in.`}
      sources={B300_INDEX.sources}
    >
      <B300Plot />
    </Figure>
  );
}

/** The B300 line and its axis, inside its Figure so it reaches that figure's tooltip. */
function B300Plot(): ReactElement {
  const tips = useContext(TipContext);
  const plot = useRef<HTMLDivElement>(null);
  const readings = B300_INDEX.readings;
  const contract = PER_MW.rates[0].rate;
  const [lo, hi] = [4.5, 8];
  const W = 600;
  const H = 240;
  const x = (i: number) => (i / (readings.length - 1)) * W;
  const y = (v: number) => H - ((v - lo) / (hi - lo)) * H;
  const top = (v: number) => `${((hi - v) / (hi - lo)) * 100}%`;
  const left = (i: number) => `${(i / (readings.length - 1)) * 100}%`;
  const points = readings.map(([, v], i) => `${x(i).toFixed(1)},${y(v).toFixed(1)}`).join(' ');
  const area = `M0,${H} L${points.replaceAll(' ', ' L')} L${W},${H} Z`;
  const [first, last] = [readings[0], readings[readings.length - 1]];
  const date = (iso: string) => monthDay.format(new Date(`${iso}T00:00:00Z`));
  const indexOf = (iso: string) => readings.findIndex(([day]) => day === iso);
  const steps = B300_INDEX.steps.map(({ from, to }) => {
    const [a, b] = [indexOf(from), indexOf(to)];
    return { a, b, delta: readings[b][1] - readings[a][1] };
  });
  const weekly = readings.map((_, i) => i).filter((i) => i % 7 === 0 && readings.length - 1 - i >= 4);

  const active = tips.active === null ? null : Number(tips.active);

  function showAt(i: number) {
    const box = plot.current?.getBoundingClientRect();
    if (!box) return;
    const [day, price] = readings[i];
    const money = (n: number) => signed(n, `$${Math.abs(n).toFixed(2)}`);
    const pct = (n: number) => signed(n, `${Math.abs(n * 100).toFixed(1)}%`);
    const rows: [string, string][] = [['Price', `$${price.toFixed(2)} / GPU-hr`]];
    if (i > 0) rows.push(['Day change', money(price - readings[i - 1][1])]);
    if (i >= 7) rows.push(['7-day change', pct(price / readings[i - 7][1] - 1)]);
    if (i > 0) rows.push([`Since ${date(first[0])}`, pct(price / first[1] - 1)]);
    rows.push([`Over $${contract.toFixed(2)} contract`, `${money(price - contract)} (${pct(price / contract - 1)})`]);
    rows.push(['Revenue per MW a year', `$${revenuePerMw(price).toFixed(1)}M`]);
    const step = steps.find((s) => i > s.a && i <= s.b);
    const pointY = box.top + ((hi - price) / (hi - lo)) * box.height;
    tips.show(
      String(i),
      { x: box.left + (i / (readings.length - 1)) * box.width, top: pointY, bottom: pointY },
      {
        title: fullDate.format(new Date(`${day}T00:00:00Z`)),
        rows,
        note: step
          ? `Step up: ${money(step.delta)} from ${date(readings[step.a][0])} to ${date(readings[step.b][0])}.`
          : `At ${GPU_HOURS_PER_MW.toLocaleString('en-US')} GPU-hours a year per MW, fully utilized.`,
      },
    );
  }

  function indexAt(clientX: number): number | null {
    const box = plot.current?.getBoundingClientRect();
    if (!box || !box.width) return null;
    const ratio = Math.min(Math.max((clientX - box.left) / box.width, 0), 1);
    return Math.round(ratio * (readings.length - 1));
  }

  function onPointer(event: PointerEvent<HTMLDivElement>) {
    // Touch shows the reading under a tap; mouse and pen follow the pointer.
    if (event.type === 'pointermove' && event.pointerType === 'touch') return;
    const i = indexAt(event.clientX);
    if (i !== null && i !== active) showAt(i);
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const current = active ?? readings.length - 1;
    const next =
      event.key === 'ArrowLeft'
        ? Math.max(current - 1, 0)
        : event.key === 'ArrowRight'
          ? Math.min(current + 1, readings.length - 1)
          : event.key === 'Home'
            ? 0
            : event.key === 'End'
              ? readings.length - 1
              : null;
    if (event.key === 'Escape') tips.hide();
    if (next === null) return;
    event.preventDefault();
    showAt(next);
  }

  return (
    <>
      <div
        ref={plot}
        className={styles.plot}
        data-tall=""
        tabIndex={0}
        role="img"
        aria-label={`B300 rental index rose from $${first[1].toFixed(2)} on ${date(first[0])} to $${last[1].toFixed(2)} on ${date(last[0])}, against a $${contract.toFixed(2)} contract rate. Use the arrow keys to read each day.`}
        aria-describedby={active === null ? undefined : tips.id}
        data-active={active === null ? undefined : ''}
        onPointerMove={onPointer}
        onPointerDown={onPointer}
        onPointerLeave={(event) => {
          if (event.pointerType === 'mouse') tips.hide();
        }}
        onFocus={(event) => {
          if (event.currentTarget.matches(':focus-visible')) showAt(active ?? readings.length - 1);
        }}
        onBlur={() => tips.hide()}
        onKeyDown={onKeyDown}
      >
        {steps.map((s) => (
          <span
            key={s.a}
            className={styles.stepBand}
            style={{ left: left(s.a), width: `calc(${left(s.b)} - ${left(s.a)})` }}
          >
            <span className={styles.stepBandLabel}>+${s.delta.toFixed(2)}</span>
          </span>
        ))}
        {[5.5, 6, 6.5, 7, 7.5].map((v) => (
          <span key={v} className={styles.gridLine} style={{ top: top(v) }}>
            <span className={styles.gridLabel}>${v.toFixed(2)}</span>
          </span>
        ))}
        <span className={styles.gridLine} data-reference="" style={{ top: top(contract) }}>
          <span className={styles.gridLabel}>${contract.toFixed(2)}</span>
          <span className={styles.referenceLabel}>Long-term contract rate</span>
        </span>
        {active !== null ? <span className={styles.cursor} style={{ left: left(active) }} /> : null}
        <svg className={styles.line} viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" aria-hidden="true">
          <path className={styles.area} d={area} />
          <polyline points={points} />
        </svg>
        {readings.map(([day, v], i) => (
          <span key={day} className={styles.dayDot} style={{ left: left(i), top: top(v) }} />
        ))}
        <span className={styles.point} style={{ left: '0%', top: top(first[1]) }} />
        <span className={styles.point} data-accent="" style={{ left: '100%', top: top(last[1]) }} />
        {active !== null ? (
          <span className={styles.point} data-hover="" style={{ left: left(active), top: top(readings[active][1]) }} />
        ) : null}
        <span className={styles.pointLabel} style={{ left: '0%', top: top(first[1]) }}>
          ${first[1].toFixed(2)}
        </span>
        <span className={styles.pointLabel} data-end="" style={{ left: '100%', top: top(last[1]) }}>
          ${last[1].toFixed(2)}
        </span>
      </div>
      <p className={styles.axis} data-ticks="">
        {weekly.map((i) => (
          <span key={i} style={{ left: left(i) }}>
            {date(readings[i][0])}
          </span>
        ))}
        <span style={{ left: '100%' }}>{date(last[0])}</span>
      </p>
    </>
  );
}

/** One MW's revenue a year at each way of selling its GPU-hours, against what it costs and is worth. */
function RevenuePerMw(): ReactElement {
  const { cost, value } = PER_MW;
  const max = 80;
  const spot = B300_INDEX.readings[B300_INDEX.readings.length - 1][1];
  const rows: Bar[] = PER_MW.rates.map((r, i) => {
    const revenue = revenuePerMw(r.rate);
    return {
      label: r.name,
      detail: `$${r.rate.toFixed(2)} per GPU-hour · ${r.detail}`,
      value: revenue,
      shown: `$${revenue.toFixed(1)}M`,
      accent: i === PER_MW.rates.length - 1,
      tip: {
        title: `${r.name}: one MW for a year`,
        rows: [
          ['Rate', `$${r.rate.toFixed(2)} / GPU-hr`],
          ['Revenue a year', `$${revenue.toFixed(1)}M`],
          ['Against $41.2M cost', `${percent(revenue / cost.total)} a year`],
          ['Revenue pays back cost in', `${((cost.total / revenue) * 12).toFixed(0)} months`],
          ["Vs today's spot", `${signed(r.rate - spot, `$${Math.abs(r.rate - spot).toFixed(2)}`)} / GPU-hr`],
        ],
        note: `${PER_MW.gpus} B300 GPUs in ${PER_MW.nodes} nodes, fully utilized, ${GPU_HOURS_PER_MW.toLocaleString('en-US')} GPU-hours a year. Revenue before operating costs and debt service.`,
      },
    };
  });
  rows.push({
    label: 'Cost to build',
    detail: 'GPUs, data center and powered land',
    value: cost.total,
    shown: `$${cost.total.toFixed(1)}M`,
    tip: {
      title: 'What one MW costs to build',
      rows: [
        ['GPUs', `$${cost.gpus.toFixed(1)}M`],
        ['Data center conversion', `$${cost.building.toFixed(1)}M`],
        ['Land and power', `$${cost.land.toFixed(1)}M`],
        ['Total', `$${cost.total.toFixed(1)}M`],
      ],
      note: 'Half equity, half debt secured on the assets. GPUs at $550K per node before tax and setup.',
    },
  });
  rows.push({
    label: 'Market value',
    detail: 'Enterprise value per MW of listed neoclouds',
    value: value[1],
    shown: `$${value[0]}–${value[1]}M`,
    tip: {
      title: 'What the market pays for one MW',
      rows: [
        ['Low', `$${value[0]}M`],
        ['High', `$${value[1]}M`],
        ['Over cost to build', `${(value[0] / cost.total).toFixed(1)}–${(value[1] / cost.total).toFixed(1)}×`],
      ],
      note: 'CoreWeave, Nebius and IREN enterprise value per MW, October 13, 2026.',
    },
  });
  return (
    <Figure
      title="What one megawatt earns, depending on how its compute is sold"
      subtitle="Revenue a year from one MW of critical IT, against its cost to build and what the market values it at. USD."
      sources={[
        { label: 'ZODE estimates', href: 'https://zode.org/deck' },
        { label: 'Silicon Data B300 index', href: B300_INDEX.sources[0].href },
        { label: 'NeoCloud Stocks, Oct 13, 2026', href: 'https://www.neocloudstocks.com/' },
      ]}
    >
      <p className={styles.group}>Revenue a year</p>
      <Bars rows={rows.slice(0, 3)} max={max} marker={{ at: cost.total, label: 'Cost to build' }} />
      <p className={styles.group}>Cost and value</p>
      <Bars rows={rows.slice(3)} max={max} />
      <p className={styles.footnote}>Preliminary, fully utilized. Targets, not results.</p>
    </Figure>
  );
}

function AgentKernels(): ReactElement {
  return (
    <Figure
      title="Kernels written by AI agents, against the best human or vendor baseline"
      subtitle="Speed relative to the baseline each was measured against."
      sources={AGENT_KERNELS.sources}
    >
      <Bars
        max={2}
        marker={{ at: 1, label: 'Baseline' }}
        rows={AGENT_KERNELS.results.map((r, i) => ({
          label: r.name,
          detail: r.detail,
          value: r.speedup,
          shown: `${r.speedup.toFixed(2)}×`,
          accent: i === 0,
          tip: {
            title: r.name,
            rows: [
              ['Speed vs baseline', `${r.speedup.toFixed(2)}×`],
              ['Faster by', `${Math.round((r.speedup - 1) * 100)}%`],
            ],
            note: r.detail,
          },
        }))}
      />
    </Figure>
  );
}

function Roadmap(): ReactElement {
  return (
    <Figure
      title="What we have to prove, in megawatts"
      subtitle="Each stage funds and de-risks the next. Log scale."
      sources={[]}
    >
      <RoadmapSteps />
    </Figure>
  );
}

/** The roadmap's stages, inside its Figure so they reach that figure's tooltip. */
function RoadmapSteps(): ReactElement {
  const { active } = useContext(TipContext);
  const target = useTipTarget();
  const height = (mw: number) => `${((Math.log10(mw) + 1) / (Math.log10(1000) + 1)) * 100}%`;
  return (
    <ol className={styles.steps} data-has-active={active !== null || undefined}>
      {ROADMAP.map((step, i) => {
        const letter = String.fromCharCode(97 + i);
        const [low, high] = [revenuePerMw(PER_MW.rates[0].rate), revenuePerMw(PER_MW.rates[2].rate)];
        const scaled = (n: number) =>
          n * step.mw >= 1000 ? `$${((n * step.mw) / 1000).toFixed(1)}B` : `$${(n * step.mw).toFixed(1)}M`;
        const rows: [string, string][] = [['Capacity', step.label]];
        if (i > 0)
          rows.push(['Step up', `${+(step.mw / ROADMAP[i - 1].mw).toFixed(1)}× stage ${String.fromCharCode(96 + i)}`]);
        rows.push(['Revenue a year', `${scaled(low)}–${scaled(high)}`]);
        rows.push(['Cost to build', scaled(PER_MW.cost.total)]);
        rows.push(['Market value', `${scaled(PER_MW.value[0])}–${scaled(PER_MW.value[1])}`]);
        return (
          <li
            key={step.label}
            className={styles.step}
            {...target(step.label, {
              title: `${letter}. ${step.stage}`,
              rows,
              note: `${step.detail}. Revenue from all contracted to all inference, at the per-MW figures above.`,
            })}
          >
            <span className={styles.stepCol}>
              <span
                className={styles.stepBar}
                data-anchor="center"
                data-accent={i === 0 || undefined}
                style={{ height: height(step.mw) }}
              >
                <span className={styles.stepValue}>{step.label}</span>
              </span>
            </span>
            <span className={styles.stepStage}>
              {letter}. {step.stage}
            </span>
            <span className={styles.stepDetail}>{step.detail}</span>
          </li>
        );
      })}
    </ol>
  );
}

/* Gate: the router behind Aura's `gate` model id. Data in data.ts (GATE_FLOW) and gate-data.ts. */

const gateUsd = (n: number) => (n < 0.1 ? `$${n.toFixed(3)}` : `$${n.toFixed(2)}`);
const gateRatio = (n: number) => `${n.toFixed(2)}×`;
const minutes = (n: number) => `${Math.round(n)} min`;
const HEAD = GATE_RESULTS.headline;
const OPUS_ARM = 'Opus 5.5 alone';

/** The stages of a turn left to right, the learning loop under them, then the three layers. */
function GateArchitecture(): ReactElement {
  return (
    <Figure
      title="How Gate handles one turn"
      subtitle="Seven stages, a loop that learns from every outcome, and three layers that keep the decisions pure and replayable."
      sources={[]}
    >
      <GateFlow />
    </Figure>
  );
}

function GateFlow(): ReactElement {
  const { active } = useContext(TipContext);
  const target = useTipTarget();
  const { stages, learn, layers } = GATE_FLOW;
  return (
    <div className={styles.flow} data-has-active={active !== null || undefined}>
      <ol className={styles.stages}>
        {stages.map((stage, i) => {
          const rows: [string, string][] = [['Module', stage.module]];
          if (stage.ranOn) rows.push([`Ran on in #${HEAD.compare}`, stage.ranOn]);
          if (stage.spend) {
            rows.push([`Calls in #${HEAD.compare}`, String(GATE_RESULTS.calls[stage.spend])]);
            rows.push([`Cost in #${HEAD.compare}`, gateUsd(GATE_RESULTS.spend[stage.spend])]);
          }
          return (
            <li
              key={stage.id}
              className={styles.stage}
              data-accent={stage.id === 'verify' || undefined}
              {...target(stage.id, {
                title: `${i + 1}. ${stage.name}: ${stage.does}`,
                rows,
                list: { heading: 'What it does', items: stage.points },
              })}
            >
              <span className={styles.stageNo} data-anchor="center">
                {String(i + 1).padStart(2, '0')}
              </span>
              <span className={styles.stageName}>{stage.name}</span>
              <span className={styles.stageDoes}>{stage.does}</span>
            </li>
          );
        })}
      </ol>
      <div
        className={styles.loop}
        {...target('learn', {
          title: 'Learn: the table is a fold over the record',
          rows: [
            ['Module', 'stats.rs · gate_live.rs'],
            ['Proven at', '10 tries, 85% success'],
            ['Ruled out', 'once 85% is out of reach'],
            ['Exploration', '5% of routed turns'],
          ],
          note: learn,
        })}
      >
        <span className={styles.loopArrow} data-anchor="center" aria-hidden="true">
          ↰
        </span>
        <span className={styles.loopLabel}>Learn</span>
        <span className={styles.loopText}>Each outcome updates the table the next pick reads</span>
      </div>
      <p className={styles.group}>Layers</p>
      <ul className={styles.layers}>
        {layers.map((layer) => (
          <li
            key={layer.name}
            className={styles.layer}
            {...target(layer.name, { title: `${layer.name}: ${layer.crate}`, rows: [], note: layer.holds })}
          >
            <span className={styles.layerName} data-anchor="center">
              {layer.name}
            </span>
            <span className={styles.layerCrate}>{layer.crate}</span>
            <span className={styles.layerHolds}>{layer.holds}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function GateSavings(): ReactElement {
  const { gate, opus } = HEAD;
  const saved = 1 - gate.cost / opus.cost;
  const second = GATE_RESULTS.second;
  const arm = (
    group: string,
    gateValue: number,
    opusValue: number,
    shown: (n: number) => string,
    rows: (isGate: boolean) => [string, string][],
  ): Bar[] => [
    {
      id: `${group}-opus`,
      label: OPUS_ARM,
      value: opusValue,
      shown: shown(opusValue),
      tip: { title: `${OPUS_ARM}: ${group.toLowerCase()}`, rows: rows(false) },
    },
    {
      id: `${group}-gate`,
      label: 'Gate',
      detail: group === 'Cost' ? `${Math.round(saved * 100)}% less` : undefined,
      value: gateValue,
      shown: shown(gateValue),
      accent: true,
      tip: { title: `Gate: ${group.toLowerCase()}`, rows: rows(true) },
    },
  ];
  return (
    <Figure
      title={`Gate against Opus 5.5 alone on ${HEAD.pairs} SWE-Bench Pro hard tasks`}
      subtitle={`Compare #${HEAD.compare}: each task once on each arm, graded by the task's hidden tests. List-price USD, Gate's own calls included.`}
      sources={GATE_SOURCES}
    >
      <p className={styles.group}>Cost</p>
      <Bars
        max={Math.ceil(opus.cost)}
        rows={arm('Cost', gate.cost, opus.cost, (n) => `$${n.toFixed(2)}`, (isGate) => [
          ['Total', `$${(isGate ? gate : opus).cost.toFixed(2)}`],
          ['Per task', `$${((isGate ? gate : opus).cost / HEAD.pairs).toFixed(2)}`],
          ['Gate ÷ Opus', gateRatio(gate.cost / opus.cost)],
        ])}
      />
      <p className={styles.group}>Wall time</p>
      <Bars
        max={Math.ceil(opus.wallMin / 10) * 10}
        rows={arm('Wall time', gate.wallMin, opus.wallMin, minutes, (isGate) => [
          ['Total', minutes((isGate ? gate : opus).wallMin)],
          ['Per task', minutes((isGate ? gate : opus).wallMin / HEAD.pairs)],
        ])}
      />
      <p className={styles.group}>Hidden tests passed</p>
      <Bars
        max={HEAD.pairs}
        rows={arm('Passed', gate.passed, opus.passed, (n) => `${n} of ${HEAD.pairs}`, (isGate) => [
          ['Passed', `${(isGate ? gate : opus).passed} of ${HEAD.pairs}`],
        ])}
      />
      <p className={styles.footnote}>
        Compare #{second.compare}, an earlier run of the same design on {second.pairs} of these tasks: Gate $
        {second.gate.cost.toFixed(2)}, Opus ${second.opus.cost.toFixed(2)}, {second.gate.passed} of {second.pairs}{' '}
        passed on each.
      </p>
    </Figure>
  );
}

/** Per task, Gate's cost and Opus's cost as two bars on one row. */
function GateSweTasks(): ReactElement {
  const max = Math.max(...GATE_RESULTS.tasks.flatMap((t) => [t.gate.cost, t.opus.cost]));
  const repos = [...new Set(GATE_RESULTS.tasks.map((t) => t.repo))];
  const allPassed = GATE_RESULTS.tasks.every((t) => t.gate.passed && t.opus.passed);
  return (
    <Figure
      title="Cost of each task, Gate against Opus 5.5 alone"
      subtitle={`Compare #${HEAD.compare}, SWE-Bench Pro hard. ${
        allPassed ? "Every run on both arms passed the task's hidden tests." : 'Graded by each task’s hidden tests.'
      }`}
      sources={GATE_SOURCES}
    >
      <p className={styles.legend} aria-hidden="true">
        <span className={styles.swatch} data-accent="" /> Gate
        <span className={styles.swatch} /> {OPUS_ARM}
      </p>
      {repos.map((repo) => (
        <Fragment key={repo}>
          <p className={styles.group}>{repo}</p>
          <PairedBars max={max} tasks={GATE_RESULTS.tasks.filter((t) => t.repo === repo)} />
        </Fragment>
      ))}
    </Figure>
  );
}

function PairedBars({
  tasks,
  max,
}: {
  tasks: readonly (typeof GATE_RESULTS.tasks)[number][];
  max: number;
}): ReactElement {
  const { active } = useContext(TipContext);
  const target = useTipTarget();
  const pct = (n: number) => `${Math.min(n / max, 1) * 100}%`;
  return (
    <ul className={styles.bars} data-has-active={tasks.some((t) => t.task === active) || undefined}>
      {tasks.map((t) => {
        const moved = t.gate.route.length > 1;
        return (
          <li
            key={t.task}
            className={styles.row}
            {...target(t.task, {
              title: t.task,
              rows: [
                ['Gate', `$${t.gate.cost.toFixed(2)} · ${minutes(t.gate.wallMin)}`],
                [OPUS_ARM, `$${t.opus.cost.toFixed(2)} · ${minutes(t.opus.wallMin)}`],
                ['Gate ÷ Opus', gateRatio(t.gate.cost / t.opus.cost)],
                ['Gate ran on', t.gate.route.join(', then ')],
                ["Gate's own calls", gateUsd(t.gate.ownCalls)],
                ['Hidden tests', t.gate.passed && t.opus.passed ? 'both passed' : 'see run'],
              ],
              note: t.title,
            })}
          >
            <span className={styles.rowLabel}>
              {t.task.slice(t.repo.length + 1)}
              <span className={styles.rowDetail}>
                {moved ? 'Sonnet, then Opus' : (t.category ?? '').replace(/_/g, ' ')}
              </span>
            </span>
            <span className={styles.pair}>
              <span className={styles.track}>
                <span className={styles.bar} data-anchor="end" data-accent="" style={{ width: pct(t.gate.cost) }} />
              </span>
              <span className={styles.track}>
                <span className={styles.bar} style={{ width: pct(t.opus.cost) }} />
              </span>
            </span>
            <span className={styles.rowValue}>
              ${t.gate.cost.toFixed(2)}
              <span className={styles.rowValueSub}>${t.opus.cost.toFixed(2)}</span>
            </span>
          </li>
        );
      })}
    </ul>
  );
}

function GateSpend(): ReactElement {
  const { spend, calls } = GATE_RESULTS;
  const own = spend['gate:classify'] + spend['gate:criteria'] + spend['gate:judge'];
  const total = HEAD.gate.cost;
  const moved = GATE_RESULTS.tasks.filter((t) => t.gate.route.length > 1).length;
  const row = (
    key: keyof typeof spend,
    label: string,
    detail: string,
    note: string,
    accent = false,
  ): Bar => ({
    label,
    detail: `${calls[key]} calls · ${detail}`,
    value: spend[key],
    shown: gateUsd(spend[key]),
    accent,
    tip: {
      title: label,
      rows: [
        ['Cost', gateUsd(spend[key])],
        ['Calls', String(calls[key])],
        ['Share of Gate', `${((spend[key] / total) * 100).toFixed(1)}%`],
      ],
      note,
    },
  });
  return (
    <Figure
      title={`Where Gate's $${total.toFixed(2)} went`}
      subtitle={`Compare #${HEAD.compare}, ${HEAD.pairs} tasks, priced per call from recorded token usage.`}
      sources={GATE_SOURCES}
    >
      <p className={styles.group}>The work</p>
      <Bars
        max={Math.ceil(spend.work)}
        rows={[
          row('work', 'Work on Sonnet 5.5', 'every task', 'The agent loop on the model Gate picked, at high effort.', true),
          row(
            'escalated',
            'Moved up to Opus 5.5',
            `${moved} ${moved === 1 ? 'task' : 'tasks'}`,
            'Work the judge found unfinished, taken over and finished on Opus.',
          ),
        ]}
      />
      <p className={styles.group}>Gate&apos;s own calls</p>
      <Bars
        max={Math.ceil(spend.work)}
        rows={[
          row('gate:judge', 'Judge and review', 'after the work', 'Deciding whether failed or unchecked work was the model’s fault, and reviewing work about to stand.'),
          row('gate:criteria', 'Acceptance criteria', 'before the work', 'What done means for each code turn, written on Sonnet.'),
          row('gate:classify', 'Classifier', 'one per prompt', 'Labelling each prompt on Haiku 5.5.'),
        ]}
      />
      <p className={styles.footnote}>
        Gate&apos;s own calls cost {gateUsd(own)}, {Math.round((own / total) * 100)}% of its total. Opus alone spent $
        {HEAD.opus.cost.toFixed(2)} on the same tasks.
      </p>
    </Figure>
  );
}

function GateIterations(): ReactElement {
  const runs = GATE_RESULTS.iterations;
  const last = runs[runs.length - 1];
  return (
    <Figure
      title="Gate's cost against Opus alone, run by run"
      subtitle={`The same ${GATE_RESULTS.core.length} SWE-Bench Pro tasks in each Compare, from the first design to the current one. Below 1× Gate costs less.`}
      sources={GATE_SOURCES}
    >
      <Bars
        max={Math.ceil(Math.max(...runs.map((r) => r.ratio)))}
        marker={{ at: 1, label: OPUS_ARM }}
        rows={runs.map((r) => ({
          label: `#${r.compare}`,
          detail: `${r.at} · passed ${r.gate.passed} of ${r.gate.runs} (Opus ${r.opus.passed})`,
          value: r.ratio,
          shown: gateRatio(r.ratio),
          accent: r === last,
          tip: {
            title: `Compare #${r.compare}, ${r.at}`,
            rows: [
              ['Gate', `$${r.gate.cost.toFixed(2)} · ${r.gate.passed} of ${r.gate.runs} passed`],
              [OPUS_ARM, `$${r.opus.cost.toFixed(2)} · ${r.opus.passed} of ${r.opus.runs} passed`],
              ['Gate ÷ Opus', gateRatio(r.ratio)],
            ],
            note: r.found,
            list: { heading: 'Changed before this run', items: [r.before] },
          },
        }))}
      />
      <p className={styles.footnote}>
        Tasks: {GATE_RESULTS.core.join(', ')}. Runs before #33 predate the logging of Gate&apos;s own calls, so their
        Gate cost leaves out the classifier and judge.
      </p>
    </Figure>
  );
}

const CHARTS: Record<string, () => ReactElement> = {
  'compute-position': ComputePosition,
  'credit-repricing': CreditRepricing,
  'b300-index': B300Index,
  'revenue-per-mw': RevenuePerMw,
  'agent-kernels': AgentKernels,
  roadmap: Roadmap,
  'gate-architecture': GateArchitecture,
  'gate-savings': GateSavings,
  'gate-swe-tasks': GateSweTasks,
  'gate-spend': GateSpend,
  'gate-iterations': GateIterations,
};

/** The chart a post names, or nothing for a name this build doesn't know. */
export const isChart = (name: string): boolean => Object.hasOwn(CHARTS, name);

export function PostChart({ name }: { name: string }): ReactElement | null {
  const Chart = Object.hasOwn(CHARTS, name) ? CHARTS[name] : undefined;
  return Chart ? <Chart /> : null;
}
