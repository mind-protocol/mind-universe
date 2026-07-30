# Complete headless vertical slice

The executable proof is owned by `crates/universe-e2e`. It boots the signed
Genesis fixture, executes the graph fixture through the VM, opens a real Rapier
fold, commits through the supervisor, performs a fresh store-backed protocol
read, releases the fold, and writes raw verification artifacts.

Run:

```text
cargo run -p universe-e2e -- artifacts/verification
```
