FROM rust:1-slim AS build

WORKDIR /build
COPY . .
RUN cargo build --release --locked

FROM debian:stable-slim

COPY --from=build /build/target/release/sqlfmt /usr/local/bin/sqlfmt

RUN mkdir /src
WORKDIR /src

CMD ["sqlfmt", "."]
