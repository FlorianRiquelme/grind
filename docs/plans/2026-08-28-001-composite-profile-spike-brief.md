# P0 spike brief — composite agent profiles (#185)

This document is the Anchor artifact for one throwaway Job that proves a
two-backend Run end to end. The work it asks for is deliberately tiny; the
point of the Run is the machinery, not the diff.

## What the Run must do

1. Execute the stage ladder with the repo binding `opus-plan` (omp workhorse,
   claude-code strong): Plan rides the foreign claude-code route; workhorse
   stages ride omp.
2. Land one commit that appends the Run's own observations to
   `docs/findings/0008-composite-profile-spike.md` (create it): which backends
   executed which stages as the Run experienced them, and whether the
   evidence tree stayed coherent across both adapters.
3. Nothing else. No code changes.

## What the human watches (not the Run's job)

- the dispatch banner's route lines,
- `stages[].backend` in run.json,
- transcript/cost coherence per adapter,
- resume behavior when a stage dies mid-flight.
