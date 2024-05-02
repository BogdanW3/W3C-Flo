Complete Ubuntu Installation Guide (Tested on Ubuntu Server 24.04)
-------------------

### Update packages

```shell
sudo apt update
sudo apt upgrade
```

### Clone the repository

```shell
git clone --recursive https://github.com/w3champions/flo.git
```

### Install Rust

```shell
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.bashrc
```

### Create .env file in flo code root

```shell
cd flo
nano .env
```

```ini
RUST_LOG=debug
DATABASE_URL=postgres://postgres:postgres@localhost/flo
FLO_NODE_SECRET=1111
JWT_SECRET_BASE64=dGVzdHRlc3R0ZXN0dGVzdHRlc3R0ZXN0dGVzdHRlc3R0ZXN0dGVzdHRlc3Q=
```

### Install CMake

```shell
sudo apt install cmake
```

### Install Diesel CLI

If you're having trouble installing Diesel, read more here: https://diesel.rs/guides/getting-started

By default Diesel CLI depends on libraries for PostgreSQL, Mysql and SQLite. Since we only need PostgreSQL support, we install the PostgreSQL dependencies.

```shell
sudo apt install postgresql libpq-dev
```

Install Diesel CLI

```shell
cargo install diesel_cli --no-default-features --features postgres
```

### Postgres configuration

Set the postgres db user password

```
sudo -u postgres psql
postgres=# alter user postgres password 'postgres';
```

In `/etc/postgresql/<version>/main/pg_hba.conf`, Change ```local  all  postgres  peer``` to ```local  all  postgres  md5```

Restart postgres

```shell
sudo systemctl restart postgresql
```

Create database schema using diesel

```shell
diesel setup
```

### Insert db data

Insert a row into the `api_client` table with `secret_key` = `1111` (Corresponds to the value in the .env file)

```shell
psql -U postgres -d flo -c "insert into api_client (name, secret_key) VALUES ('testclient', '1111')"
```

Insert a row into the `node` table with `secret` = `1111` (Corresponds to the value in the .env file)
(NOTE: use the server IP and not 127.0.0.1)

```shell
psql -U postgres -d flo -c "insert into node (name, location, secret, ip_addr) VALUES ('testnode', 'Germany', '1111', '127.0.0.1')"
```

### Build flo-node-service

Before building flo-node-service, make sure the `pkg-config` package is installed. It is needed for the system to find OpenSSL which is used when compiling `openssl-sys`.

```shell
sudo apt install pkg-config
```

```shell
cargo build -p flo-node-service --release
```

### Build flo-controller-service

```shell
cargo build -p flo-controller-service --release
```

### Run flo-node-service

Set the `FLO_NODE_SECRET` environment variable

```shell
export FLO_NODE_SECRET='1111'
```

Run flo-node-service

```shell
./target/release/flo-node-service
```

### Run flo-controller-service

```shell
./target/release/flo-controller-service
```

Running as a service
------------------

Create the following service files for systemd (NOTE: Change WorkingDirectory to where you cloned the repository)

 - /usr/lib/systemd/system/flo-node.service

```service
[Unit]
Description=Flo Node Service
After=network.target
After=postgresql.target

[Service]
Type=simple
WorkingDirectory=/home/<YOUR_USERNAME>/flo
ExecStart=/bin/bash -l -c "FLO_NODE_SECRET='1111' ./target/release/flo-node-service"
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

 - /usr/lib/systemd/system/flo-controller.service

```service
[Unit]
Description=Flo Controller Service
After=network.target
After=postgresql.target

[Service]
Type=simple
WorkingDirectory=/home/<YOUR_USERNAME>/flo
ExecStart=/bin/bash -l -c "FLO_NODE_SECRET='1111' ./target/release/flo-controller-service"
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

Make the services visible by running

```shell
sudo systemctl daemon-reload
```

Run the services with

```shell
sudo systemctl start flo-node
sudo systemctl start flo-controller
```

Trace logs

```shell
journalctl -f -u flo-node
journalctl -f -u flo-controller
```

Run automatically on system startup

```shell
sudo systemctl enable postgresql
sudo systemctl enable flo-node
sudo systemctl enable flo-controller
```

TESTING
-------

Before building flo-cli, make sure you have `g++` (C++ compiler) `zlib1g-dev` (ZLIB library), `libbz2-dev` (BZIP2 library) and `libavahi-compat-libdnssd-dev` (dnssd library) installed.

```shell
sudo apt install g++ zlib1g-dev libbz2-dev libavahi-compat-libdnssd-dev
```

### Build flo-cli

```shell
cargo build -p flo-cli --release
```

If you receive an error saying something like ```multiple definition of `zlibVersion'``` when building flo-cli, try deleting the `target` directory and then build flo-cli first, then flo-node-service, then flo-controller-service.

### Run flo-cli

```shell
./target/release/flo-cli server --help
./target/release/flo-cli server list-nodes
./target/release/flo-cli server upsert-player 1
```

Copy token from the last command and use it to run flo-worker locally

```shell
flo-worker.exe --controller-host="45.33.104.208" --token="eyJ0...."
```

If you get no errors you can create test game

```shell
./target/release/flo-cli server run-game 2
```

Tips
----

To update the IP address of a node, you may use:
```shell
psql -U postgres -d flo -c "update node set ip_addr = '45.33.104.208' WHERE id = 1"
psql -U postgres -d flo -c "select * from node"
sudo systemctl restart flo-node
sudo systemctl restart flo-controller
```
