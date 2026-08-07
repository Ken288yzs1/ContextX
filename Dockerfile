FROM rust:1.94-slim-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -u 1000 user

COPY --from=builder /app/target/release/contextX /usr/local/bin/contextX

USER user
ENV HOME=/home/user \
    BIND_ADDR=0.0.0.0:7860
EXPOSE 7860
ENTRYPOINT ["/usr/local/bin/contextX"]
