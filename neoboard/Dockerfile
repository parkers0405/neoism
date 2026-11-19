FROM rust:1.92-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release --bins

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && install -d -m 0700 -o 10001 -g 10001 /var/lib/neochat
COPY --from=build /build/target/release/neochat /build/target/release/neochatd /build/target/release/neochat-mcp /build/target/release/neochat-share /usr/local/bin/
USER 10001:10001
ENV NEOCHAT_DATA_DIR=/var/lib/neochat NEOCHAT_BIND=0.0.0.0:41580
EXPOSE 41580
ENTRYPOINT ["neochatd"]