FROM rust:1.94-slim-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# 依存関係だけを先にビルドし、レイヤーキャッシュを効かせます。
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && cargo build --locked --release \
    && rm -rf src

COPY src ./src
# ダミーのmainでビルドされた成果物を破棄し、本体を再ビルドします。
RUN touch src/main.rs \
    && cargo build --locked --release

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
