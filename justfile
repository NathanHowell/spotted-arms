set shell := ["zsh", "-cu"]

# List recipes
default:
    @just --list

# Model-check one TLA+ scenario, tla/<cfg>.cfg (non-zero exit on invariant violation)
tlc cfg="kill_basic":
    tla/run.sh {{cfg}}

# Run all TLA+ scenarios (most are expected to fail: they are known-bug counterexamples)
tlc-all:
    #!/usr/bin/env zsh
    for f in tla/*.cfg; do
        name=${f:t:r}
        echo "=== $name"
        tla/run.sh $name 2>&1 | grep -E "Invariant .* violated|Model checking completed" || true
    done
