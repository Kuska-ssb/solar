# 🌞 Solar

![Rust](https://github.com/Kuska-ssb/solar/workflows/Rust/badge.svg)

The idea behing solar is to enable community hardware devices to speak [Secure Scuttlebutt](https://scuttlebutt.nz/) using the [Kuska](https://github.com/Kuska-ssb) Rust libraries, mainly based on [async_std](https://async.rs/).

Core features:

- [X] auto-create private key if not exists
- [X] broadcast the identity via lan discovery
- [X] automatic feed generation 
- [X] minimal [sled](https://github.com/spacejam/sled) database to store generate feeds
- [X] mux-rpc implementation
  - [X] `whoami`
  - [X] `get`
  - [X] `createHistoryStream` 
  - [X] `blobs createWants` & `blobs get` 
- [X] patchwork and cryptoscope interoperability

Extensions (undergoing active development):

- [X] json-rpc server over ws for user queries

JSON-RPC endpoints:

`ping` -> `pong!`
`whoami` -> `@2gAUKQzPiXZWeaC/eOyyIke83RAAKwNB/QxFWm66iJg=.ed25519`

_Note:_ [jsonrpc-cli](https://github.com/monomadic/jsonrpc-cli) is a Rust tool useful for testing JSON-RPC endpoints.
