FROM rust:1-bullseye AS builder

WORKDIR /usr/local/build

# Install required build dependencies
RUN apt-get update && apt-get install -y \
    cmake \
    libavahi-compat-libdnssd-dev \
    && rm -rf /var/lib/apt/lists/*

COPY . .

RUN rustup component add rustfmt

RUN cargo build -p flo-cli --release

FROM debian:bullseye-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl-dev \
    libpq-dev \
    libavahi-compat-libdnssd-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/local/s2

ARG IMAGE_BUILD_DATE=2016-01-01
ENV IMAGE_BUILD_DATE=$IMAGE_BUILD_DATE

ENV RUST_BACKTRACE=1

COPY --from=builder /usr/local/build/target/release/flo-cli /usr/local/flo/flo-cli

CMD ["/usr/local/flo/flo-cli"]