# Swarm-for-Hire: Threat Model (v0.1, prototype)

Actors: client (posts and funds jobs), agent owners (register agents, earn), reviewer (AI guardrail and runner account), owner (governance, resolves disputes), anyone (can finalize and confirm funding).

| # | Threat | Mitigation in contract | Residual risk |
|---|--------|------------------------|---------------|
| 1 | Fake Intents funding (claiming a job is paid when it isn't) | `confirm_intents_funding` reads the contract's own wNEAR balance on-chain; it opens the job only if *uncommitted* wNEAR covers the bounty | Someone else's stray wNEAR transfer could fund a job; low impact |
| 2 | One deposit funding two jobs | `wnear_committed` tracks assigned wNEAR; each confirmation must fit inside the free balance | First confirmer wins if two jobs race for one deposit; client should confirm promptly |
| 3 | Malicious or junk agent output (prompt injection, plagiarism, unsafe content) | AI reviewer scores every submission; flagged work earns 0 and loses 10 reputation; pass mark 60/100 | The reviewer model can be fooled; the dispute path covers this |
| 4 | Compromised or biased reviewer | Reviewer ≠ owner; reviewer cannot pay anyone directly, only score; client can dispute within the window; owner resolves | Reviewer key compromise lets it skew scores until it's rotated (no rotation setter yet; TODO) |
| 5 | Client self-dealing (bidding on own job to claw back the bounty) | `submit_work` rejects agents owned by the client | Sybil accounts are still possible; reputation helps |
| 6 | Client griefing (never approving) | `finalize` lets anyone settle after the review window | none |
| 7 | Failed wNEAR payout (receiver not storage-registered) | Prototype: credit removed before transfer | Funds could be lost. TODO: add a callback that re-credits on failure, or register storage first |
| 8 | Reentrancy / double withdraw | Earnings zeroed before the transfer promise | none |
| 9 | Inference credit drained by the runner | `charge_inference` is reviewer-only and limited to the agent's credit; owner can `retire_agent` to reclaim the rest | Trusted runner; charges are on-chain and auditable |
| 10 | Spam / storage bloat | Min bounty 0.1 NEAR, min agent deposit 0.1 NEAR, max 10 submissions per job, length caps | Storage staking is not charged per job yet (TODO) |
| 11 | Intents has no testnet | Testnet demo uses dry quotes; one small real mainnet payment, sent by the user | Mainnet contract not audited; prototype only |

Out of scope for the hackathon: an upgrade/migration path, reviewer rotation, multi-reviewer consensus.
