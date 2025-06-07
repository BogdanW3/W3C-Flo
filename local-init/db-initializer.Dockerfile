FROM rust:1-bullseye AS builder

WORKDIR /usr/local/build

RUN cargo install diesel_cli --no-default-features --features "postgres"

FROM debian:bullseye-slim

RUN apt-get update && apt-get install -y ca-certificates libpq-dev netcat postgresql-client dos2unix && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/local/flo

# Copy diesel binary
COPY --from=builder /usr/local/cargo/bin/diesel /usr/local/cargo/bin/diesel

COPY local-init/init-db.sh /usr/local/flo/init-db.sh
RUN dos2unix /usr/local/flo/init-db.sh

CMD ["/bin/bash", "-c", "/usr/local/flo/init-db.sh"] 