FROM rust:1-bullseye AS builder

WORKDIR /usr/local/build

COPY . .

RUN rustup component add rustfmt

ARG BUILD_MODE=release
RUN cargo build -p flo-controller-service $(if [ "$BUILD_MODE" = "debug" ]; then echo ""; else echo "--release"; fi)

FROM debian:bullseye-slim

RUN apt-get update && apt-get install -y ca-certificates libssl-dev libpq-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/local/s2

ARG IMAGE_BUILD_DATE=2016-01-01
ENV IMAGE_BUILD_DATE $IMAGE_BUILD_DATE

ENV RUST_BACKTRACE 1

EXPOSE 3549/tcp
EXPOSE 3550/tcp

ARG BUILD_MODE=release
COPY --from=builder /usr/local/build/target/${BUILD_MODE}/flo-controller-service /usr/local/flo/flo-controller-service

CMD ["/usr/local/flo/flo-controller-service"]