# 🌞 Solar

The idea behing solar is to build IoT devices that speaks [Secure Scuttlebut](https://scuttlebutt.nz/), using the [Kuska](https://github.com/Kuska-ssb) rust libraries and mainly based on [async_std](https://async.rs/)

Current status is:

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
