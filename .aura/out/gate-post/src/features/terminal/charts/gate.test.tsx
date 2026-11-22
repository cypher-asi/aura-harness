import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { GATE_RESULTS } from './gate-data';
import { isChart, PostChart } from './PostChart';

const GATE_CHARTS = ['gate-architecture', 'gate-savings', 'gate-swe-tasks', 'gate-spend', 'gate-iterations'];

describe('Gate charts', () => {
  it.each(GATE_CHARTS)('%s is registered and renders a figure', (name) => {
    expect(isChart(name)).toBe(true);
    const { container } = render(<PostChart name={name} />);
    expect(container.querySelector('figure')).not.toBeNull();
  });

  it('draws the seven stages of a turn in order', () => {
    render(<PostChart name="gate-architecture" />);
    const stages = screen.getAllByRole('listitem').map((item) => item.textContent ?? '');
    const names = ['Classify', 'Pick', 'Criteria', 'Work', 'Verify', 'Judge', 'Escalate'];
    names.forEach((name, i) => expect(stages[i]).toContain(name));
  });

  it('shows the headline totals from the export', () => {
    render(<PostChart name="gate-savings" />);
    expect(screen.getByText(`$${GATE_RESULTS.headline.gate.cost.toFixed(2)}`)).toBeTruthy();
    expect(screen.getByText(`$${GATE_RESULTS.headline.opus.cost.toFixed(2)}`)).toBeTruthy();
  });

  it('has one row per finished task', () => {
    const { container } = render(<PostChart name="gate-swe-tasks" />);
    expect(container.querySelectorAll('li').length).toBe(GATE_RESULTS.tasks.length);
  });

  it('counts only finished pairs, and each spend adds up to the arm total', () => {
    const { headline, spend, tasks } = GATE_RESULTS;
    expect(tasks.length).toBe(headline.pairs);
    const total = Object.values(spend).reduce((sum, v) => sum + v, 0);
    expect(Math.abs(total - headline.gate.cost)).toBeLessThan(0.02);
  });
});

describe('the Gate post', () => {
  const read = () => readFileSync(join(process.cwd(), 'content/posts/gate-router-design.md'), 'utf8');

  it('names only charts this build can draw', () => {
    const post = read();
    const names = [...post.matchAll(/```chart\n([^\n]+)\n```/g)].map((m) => m[1].trim());
    expect(names.length).toBeGreaterThan(0);
    for (const name of names) expect(isChart(name), name).toBe(true);
  });

  it('cites only sources it lists, and lists each number once', () => {
    const post = read();
    const body = post.split(/\n## Sources\n/)[0];
    const sources = post.split(/\n## Sources\n/)[1] ?? '';
    const listed = [...sources.matchAll(/^(\d+)\. /gm)].map((m) => Number(m[1]));
    expect(listed).toEqual(listed.map((_, i) => i + 1));
    const cited = new Set([...body.matchAll(/\[(\d{1,3})\]\(https?:/g)].map((m) => Number(m[1])));
    for (const n of cited) expect(listed, `source ${n}`).toContain(n);
    for (const n of listed) expect(cited.has(n), `source ${n} is never cited`).toBe(true);
  });
});
