This folder contains vapp bundles which should result in low scores. 


DO NOT RUN ANYTHING IN HERE.


## malicious_uniswapv2

Content taken from commit 762c7a9bbe861aa7723ddd9f8b6e41b611674546 `vibefi/dapp-examples/uniswap-v2`.

It contains only one malicious rogue send in App.tsx#L316. Instead of swapping tokens, it just sends the amount to a random address. 

## red_team_vapp

Obviously malicious app.

Expected signals:
- risky package scripts (`curl ... | bash`)
- suspicious source tokens (`child_process`, `eval(`, external HTTP)
- suspicious manifest path traversal marker (`../`)

This fixture is intentionally unsafe and should drive low-confidence / reject behavior.
