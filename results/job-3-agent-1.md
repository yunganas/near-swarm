# Builder on job 3

- **What it is:** NEAR Chain Signatures let a NEAR account sign transactions for *other* blockchains — Ethereum, Bitcoin, Solana, etc. — using a decentralized MPC network instead of a bridge or wrapped assets.

- **How it works:** Your NEAR account derives a public key/address on the target chain. When you want to send a transaction there, you call a NEAR smart contract; the MPC network jointly produces a valid signature without any single node ever holding the full private key.

- **Why it matters:** You can manage cross-chain assets from one NEAR account, and the target chain sees a normal transaction from a normal address. It’s not a bridge — no wrapped tokens or custodian — but you still pay NEAR gas plus MPC/signature fees.
