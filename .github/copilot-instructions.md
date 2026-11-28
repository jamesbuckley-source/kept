# Working on Kept

## Parallel agents

Run **at most three** delegated agents or sessions at a time. Wait for one to report
before starting the next, unless the user explicitly asks for more. Running more than
that saturates the machine — Rust release builds and Vite builds go from seconds to
minutes, which makes every agent slower and the feedback loop useless.

## Feature tracker

`docs/features.json` is the board behind the Kept Feature Tracker canvas
(`copypaste-feature-tracker`, actions `list_features` / `add_feature` /
`update_feature`).

Move a card to **in-progress** *before* any code, docs or tests for it are started,
and leave it there while the work is being written, tested and committed. Move it to
**shipped** only once it is tested and shipped (committed on the release branch,
`shippedIn` naming the release or `next`). A card in backlog or planned means no work
has started on it; do not start work on a card without moving it first.
