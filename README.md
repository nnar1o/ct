# ct

`ct` is a terminal command wrapper focused on compact logs and AI-friendly output.

## Install with curl

```sh
curl -fsSL https://raw.githubusercontent.com/nnar1o/ct/main/install.sh | sh
```

By default binaries are installed to `~/.local/bin`.

Supported platforms for the curl installer:

- Linux `x86_64`
- macOS `arm64`

Optional environment variables:

- `CT_VERSION` - install a specific version (example: `v0.1.0`)
- `CT_REPO` - use a different GitHub repository (example: `owner/repo`)
- `CT_INSTALL_DIR` - custom binary install directory

After installation, run:

```sh
ct-install
```

to enable shell integration for `ct cd ...`.

## Development

```sh
cargo test
cargo build --release --bins
```
