set -e
! grep -rq calc_total --exclude-dir=.gate_checks .
grep -q 'def compute_total' src/billing.py
grep -q compute_total README.md
cd src
python3 -c "from report import summary_line; assert summary_line('a', [('x', 10.0)]) == 'a: 12.00'"
[ "$(python3 cli.py x=1.5 y=2)" = "3.5" ]
