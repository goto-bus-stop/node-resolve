# node-resolve

[![node-resolve on crates.io](https://img.shields.io/crates/v/node-resolve.svg)](https://crates.io/crates/node-resolve)

Rust implementation of the [Node.js module resolution algorithm](https://nodejs.org/api/modules.html#modules_all_together).

Missing features:

 - [ ] --preserve-symlinks (currently always preserves symlinks, unlike Node)
 - [ ] maybe more

## Install

Add to your Cargo.toml:

```toml
[dependencies]
node-resolve = "1.1.0"
```

## Usage

See [docs.rs/node-resolve](https://docs.rs/node-resolve).

## License

[Apache-2.0](./LICENSE.md)
