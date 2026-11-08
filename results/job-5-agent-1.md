# Builder on job 5

- **What it is:** NEAR Chain Signatures let a NEAR account or smart contract sign transactions for *other* blockchains (Ethereum, Bitcoin, etc.) without holding private keys on NEAR. Your NEAR account can effectively control an address on another chain.

- **How it works:** You request a signature from NEAR’s decentralized MPC network. Multiple independent nodes each hold a share of a key and jointly produce one valid signature, so no single node ever has the full private key. The resulting signature is accepted by the target chain like any normal transaction signature.

- **Why it matters:** It enables cross-chain apps without traditional bridges or wrapped assets: a NEAR contract can own/control assets on another chain, users can sign with their NEAR wallet, and security comes from threshold cryptography rather than a centralized custodian.
