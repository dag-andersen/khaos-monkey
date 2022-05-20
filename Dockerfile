FROM rust:1.61.0 as build

RUN apt-get update
RUN rustup component add rustfmt
WORKDIR /build
COPY . .
RUN cargo build --release

FROM build as test

COPY --from=build . .
RUN cargo test --release

FROM debian:bookworm-slim as server

RUN apt-get update
RUN apt-get install -y ca-certificates
COPY --from=build /build/target/release/khaos-monkey .

ENTRYPOINT ["./khaos-monkey"]