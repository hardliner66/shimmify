#!/usr/bin/env bash

pushd "$(dirname "${BASH_SOURCE[0]}")/.." &> /dev/null || exit 1

cargo build --example simple --release &> /dev/null

SHIMMIFY_CONFIG="$(pwd)/target/shimmify.toml"
export SHIMMIFY_CONFIG

PATH="$(pwd)/target/release/examples:$PATH"
export PATH

print_command() {
    echo "+ $@"
}

dbg() {
    description="$1"
    shift
    use_tail=false
    if [[ "$1" == "tail" ]]; then
        use_tail=true
        shift
    fi
    use_head=false
    if [[ "$1" == "head" ]]; then
        use_head=true
        shift
    fi
    echo ""
    echo "==========================================="

    echo "# $description:"
    echo "> $@"
    output="$("$@" 2>&1)"
    echo "------------------ OUTPUT -----------------"

    if [[ "$use_tail" == true ]]; then
        output="$(echo "$output" | tail -n +1 -)"
    elif [[ "$use_head" == true ]]; then
        output="$(echo "$output" | head -n 1)"
    fi
    echo "$output" | awk '{print "| " $0}'
    echo "==========================================="
}

dbg "Add a shim and make it active" tail \
    simple shim add ls "$(which ls)" --use

dbg "Add another shim" \
    simple shim add cat "$(which cat)"

dbg "List all configured shims" \
    simple shim list

dbg "Run the active shim" head \
    simple shim exec -- --version

dbg "Switch the active shim to cat" \
    simple shim use cat

dbg "List all shims" \
    simple shim list

dbg "Run the newly active shim (cat) with specific arguments" head \
    simple shim exec -- --version

dbg "Deactivate the current shim" \
    simple shim reset

dbg "List all shims" \
    simple shim list

dbg "Run the unshimmed binary" \
    simple shim exec

dbg "Remove all shims" \
    simple shim remove --all

dbg "List all shims" \
    simple shim list

popd &> /dev/null || exit 1