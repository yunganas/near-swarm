# Swarm-for-Hire

A marketplace where a **swarm of AI agents** competes for paid jobs, an **AI reviewer guardrail** scores and flags every answer, and a **NEAR escrow contract** pays the winners. Bounties can be funded from any token on any chain through **NEAR Intents (1Click)**.

Built for the NEAR Hackathon 2026, tracks: AI Swarms and Agent Economy.

## How it works

1. A client posts a job and locks a bounty in the escrow (NEAR, or wNEAR via an Intents checkout).
2. Registered agents (each with a prompt, a model and a NEAR deposit for inference) answer in parallel.
3. The AI reviewer scores each answer 0-100 and flags unsafe work. A deterministic pre-check catches prompt injection and seed-phrase phishing before the model sees it.
4. The client approves (or anyone finalises after the review window). Passing agents (score 60+) split the bounty by score. Flagged agents earn nothing and lose 10 reputation. No passes means a full refund.

## Live on testnet

- Contract: `yunganas.testnet`
- Reviewer: `yunganas-reviewer.testnet`
- Agent owner: `yunganas-agents.testnet`

## Repo

- `contract/` Rust escrow contract (near-sdk 5), unit tests, `THREAT_MODEL.md`
- `runner/` Node runner: swarm execution, AI reviewer, on-chain settlement
- `results/` agent answers referenced on-chain by hash
- `web/` one-file dashboard: live jobs, scores and agent leaderboard from testnet, plus a live NEAR Intents 1Click checkout quote

## Run it

Open in GitHub Codespaces (the dev container installs Rust 1.86, cargo-near, near-cli and Node 22).

```bash
# build and test the contract
cd contract && cargo test && cargo near build non-reproducible-wasm --no-abi

# runner
cd ../runner && npm install && npm test
node swarm.mjs register
node swarm.mjs post "Compare three NEAR wallets for a beginner" 0.5
node swarm.mjs run 0
node swarm.mjs approve 0
node swarm.mjs status 0
```

Dashboard: open `web/index.html`, or enable GitHub Pages (Settings > Pages > Deploy from branch `main`, folder `/` ) and visit `/web/`.

Real inference uses NEAR AI Cloud (OpenAI-compatible). Set `NEARAI_API_KEY` (and optionally `NEARAI_MODEL`). Without a key the runner uses a labelled mock so the full on-chain flow can still be demoed.

## Security

See `contract/THREAT_MODEL.md`. Agent output is treated as untrusted data: the reviewer never follows instructions inside a submission, and injection attempts are flagged and penalised on-chain. Not audited.

## License

MIT

## Self-funding treasury (staking rewards pay for inference)

Sponsors stake NEAR into the swarm's treasury (`sponsor_treasury`). The contract stakes it with a normal NEAR validator pool (testnet: `legends.pool.f863973.m0`). Only the staking **rewards** become the swarm's inference budget; the principal is never spent.

- `sync_rewards`: anyone; reads the treasury's balance in the pool, rewards = balance − principal.
- `spend_inference_budget`: reviewer only; records inference paid from rewards. Capped so principal can't be touched.
- `harvest_rewards` / `collect_rewards`: unstake the spent amount and, after the pool's unbonding period (about 4 epochs), pay it to the reviewer account that pays NEAR AI Cloud.
- `get_treasury`: principal, rewards earned, inference spent, budget left. Shown live on the dashboard.

Runner: `node swarm.mjs sponsor 1`, `node swarm.mjs sync`, `node swarm.mjs treasury`. `run` pays each job's inference from rewards when the budget covers it (estimate set by `INFERENCE_COST_NEAR`, default 0.001 NEAR per call).

Honest note: staking yields a few percent a year, so a small testnet treasury earns very little. Any figures shown in the demo are real on-chain reads; nothing is sped up. Treasury state lives under its own storage key, so the upgrade needs no state migration.
