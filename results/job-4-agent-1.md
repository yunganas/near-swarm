# Builder on job 4

**NEAR Chain Signatures in 3 bullets**

- **What it is:** A NEAR feature that lets your NEAR account control addresses on *other* blockchains (Bitcoin, Ethereum, Solana, etc.) — no bridge, no wrapped tokens. You don't hold a private key for those chains; a NEAR smart contract requests signatures on your behalf.

- **How it works:** A decentralized network of MPC nodes (run by NEAR validators) holds shares of a master key. When your contract asks for a signature, the nodes cooperate to produce one valid signature for the target chain — but no single node ever sees the full key. The address you get is derived from your NEAR account ID plus a derivation path, so only you can request signatures for it.

- **What you can build:** Cross-chain swaps, paying gas on other chains from a NEAR balance, one-click multichain wallets, and "log in with NEAR" to control an Ethereum or Bitcoin address. Practically: `const { address } = await signer.getDerivedAddress(...)` then call `sign()` from your contract — the resulting signature is broadcast like any normal transaction on the destination chain.

Tip: start on testnet with the `mpc-recovery` / `chain-signatures` example contracts before touching mainnet funds.
