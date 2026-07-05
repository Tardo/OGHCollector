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
cargo run --bin collector -- OCA 15.0
```
** set `OGHCOLLECTOR_TOKEN_GH` (or `OGHCOLLECTOR_TOKEN_GL` for GitLab) in the environment variables
