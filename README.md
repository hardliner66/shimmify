# shimmify

`shimmify` is a small Rust library to add the capability to change the binary that is executed,
without having to replace the original. It stores shim paths and the active shim in a TOML configuration file,
and executes the shim by replacing the current process with the binary registered for the shim using `execvp`.

## Usage

Add `shimmify` as a dependency:

```toml
[dependencies]
shimmify = "1"
```

Call `shimmify` from your application's `main` function. It parses the
application's command line and handles the `daemon`, `exec`, and `shim`
commands before returning control to the application when no shim is active:

```rust
use shimmify::shimmify;

fn main() {
    shimmify("shimmify.toml");

    println!("Unshimmed!");
}
```

With the included example, commands look like this:

```bash
# Add a shim and make it active
cargo run --example simple -- shim add ls "$(which ls)" --use

# Add another shim
cargo run --example simple -- shim add cat "$(which cat)"

# Show configured shims
cargo run --example simple -- shim list

# Run the active shim, passing through arguments
cargo run --example simple -- exec -- -lah

# Switch the active shim
cargo run --example simple -- shim use cat

# Run the active shim, passing through arguments
cargo run --example simple -- exec -- --version

# Deactivate the current shim
cargo run --example simple -- shim reset

# Remove multiple shims
cargo run --example simple -- shim remove ls cat

# Or remove every configured shim
cargo run --example simple -- shim remove --all
```

The config file defaults to `shimmify.toml` in the caller's working directory.
Use `--config path/to/config.toml` or set `SHIMMIFY_CONFIG` to choose another
file.

## Configuration

The configuration is plain TOML and is managed by the library:

```toml
current = "nightly"

[shims]
nightly = "/opt/nightly/bin/tool"
stable = "/usr/local/bin/tool"
```

When no shim is active, `exec` leaves the process unchanged. When one is
active, `exec` replaces the wrapper process with that executable and forwards
the environment and command-line arguments.

## Development

Run the test suite and check the example with Cargo:

```bash
cargo test
cargo check --examples
```

The crate only supports Unix-like systems because it uses Unix process
replacement APIs. Restarting services additionally requires `systemctl` and
systemd.