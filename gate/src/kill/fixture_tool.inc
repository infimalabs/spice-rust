#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_LOG"

if [ "$1" = "mutants" ] && [ "$2" = "--version" ]; then
    if [ -f "$FAKE_BAD_VERSION" ]; then
        printf 'cargo-mutants 0.0.0\n'
    else
        printf 'cargo-mutants 27.1.0\n'
    fi
    exit 0
fi

if [ "$1" = "test" ]; then
    if [ -f baseline-fails ]; then
        exit 1
    fi
    if [ -f drift-after-baseline ]; then
        printf 'yes\n' > "$FAKE_DRIFT"
    fi
    exit 0
fi

for argument in "$@"; do
    if [ "$argument" = "--list" ]; then
        if [ -f "$FAKE_DRIFT" ]; then
            printf '[{"name":"Drifted"}]\n'
        else
            /bin/cat inventory.json
        fi
        exit 0
    fi
done

if [ "$SPICE_MUTATION_SEED" != "41" ] || [ -z "$SPICE_MUTATION_ATTEMPT" ]; then
    exit 72
fi

identity=
output_dir=
previous=
for argument in "$@"; do
    if [ "$previous" = "--re" ]; then
        identity=$argument
    fi
    if [ "$previous" = "--output" ]; then
        output_dir=$argument
    fi
    previous=$argument
done

write_outcome() {
    /bin/mkdir -p "$output_dir"
    printf '%s\n' "$1" > "$output_dir/outcomes.json"
}

printf 'mutated\n' >> src/lib.rs
case "$identity" in
    *Killed*)
        write_outcome '{"cargo_mutants_version":"27.1.0","total_mutants":1,"missed":0,"caught":1,"timeout":0,"unviable":0,"outcomes":[{"summary":"CaughtMutant"}]}'
        exit 0
        ;;
    *Survived*)
        write_outcome '{"cargo_mutants_version":"27.1.0","total_mutants":1,"missed":1,"caught":0,"timeout":0,"unviable":0,"outcomes":[{"summary":"MissedMutant"}]}'
        exit 2
        ;;
    *Flaky*)
        if [ "$SPICE_MUTATION_ATTEMPT" = "1" ]; then
            write_outcome '{"cargo_mutants_version":"27.1.0","total_mutants":1,"missed":0,"caught":1,"timeout":0,"unviable":0,"outcomes":[{"summary":"CaughtMutant"}]}'
            exit 0
        fi
        write_outcome '{"cargo_mutants_version":"27.1.0","total_mutants":1,"missed":1,"caught":0,"timeout":0,"unviable":0,"outcomes":[{"summary":"MissedMutant"}]}'
        exit 2
        ;;
    *Timeout*)
        write_outcome '{"cargo_mutants_version":"27.1.0","total_mutants":1,"missed":0,"caught":0,"timeout":1,"unviable":0,"outcomes":[{"summary":"Timeout"}]}'
        exit 3
        ;;
    *Incomplete*)
        write_outcome '{"cargo_mutants_version":"27.1.0","total_mutants":1,"missed":0,"caught":1,"timeout":0,"unviable":0,"outcomes":[{"summary":"CaughtMutant"}]}'
        printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'
        exit 0
        ;;
    *Unviable*)
        write_outcome '{"cargo_mutants_version":"27.1.0","total_mutants":1,"missed":0,"caught":0,"timeout":0,"unviable":1,"outcomes":[{"summary":"Unviable"}]}'
        exit 0
        ;;
    *NoOutcome*)
        exit 0
        ;;
    *RemoveTool*)
        /bin/mv "$FAKE_PROGRAM" "$FAKE_PROGRAM.removed"
        exit 70
        ;;
    *) exit 70 ;;
esac
