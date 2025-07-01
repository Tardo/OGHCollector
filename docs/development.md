# Development

## Requirements

- Rust
- npm

## Start Server

```sh
cargo run --bin server
```

## Update Static Resources
```sh
npm run build
```

## Update Database
```sh
cargo run --bin collector -- OCA 15.0
```
** set OGHCOLLECTOR_GITHUB_TOKEN in the environment variables