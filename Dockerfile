FROM rust:1.92-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY prompts ./prompts
COPY config ./config

RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 10001 governance

WORKDIR /app
COPY --from=builder /app/target/release/governance-agent /usr/local/bin/governance-agent
COPY --from=builder /app/prompts ./prompts
COPY --from=builder /app/config ./config

USER governance
ENV RUST_LOG=info

ENTRYPOINT ["governance-agent"]
CMD ["run"]
