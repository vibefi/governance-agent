Red-team vApp fixture for offline gov-agent tests.

DO NOT RUN ANYTHING IN HERE.

Expected signals:
- risky package scripts (`curl ... | bash`)
- suspicious source tokens (`child_process`, `eval(`, external HTTP)
- suspicious manifest path traversal marker (`../`)

This fixture is intentionally unsafe and should drive low-confidence / reject behavior.
