FROM rust:1.60.0 as build

RUN apt-get update
RUN rustup component add rustfmt
WORKDIR /build
COPY . .
RUN cargo build --release

FROM rust:1.60.0 as server

RUN apt-get update
RUN apt-get install -y ca-certificates
COPY --from=build /build/target/release/khaos-monkey .

ENTRYPOINT ["./khaos-monkey"]