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

> [!NOTE]
> You can also use Docker - `docker-compose up -d postgres`. 
> You still need to run `diesel setup` to create the schema. 

1. Set the postgres db user password

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

Ensure that the `FLO_CONTROLLER_SECRET` is set to the same value inserted into the `api_client` table earlier.  
For local development, you can use the default value `1111`: `FLO_CONTROLLER_SECRET=1111 ./target/release/flo-cli`

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

Mac Installation Guide
-------------------

### Install Rust

```shell
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.zshrc   # Reload shell
```

### Install Diesel & Other Dependencies

```shell
brew install cargo-binstall
cargo binstall diesel_cli
brew install pkg-config gcc zlib bzip2 avahi libpq
```

### Install CMake 3 (not compatible with CMake 4 due to casclib)

```shell
curl -LO https://cmake.org/files/v3.31/cmake-3.31.6-macos-universal.dmg && hdiutil attach cmake-3.31.6-macos-universal.dmg && sudo cp -R /Volumes/cmake-3.31.6-macos-universal/CMake.app /
Applications && echo 'export PATH="/Applications/CMake.app/Contents/bin:$PATH"' >> ~/.zshrc && source ~/.zshrc

```

### Set Environment Variables

```shell
export LDFLAGS="-L/opt/homebrew/opt/libpq/lib"
export CPPFLAGS="-I/opt/homebrew/opt/libpq/include"
export PKG_CONFIG_PATH="/opt/homebrew/opt/libpq/lib/pkgconfig"
export LIBRARY_PATH="$LIBRARY_PATH:/opt/homebrew/opt/libpq/lib"
```

### Compile

```shell
cargo build
```

Windows Installation Guide
-------------------

### Install Binstall Package Manager

```powershell
Set-ExecutionPolicy Unrestricted -Scope Process
iex (iwr "https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.ps1").Content
```

### Fix Missing libpq.lib

1. Install PostgreSQL from https://www.postgresql.org/download/windows/
2. Add PostgreSQL library path to your LIB environment variable:
   ```powershell
   $env:LIB += ";C:\Program Files\PostgreSQL\17\lib"
   ```
   > [!NOTE]
   > To make this permanent, add this path to the LIB system environment variable.

### Install Diesel CLI

```powershell
cargo binstall diesel_cli
```

### Clone & Setup

Follow the same steps as in the Ubuntu guide for:
- Cloning the repository
- Creating the .env file
- Setting up the database
- Building and running the services