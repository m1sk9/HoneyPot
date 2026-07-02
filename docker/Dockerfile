FROM lukemathwalker/cargo-chef:latest-rust-1.93.1-bookworm AS chef
WORKDIR /root/app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS cook
COPY --from=planner /root/app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

FROM cook AS builder
COPY . .
RUN cargo build --release --bin honeypot

FROM gcr.io/distroless/cc-debian12 AS runner

COPY --from=builder --chown=root:root /root/app/target/release/honeypot /

LABEL org.opencontainers.image.source=https://github.com/m1sk9/HoneyPot

# Log level defaults to `honeypot=info`. Set the RUST_LOG environment variable
# to override it at runtime, e.g. `RUST_LOG=honeypot=debug`. The guild
# configuration is read from `config/config.toml` by default; override the path
# with the HONEYPOT_CONFIG_PATH environment variable.

CMD ["./honeypot"]
