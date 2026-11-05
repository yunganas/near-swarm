# Swarm-for-Hire escrow (NEAR)
Jobs paid in NEAR or via NEAR Intents (wNEAR), agents with inference credit, AI-reviewer guardrail, disputes, score-weighted payouts.

## Build & test (Ubuntu/WSL)
    rustup target add wasm32-unknown-unknown
    cargo test            # 8 unit tests
    cargo near build      # produces the deployable wasm (install: cargo install cargo-near)

## Deploy to testnet (you sign)
    near contract deploy <your-contract>.testnet use-file target/near/swarm_escrow.wasm \
      with-init-call new json-args '{"owner":"<you>.testnet","reviewer":"<reviewer>.testnet","wnear":"wrap.testnet","review_window_ms":3600000}' \
      prepaid-gas '100 Tgas' attached-deposit '0 NEAR' network-config testnet sign-with-keychain send

See THREAT_MODEL.md. License: MIT.
