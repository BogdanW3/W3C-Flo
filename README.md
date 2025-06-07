# Overview

FLO is a Warcraft III toolkit written in Rust:

- Warcraft III server implementation with reconnect, artificial delay, live streaming, and many other features
- Containerized-micro services for easy-to-use server-side API and rapid nodes deployment
- Cross-platform client-side application to run internet games through LAN emulation
- Libraries, for example, W3GS protocol parsing, map file parsing, replay parsing/generating, and LAN game creation

## Setup Local Development Environment

NOTE: Full Ubuntu Installation guide is available in [INSTALL.md](INSTALL.md)

### Install Prerequisites

- C++ Compiler
- CMake
- Latest Stable Rust

### Init submodules

```
git submodule update --init --recursive
```

### Environment Variable Setup Before Cargo Build

```
BONJOUR_SDK_HOME = <path_to_flo>/deps/bonjour-sdk-windows
PQ_LIB_DIR = <path_to_PostgreSQL>/lib
```

### Create a .env file:
```
RUST_LOG=debug
DATABASE_URL=postgres://postgres:postgres@localhost/flo
FLO_CONTROLLER_SECRET='1111'
FLO_NODE_SECRET='1111'
JWT_SECRET_BASE64=MTExMQ==
```

### Run migration to create db schema

```
diesel migration run
```

### Add an API client
Insert a row into `api_client` table with secret_key = `1111` (Corresponds to the FLO_NODE_SECRET value in above .env file)

```shell
psql -d flo -U postgres
```

```
flo=# insert into api_client (name, secret_key) values ('testclient', '1111');
```

### Add Players
Insert 2 rows into `player` table with `source` = `0`, `source_id` = unique values (e.g 1, 2), and `api_client_id` equal to the id of the row you created in `api_client`.

```
flo=# insert into player (name, source, source_id, api_client_id) values ('player1', '0', '1', '1');
```

```
flo=# insert into player (name, source, source_id, api_client_id) values ('player2', '0', '2', '1');
```

### Add Node
Insert a row into `node` with `secret` = `1111` (Corresponds to the FLO_NODE_SECRET value in above .env file).

**Note:** If you are in an environment (like docker) where the visible IP (the client connects to) is not routable for the controller, then insert the client-routable IP as `ip_addr` while setting the internal address in the `internal_address` field. Note that `internal_address` might also be a DNS address, while `ip_addr` **has to** be an IPv4 address.

```
flo=# insert into node (name, location, ip_addr, secret, internal_address) values ('node1', 'Germany', '192.168.0.5', '1111', null);
```

### Start Node & Controller
```
cargo run -p flo-node-service
cargo run -p flo-controller-service
```

### Start 2 Clients
For player 1
```
cargo run -p flo-cli -- client 1 connect
```

For player 2
```
cargo run -p flo-cli -- client 2 connect
```

### Create a test game and invite player 1 & 2 to join

```
cargo run -p flo-cli -- server run-game 1 2
```

### Join the game using LOCAL AREA NETWORK

You will see 2 games correspond to the 2 players

Open 2 Warcraft III to join both games and the game will start.

## Credits

- @nielsAD -- [GoWarcraft3](https://github.com/nielsAD/gowarcraft3)
- Fingon -- Help in game mechanics and algorithms
- @Josko -- [Aura Bot](https://github.com/Josko/aura-bot)
- Varlock -- the author of the GHost++ bot
- @Miezhiko -- initial Linux support
- JSamir/tofik-mamisho -- [wc3-replay-parser](https://github.com/JSamir/wc3-replay-parser)
- PBug90 -- [w3gjs](hhttps://github.com/PBug90/w3gjs)

