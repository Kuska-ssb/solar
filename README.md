# 🌞 Solar

Solar was written by [@adria0](https://github.com/adria0) with the idea to 
enable community hardware devices to speak [Secure Scuttlebutt](https://scuttlebutt.nz/)
using the [Kuska](https://github.com/Kuska-ssb) Rust libraries, mainly based on 
[async_std](https://async.rs/).

This fork aims to evolve solar into a minimal Scuttlebutt node capable of 
lightweight replication and feed storage. Much like 
[scuttlego](https://github.com/planetary-social/scuttlego), this fork is not
intended to reproduce the full suite of MUXRPC methods used in the JS SSB
ecosystem. It will only implement the core MUXRPC methods required for 
message publishing and replication. Indexing of database messages will be
offloaded to client applications (ie. piping feeds from solar into a SQLite
database).

## Quick Start

Clone the source and build the binary:

```
git clone git@github.com:mycognosist/solar.git
cd solar
cargo build --release
```

Add friend(s) to `solar.toml` (public key(s) of feeds you wish to replicate):

```
vim ~/.local/share/solar/solar.toml

id = "@...=.ed25519"
secret "...==.ed25519"
friends = ["@...=.ed25519"]
```

Run solar with LAN discovery enabled:

```
./target/release/solar --lan true
```

## Core Features

- [X] auto-create private key if not exists
- [X] broadcast the identity via lan discovery
- [X] automatic feed generation
- [X] minimal [sled](https://github.com/spacejam/sled) database to store generate feeds
- [X] mux-rpc implementation
  - [X] `whoami`
  - [X] `get`
  - [X] `createHistoryStream`
  - [X] `blobs createWants`
  - [X] `blobs get`
  - [X] [patchwork](https://github.com/ssbc/patchwork) and [go-ssb](https://github.com/ssbc/go-ssb) interoperability
- [X] legacy replication (using `createHistoryStream`)

## Extensions

_Undergoing active development._

- [X] json-rpc server for user queries
  - [X] ping
  - [X] whoami
  - [ ] ...
- [ ] improved connection handler
- [ ] ebt replication

## Documentation

_Undergoing active development._

- [ ] comprehensive doc comments
- [ ] comprehensive code comments

## JSON-RPC API

The server currently supports Websocket connections. This may soon change to 
HTTP.

| Method | Parameters | Response | Description |
| --- | --- | --- | --- |
| `ping` | | `pong!` | Responds if the JSON-RPC server is running |
| `whoami` | | `<@...=.ed25519>` | Returns the public key of the local node |

-----

_Note:_ [jsonrpc-cli](https://github.com/monomadic/jsonrpc-cli) is a Rust tool 
useful for testing JSON-RPC endpoints.

## License

AGPL-3.0
