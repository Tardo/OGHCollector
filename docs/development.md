# Development

## Requirements

- Rust
- npm
- Python (with devel)

## Rust Components

- rustfmt: ```rustup component add rustfmt```
- clippy: ```rustup component add clippy```

## Install NPM Project

```
npm install
```

## Update Static Resources
```sh
npm run build
```

## Start Server

```sh
cargo run --bin server
```

## Update Database
```sh
cargo run --bin collector -- OCA 15.0   # one source/version
cargo run --bin collector               # every source/version in ./collector.yaml
```
** set `OGHCOLLECTOR_TOKEN_GH` (or `OGHCOLLECTOR_TOKEN_GL` for GitLab) in the environment variables
