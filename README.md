# btimeline

## Verification

| Check                    | Command              | Toolchain                                                                                        |
| ------------------------ | -------------------- | ------------------------------------------------------------------------------------------------ |
| Cast start pairing proof | `cargo verus verify` | [Verus 0.2026.09.27.3cf1832](https://github.com/verus-lang/verus/releases), Rust 1.98.1 (stable) |
| Rust tests               | `cargo test`         | Stable Rust                                                                                      |

Runtime contracts for P4 grouping and wipe handling use [contracts](https://github.com/x52dev/contracts).

## Multi-pull alignment (P4)

`btimeline align FILE FILE [FILE ...]` prints pairwise boss-anchor segments, ordered signal slots, repeated-anchor candidates, divergent paths, and wipe-censored suffixes as JSON. See [P4 alignment results](docs/p4-alignment.md).
