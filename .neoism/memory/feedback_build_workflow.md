---
name: Don't run cargo build --release autonomously
description: User has their own dev/build workflow; I should only verify with cargo check
type: feedback
originSessionId: a22dfe78-6308-4520-8808-47ad28a168f6
---
Do not run `cargo build -p neoism --release` (or any heavy/slow build) on my own initiative when working on neoism. The user already has a dev workflow running and treats my running release builds as wasteful and presumptuous.

**Why:** They handle their own rebuild + relaunch cycle. My job is to verify the code compiles, not to produce binaries.

**How to apply:** For neoism (and likely most user projects): use `cargo check -p neoism` to confirm compilation. Stop after that. If they want a binary they will produce one. Only run `cargo build` / `cargo run` when the user explicitly asks for it.
