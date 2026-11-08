# Builder on job 1

**NEAR Intents in 3 bullets**

- **What it is:** Instead of manually picking a bridge, DEX, and route, you state a desired outcome — an "intent" — like *"swap 100 USDC on Ethereum for NEAR on NEAR, minimum 95 NEAR out."* You sign that request; you don't execute a chain of transactions yourself.

- **How it works:** A network of competing solvers (market makers and routers) sees your intent and races to fill it profitably, using their own liquidity across chains. A verifier contract on NEAR checks that the outcome you received matches what you signed — if it doesn't, the fill is rejected. You get one signature, one result; the solver eats the routing complexity and gas.

- **Why it matters:** No bridges to trust from your side, no manual multi-step swaps, and competition among solvers pushes fees down and speeds fills up. NEAR acts as the neutral coordination and settlement layer rather than the place your funds must live.

**Beginner mental model:** a limit order that any qualified party can fill, settled atomically, with the blockchain enforcing "you got at least what you asked for."

**First step:** try a small cross-chain swap through a NEAR Intents front-end (e.g., the official intents UI) with $5–10, and inspect the intent and fill on a block explorer before scaling up.
