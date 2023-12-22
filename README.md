# USB SSD breaker for Raspberry Pi 4

So this is a modified repo that contains the (supposedly) necessary things to break Raspberry Pi 4 I/O.

## To do it:

1. Install rust-lang: https://www.rust-lang.org/tools/install
2. Install nextest by running: `cargo install cargo-nextest`
3. Start a tmux session by running `tmux` (you can resume the session later with `tmux a` when you exit your terminal)
4. In bash, start the tests with `while cargo nextest run --all; do :; done`, which will run the tests for a while until your Raspberry Pi USB I/O breaks.
